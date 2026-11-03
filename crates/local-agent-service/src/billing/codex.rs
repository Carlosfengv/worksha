use local_agent_core::{billing::AccountSnapshot, providers::files};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub fn read() -> Result<AccountSnapshot, &'static str> {
    let binary =
        crate::providers::binary("codex").ok_or("未找到 Codex CLI，请先安装并在 Codex 登录")?;
    read_binary(&binary)
}
fn read_binary(binary: &std::path::Path) -> Result<AccountSnapshot, &'static str> {
    let mut command = Command::new(binary);
    command
        .args(["app-server", "--listen", "stdio://"])
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // The CLI owns its authentication; don't extract tokens or forward inherited API secrets.
    for key in [
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "CODEX_HOME",
        "CODEX_API_KEY",
        "CODEX_API_BASE_URL",
        "LOCAL_AGENT_SERVICE_TOKEN",
    ] {
        command.env_remove(key);
    }
    let mut server = Server(command.spawn().map_err(|_| "Codex 账户读取进程无法启动")?);
    let mut input = server.0.stdin.take().ok_or("Codex 输入不可用")?;
    let stdout = server.0.stdout.take().ok_or("Codex 输出不可用")?;
    let (sender, receiver) = mpsc::sync_channel(64);
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        // Bounded messages and channel; closing the receiver also ends this reader.
        for _ in 0..256 {
            let mut bytes = Vec::new();
            if std::io::Read::by_ref(&mut reader)
                .take(1024 * 1024 + 1)
                .read_until(b'\n', &mut bytes)
                .is_err()
                || bytes.is_empty()
                || bytes.len() > 1024 * 1024
            {
                break;
            }
            let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
                break;
            };
            if sender.send(value).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(18);
    let mut request = |id: u64, method: &str, params: Value| -> Result<Value, &'static str> {
        serde_json::to_writer(
            &mut input,
            &if id == 0 {
                json!({"method":method})
            } else {
                json!({"id":id,"method":method,"params":params})
            },
        )
        .map_err(|_| "Codex 请求未发送")?;
        input
            .write_all(b"\n")
            .and_then(|_| input.flush())
            .map_err(|_| "Codex 请求未发送")?;
        if id == 0 {
            return Ok(Value::Null);
        }
        loop {
            let timeout = deadline.saturating_duration_since(Instant::now());
            let value = receiver
                .recv_timeout(timeout)
                .map_err(|_| "Codex 账户读取超时或进程已退出")?;
            if value["id"] != id {
                continue;
            }
            if value["error"].is_object() {
                return Err("Codex 未授权或当前版本不支持账户查询，请在官方客户端核查");
            }
            return Ok(value["result"].clone());
        }
    };
    request(
        1,
        "initialize",
        json!({"clientInfo":{"name":"local_agent_usage_reader","version":"0.1.0"},"capabilities":{"experimentalApi":false}}),
    )?;
    request(0, "initialized", Value::Null)?;
    // Only account reads. No thread, inference, login, logout or redemption methods are exposed.
    let first = request(2, "account/read", json!({"refreshToken":false}))?;
    let limits = request(3, "account/rateLimits/read", Value::Null)?;
    let last = request(4, "account/read", json!({"refreshToken":false}))?;
    if first["account"] != last["account"] {
        return Err("读取期间账户发生变化，请重新刷新");
    }
    let mut snapshot = local_agent_core::billing::codex(&first["account"], &limits)?;
    let scope = files::hash(
        serde_json::to_string(&json!([first["account"], limits["accountId"]]))
            .unwrap_or_default()
            .as_bytes(),
    );
    snapshot.notes.push(format!(
        "账户范围 {} · 同一账户在其他客户端的用量共享。",
        &scope[..12]
    ));
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn rpc_reads_only_and_rejects_account_changes() {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("codex-fixture");
        let log = temp.path().join("methods");
        for changed in [false, true] {
            let code = format!(
                r#"#!/usr/bin/python3
import sys,json
count=0
for line in sys.stdin:
    v=json.loads(line); method=v['method']
    with open({log:?},'a') as f: f.write(method+'\n')
    if method=='initialized': continue
    if method=='initialize': result={{'userAgent':'fixture'}}
    elif method=='account/read':
        count+=1
        result={{'account':{{'type':'chatgpt','planType':'pro','email':'changed@example.test' if {changed} and count==2 else 'fixture@example.test'}}}}
    elif method=='account/rateLimits/read': result={{'accountId':'fixture','rateLimits':{{'primary':{{'usedPercent':20,'resetsAt':2000000000,'windowDurationMins':300}}}},'rateLimitResetCredits':{{'availableCount':3,'credits':[{{'expiresAt':None}}]}}}}
    else: sys.exit(10)
    print(json.dumps({{'id':v['id'],'result':result}}),flush=True)
"#,
                log = log.to_string_lossy(),
                changed = if changed { "True" } else { "False" }
            );
            std::fs::write(&script, code).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
            let result = read_binary(&script);
            if changed {
                assert!(result.is_err());
            } else {
                let s = result.unwrap();
                assert_eq!(s.plan.as_deref(), Some("pro"));
                assert_eq!(s.reset_credits.available_count, Some(3));
            }
        }
        let methods = std::fs::read_to_string(log).unwrap();
        assert!(methods.lines().all(|m| {
            [
                "initialize",
                "initialized",
                "account/read",
                "account/rateLimits/read",
            ]
            .contains(&m)
        }));
    }
}
