use super::{Checked, Lease, Manager, Query, Result, fail};
use crate::plugin_host::external::run_sync;
use crate::plugin_host::external::{
    broker::{monitor_rss, read_frame},
    worker::RpcRequest,
};
use local_agent_core::plugin::{
    advanced::API_VERSION,
    external::{Initialize, MAX_FRAME},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout},
    sync::{OwnedMutexGuard, OwnedSemaphorePermit},
};

/// One worker is held for one stream; every chunk checks the immutable lease.
pub(super) struct Worker {
    manager: Arc<Manager>,
    lease: Lease,
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    sequence: u64,
    _permit: OwnedSemaphorePermit,
    _gate: OwnedMutexGuard<()>,
    deadline: tokio::time::Instant,
}
impl Worker {
    pub(super) async fn open(manager: Arc<Manager>, q: &Query) -> Result<Self> {
        let permit = manager
            .workers
            .clone()
            .try_acquire_owned()
            .checked("resource_limit")?;
        let copy = manager.clone();
        let query = q.clone();
        let (p, lease, gate) = run_sync(move || {
            let (p, _, lease) = copy.extension(&query)?;
            Ok((p, lease, copy.gate(&query.plugin_id)?))
        })
        .await?;
        let gate = gate.try_lock_owned().checked("resource_limit")?;
        let mut child =
            tokio::process::Command::new(std::env::current_exe().checked("unavailable")?)
                .arg("--plugin-worker")
                .arg(p.path)
                .arg(&q.digest)
                .arg(if manager.development {
                    "development"
                } else {
                    "signed"
                })
                .env_clear()
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .checked("worker_crashed")?;
        let input = child.stdin.take().ok_or(fail("worker_crashed"))?;
        let output = BufReader::new(child.stdout.take().ok_or(fail("worker_crashed"))?);
        let mut worker = Self {
            manager,
            lease: lease.clone(),
            child,
            input,
            output,
            sequence: 0,
            _permit: permit,
            _gate: gate,
            deadline: tokio::time::Instant::now() + Duration::from_secs(300),
        };
        let init = Initialize {
            settings: BTreeMap::new(),
            api_version: API_VERSION.into(),
            capabilities: p.manifest.capabilities,
            package_digest: q.digest.clone(),
            activation_epoch: lease.epoch,
            grant_revision: lease.revision,
            context_id: uuid::Uuid::new_v4().to_string(),
            fuel: "100000000".into(),
            max_memory_bytes: "134217728".into(),
        };
        for (method, params) in [
            (
                "plugin.initialize",
                serde_json::to_value(init).checked("invalid_input")?,
            ),
            ("plugin.prepare", json!({})),
            ("plugin.activate", json!({})),
            ("plugin.health", json!({})),
        ] {
            worker.call(method, params).await?;
        }
        Ok(worker)
    }
    pub(super) async fn check(&self) -> Result<()> {
        if tokio::time::Instant::now() >= self.deadline {
            return Err(fail("deadline_exceeded"));
        }
        tokio::time::timeout_at(self.deadline, self.lease.check_async(&self.manager))
            .await
            .checked("deadline_exceeded")?
    }
    pub(super) async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.check().await?;
        self.sequence += 1;
        if self.sequence > 20000 {
            return Err(fail("resource_limit"));
        }
        let id = format!("host-{}", self.sequence);
        let mut frame = serde_json::to_vec(&RpcRequest {
            jsonrpc: "2.0".into(),
            id: id.clone(),
            method: method.into(),
            params,
        })
        .checked("invalid_input")?;
        frame.push(b'\n');
        if frame.len() > MAX_FRAME {
            return Err(fail("resource_limit"));
        }
        let pid = self.child.id().ok_or(fail("worker_crashed"))?;
        let deadline = self
            .deadline
            .min(tokio::time::Instant::now() + Duration::from_secs(10));
        let manager = self.manager.clone();
        let lease = self.lease.clone();
        let work = async {
            self.input
                .write_all(&frame)
                .await
                .checked("worker_crashed")?;
            self.input.flush().await.checked("worker_crashed")?;
            let frame = read_frame(&mut self.output).await?;
            let v: Value = serde_json::from_slice(&frame).checked("worker_crashed")?;
            if v["jsonrpc"] != "2.0"
                || v["id"] != id
                || v.get("method").is_some()
                || v.get("error").is_some()
                || v.get("result").is_none()
            {
                return Err(fail("worker_crashed"));
            }
            lease.check_async(&manager).await?;
            Ok(v["result"].clone())
        };
        let revoked = async {
            loop {
                tokio::time::sleep(Duration::from_millis(50)).await;
                lease.check_async(&manager).await?;
            }
        };
        tokio::select! {
            r=tokio::time::timeout_at(deadline,work)=>r.checked("deadline_exceeded")?,
            r=revoked=>r,
            _=monitor_rss(pid)=>Err(fail("resource_limit"))
        }
    }
    pub(super) async fn close(mut self) -> Result<()> {
        self.call("plugin.deactivate", json!({})).await?;
        self.call("plugin.dispose", json!({})).await?;
        drop(self.input);
        let deadline = self
            .deadline
            .min(tokio::time::Instant::now() + Duration::from_secs(1));
        tokio::time::timeout_at(deadline, self.child.wait())
            .await
            .checked("deadline_exceeded")?
            .checked("worker_crashed")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_host::external::{
        StateAction, StateRequest,
        tests::{install, manifest, new_host},
    };

    async fn worker_fixture(command: &mut tokio::process::Command) -> (tempfile::TempDir, Worker) {
        let root = tempfile::tempdir().unwrap();
        let manager = new_host(root.path());
        let installed = install(&manager, root.path(), &manifest());
        let status = manager
            .change(StateRequest {
                plugin_id: installed.manifest.id,
                digest: installed.digest,
                action: StateAction::Enable,
            })
            .await
            .unwrap();
        let gate = manager.gate(&status.manifest.id).unwrap();
        let mut child = command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let worker = Worker {
            manager: manager.clone(),
            lease: Lease::from(&status),
            input: child.stdin.take().unwrap(),
            output: BufReader::new(child.stdout.take().unwrap()),
            child,
            sequence: 0,
            _permit: manager.workers.clone().try_acquire_owned().unwrap(),
            _gate: gate.try_lock_owned().unwrap(),
            deadline: tokio::time::Instant::now() + Duration::from_millis(100),
        };
        (root, worker)
    }

    #[tokio::test]
    async fn worker_call_obeys_the_remaining_lifetime_and_releases_its_resources() {
        let (_root, mut worker) =
            worker_fixture(tokio::process::Command::new("/bin/sleep").arg("30")).await;
        let manager = worker.manager.clone();
        let gate = manager.gate(&worker.lease.id).unwrap();
        let capacity = manager.workers.available_permits();
        let result = tokio::time::timeout(
            Duration::from_secs(2),
            worker.call("protocol.feed", json!({"bytes":[]})),
        )
        .await;
        // Reap the fixture even if the lifetime assertion fails.
        worker.child.kill().await.unwrap();
        drop(worker);
        assert_eq!(manager.workers.available_permits(), capacity + 1);
        assert!(gate.try_lock_owned().is_ok());
        assert_eq!(
            result
                .expect("worker call exceeded its remaining lifetime")
                .unwrap_err()
                .code,
            "deadline_exceeded"
        );
    }

    #[tokio::test]
    async fn worker_close_obeys_the_remaining_lifetime_and_releases_its_resources() {
        let script = concat!(
            "read -r line; printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":\"host-1\",\"result\":{}}'; ",
            "read -r line; printf '%s\\n' '{\"jsonrpc\":\"2.0\",\"id\":\"host-2\",\"result\":{}}'; ",
            "exec /bin/sleep 30"
        );
        let (_root, worker) =
            worker_fixture(tokio::process::Command::new("/bin/sh").args(["-c", script])).await;
        let manager = worker.manager.clone();
        let gate = manager.gate(&worker.lease.id).unwrap();
        let capacity = manager.workers.available_permits();
        let result = tokio::time::timeout(Duration::from_millis(500), worker.close())
            .await
            .expect("worker close exceeded its remaining lifetime");
        assert_eq!(result.unwrap_err().code, "deadline_exceeded");
        assert_eq!(manager.workers.available_permits(), capacity + 1);
        assert!(gate.try_lock_owned().is_ok());
    }
}
