//! Pure workers never receive source grants, settings, routes or model request bodies.
use super::super::{Checked, Manager, Result, fail, run_sync, worker::RpcRequest};
use super::store::{Binding, Installed, Selection};
use local_agent_core::{
    domain::routing_policy as r,
    plugin::{external as common, policy as p},
};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::io::{AsyncWriteExt, BufReader};
#[derive(Clone)]
pub(super) struct Context {
    pub package: Installed,
    pub path: PathBuf,
    pub before: Selection,
    pub epoch: u64,
    pub publisher_revision: u64,
    pub preflight: bool,
    pub binding: Option<Binding>,
}
impl Manager {
    async fn policy_alive(self: &Arc<Self>, c: &Context) -> Result<()> {
        let m = self.clone();
        let c = c.clone();
        run_sync(move || m.policy_check(&c)).await
    }
    pub(super) async fn policy_run(
        self: &Arc<Self>,
        c: Context,
        request: Option<r::RankRequest>,
    ) -> Result<Option<r::RankResult>> {
        if let Some(r) = &request {
            r.validate().map_err(|e| fail(e.code()))?;
            if serde_json::to_vec(r).checked("invalid_input")?.len() > 32_768 {
                return Err(fail("resource_limit"));
            }
        }
        let _permit = self.workers.try_acquire().checked("resource_limit")?;
        self.policy_alive(&c).await?;
        let mut child =
            tokio::process::Command::new(std::env::current_exe().checked("unavailable")?)
                .arg("--plugin-worker")
                .arg(&c.path)
                .arg(&c.package.digest)
                .env_clear()
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .checked("worker_crashed")?;
        let pid = child.id().ok_or(fail("worker_crashed"))?;
        let mut client = Client {
            input: child.stdin.take().ok_or(fail("worker_crashed"))?,
            output: BufReader::new(child.stdout.take().ok_or(fail("worker_crashed"))?),
            sequence: 0,
        };
        let work = async {
            let init = serde_json::to_value(common::Initialize {
                api_version: p::API_VERSION.into(),
                capabilities: vec![p::CAPABILITY.into()],
                package_digest: c.package.digest.clone(),
                activation_epoch: c.epoch.to_string(),
                grant_revision: c.publisher_revision.to_string(),
                context_id: uuid::Uuid::new_v4().to_string(),
                fuel: "100000000".into(),
                max_memory_bytes: "134217728".into(),
                settings: Default::default(),
            })
            .checked("invalid_input")?;
            if client
                .exchange(self, &c, "plugin.initialize", init.clone())
                .await?
                != init
            {
                return Err(fail("incompatible_api"));
            }
            for method in ["plugin.prepare", "plugin.activate", "plugin.health"] {
                if client.exchange(self, &c, method, json!({})).await? != json!({"ready":true}) {
                    return Err(fail("invalid_record"));
                }
            }
            let result = if let Some(request) = request {
                let invocation = p::RankInvocation {
                    request_identity: request.identity(),
                    request: request.clone(),
                };
                let value = client
                    .exchange(
                        self,
                        &c,
                        "policy.rank",
                        serde_json::to_value(invocation).checked("invalid_input")?,
                    )
                    .await?;
                if serde_json::to_vec(&value).checked("invalid_record")?.len() > 16_384 {
                    return Err(fail("resource_limit"));
                }
                let ranked: r::RankResult =
                    serde_json::from_value(value).checked("invalid_record")?;
                if ranked.reason != r::RankReason::ExtensionOrder {
                    return Err(fail("invalid_record"));
                }
                Some(r::inspect_result(&request, ranked).map_err(|e| fail(e.code()))?)
            } else {
                None
            };
            for method in ["plugin.deactivate", "plugin.dispose"] {
                if client.exchange(self, &c, method, json!({})).await? != json!({"ready":true}) {
                    return Err(fail("invalid_record"));
                }
            }
            self.policy_alive(&c).await?;
            Ok(result)
        };
        let monitored = async {
            tokio::select! {result=work=>result,result=super::super::broker::monitor_rss(pid)=>Err(result.err().unwrap_or(fail("worker_crashed")))}
        };
        let result = tokio::time::timeout(Duration::from_secs(3), monitored)
            .await
            .checked("deadline_exceeded")
            .and_then(|r| r);
        drop(client);
        if result.is_err() {
            let _ = child.kill().await;
        }
        let exited = tokio::time::timeout(Duration::from_secs(1), child.wait()).await;
        if !matches!(exited,Ok(Ok(status))if status.success()) && result.is_ok() {
            let _ = child.kill().await;
            return Err(fail("worker_crashed"));
        }
        result
    }
}
struct Client {
    input: tokio::process::ChildStdin,
    output: BufReader<tokio::process::ChildStdout>,
    sequence: u32,
}
impl Client {
    async fn exchange(
        &mut self,
        m: &Arc<Manager>,
        c: &Context,
        method: &str,
        params: Value,
    ) -> Result<Value> {
        m.policy_alive(c).await?;
        self.sequence += 1;
        let id = format!("host-{}", self.sequence);
        let mut bytes = serde_json::to_vec(&RpcRequest {
            jsonrpc: "2.0".into(),
            id: id.clone(),
            method: method.into(),
            params,
        })
        .checked("invalid_input")?;
        bytes.push(b'\n');
        if bytes.len() > common::MAX_FRAME {
            return Err(fail("resource_limit"));
        }
        self.input
            .write_all(&bytes)
            .await
            .checked("worker_crashed")?;
        self.input.flush().await.checked("worker_crashed")?;
        let pending = super::super::broker::read_frame(&mut self.output);
        tokio::pin!(pending);
        let bytes = loop {
            tokio::select! {r=&mut pending=>break r?,_=tokio::time::sleep(Duration::from_millis(25))=>m.policy_alive(c).await?,}
        };
        let value: Value = serde_json::from_slice(&bytes).checked("worker_crashed")?;
        if value["method"] == "host.call" {
            return Err(fail("permission_denied"));
        }
        if value["jsonrpc"] != "2.0" || value["id"] != id || value.get("method").is_some() {
            return Err(fail("worker_crashed"));
        }
        if let Some(error) = value.get("error") {
            return Err(fail(match error["code"].as_str() {
                Some("permission_denied") => "permission_denied",
                Some("resource_limit") => "resource_limit",
                _ => "worker_crashed",
            }));
        }
        m.policy_alive(c).await?;
        value.get("result").cloned().ok_or(fail("worker_crashed"))
    }
}
