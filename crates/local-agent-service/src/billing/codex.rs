use super::{ApiResult, AppState, catalog, err, json_value};
use axum::{Json, extract::State};
use local_agent_core::{
    billing::AccountSnapshot,
    providers::{ProviderManager, files},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
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
    let email = first["account"]["email"]
        .as_str()
        .filter(|email| !email.is_empty());
    let account_id = limits["accountId"].as_str().filter(|id| !id.is_empty());
    snapshot.identity = None;
    if let Some(evidence) = account_id.or(email) {
        let scope = files::hash(evidence.as_bytes());
        snapshot.notes.push(format!(
            "账户范围 {} · 同一账户在其他客户端的用量共享。",
            &scope[..12]
        ));
    }
    Ok(snapshot)
}

const CONSENT_FILE: &str = "codex-read-consent.json";
const QUOTA_FILE: &str = "codex-quota.json";
const FORMAT: u8 = 1;

#[derive(Clone, Serialize)]
pub(super) struct ReadState {
    pub(super) authorized: bool,
    pub(super) historical: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Consent {
    format: u8,
    source: String,
    revision: String,
    generation: String,
    enabled: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Quota {
    format: u8,
    source: String,
    snapshot: AccountSnapshot,
}

fn source_key(home: &std::path::Path) -> String {
    files::hash(home.as_os_str().as_encoded_bytes())
}

fn local_record<T: DeserializeOwned>(directory: &std::path::Path, name: &str) -> Option<T> {
    let file = files::read(&directory.join(name)).ok()?;
    if file.stamp.mode != 0o600 {
        return None;
    }
    serde_json::from_slice(file.bytes.as_deref()?).ok()
}

pub(super) fn authorized_generation(
    directory: &std::path::Path,
    home: &std::path::Path,
    revision: &str,
) -> Option<String> {
    let consent: Consent = local_record(directory, CONSENT_FILE)?;
    (consent.format == FORMAT
        && consent.enabled
        && consent.source == source_key(home)
        && consent.revision == revision)
        .then_some(consent.generation)
}

pub(super) fn current_revision(manager: &ProviderManager, revision: &str) -> bool {
    !revision.is_empty()
        && manager.inspect_configs().iter().any(|config| {
            config.agent_id == "codex"
                && matches!(config.state.as_str(), "ready" | "absent")
                && config.revision.as_deref() == Some(revision)
        })
}

pub(super) fn history(
    directory: &std::path::Path,
    home: &std::path::Path,
) -> Option<AccountSnapshot> {
    let quota: Quota = local_record(directory, QUOTA_FILE)?;
    (quota.format == FORMAT && quota.source == source_key(home)).then_some(quota.snapshot)
}

pub(super) fn persist(
    directory: &std::path::Path,
    home: &std::path::Path,
    snapshot: &AccountSnapshot,
) -> Result<(), &'static str> {
    write_record(
        directory,
        QUOTA_FILE,
        &Quota {
            format: FORMAT,
            source: source_key(home),
            snapshot: snapshot.clone(),
        },
    )
}

fn write_record(
    directory: &std::path::Path,
    name: &str,
    record: &impl Serialize,
) -> Result<(), &'static str> {
    let content = serde_json::to_vec(record).map_err(|_| "Codex 额度数据格式无效")?;
    files::private_dir(directory).map_err(|_| "Codex 额度目录不可安全写入")?;
    files::atomic_write(&directory.join(name), &content, 0o600)
        .map_err(|_| "Codex 额度数据未能保存")
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ConsentChange {
    expected_revision: String,
    enabled: bool,
}

pub(crate) async fn consent(
    State(state): State<AppState>,
    Json(change): Json<ConsentChange>,
) -> ApiResult {
    crate::blocking::management(move || {
        let account = catalog(&state)?
            .into_iter()
            .find(|item| item.id == "native-codex")
            .ok_or_else(|| err("Codex 额度来源不可用"))?;
        if change.expected_revision != account.source_revision
            || (change.enabled && (!account.readable || account.source_revision.is_empty()))
        {
            return Err(err("Codex 额度来源已变更，请重新读取"));
        }
        let manager = state
            .providers
            .lock()
            .map_err(|_| err("配置读取暂不可用"))?;
        let _config = manager.provider_edit_guard().map_err(|e| err(&e.message))?;
        if change.enabled && !current_revision(&manager, &change.expected_revision) {
            return Err(err("Codex 额度来源已变更，请重新读取"));
        }
        let mut billing = state.billing.lock().map_err(|_| err("账户服务暂不可用"))?;
        write_record(
            &billing.directory,
            CONSENT_FILE,
            &Consent {
                format: FORMAT,
                source: source_key(&billing.home),
                revision: change.expected_revision,
                generation: uuid::Uuid::new_v4().to_string(),
                enabled: change.enabled,
            },
        )
        .map_err(err)?;
        if !change.enabled {
            billing.cache.remove("native-codex");
            billing.failures.remove("native-codex");
        }
        for (_, active) in billing
            .in_flight
            .iter()
            .filter(|(key, _)| key.account == "native-codex")
        {
            active.store(false, std::sync::atomic::Ordering::Release);
        }
        json_value(json!({"authorized": change.enabled}))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn rpc_reads_only_and_rejects_account_changes() {
        let temp = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
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
                let snapshot = result.unwrap();
                assert_eq!(snapshot.plan.as_deref(), Some("pro"));
                assert_eq!(snapshot.reset_credits.available_count, Some(3));
                let billing = temp.path().join("billing");
                persist(&billing, temp.path(), &snapshot).unwrap();
                let saved = std::fs::read_to_string(billing.join(QUOTA_FILE)).unwrap();
                assert!(!saved.contains("fixture@example.test"));
                assert_eq!(
                    history(&billing, temp.path()).unwrap().plan.as_deref(),
                    Some("pro")
                );
                assert!(history(&billing, &temp.path().join("other-home")).is_none());
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
