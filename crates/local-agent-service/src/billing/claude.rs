use super::*;
use local_agent_core::providers::ConfigError;

const MANAGED_QUOTA: &str = "claude-quota-managed.json";

#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(crate) enum Action {
    Status,
    Preview {
        enabled: bool,
        expected_revision: String,
    },
    Apply {
        review_id: String,
    },
    Cancel {
        review_id: String,
    },
}

pub(crate) async fn statusline(
    State(state): State<AppState>,
    Json(action): Json<Action>,
) -> ApiResult {
    crate::blocking::management(move || {
        let (directory, home) = {
            let billing = state.billing.lock().map_err(|_| err("账户服务不可用"))?;
            (billing.directory.clone(), billing.home.clone())
        };
        let manager = state.providers.lock().map_err(|_| err("配置事务暂不可用"))?;
        let config_error = crate::providers::error;
        match action {
            Action::Status => {},
            Action::Preview { enabled, expected_revision } => {
                let status = manager.claude_statusline().map_err(config_error)?;
                if status.revision != expected_revision {
                    return Err(config_error(ConfigError::new("conflict", "Claude 配置版本已变化，请重新预览")));
                }
                if enabled && status.state == "custom" {
                    return Err(config_error(ConfigError::new("conflict", "Claude 已配置自定义状态栏，保留原配置")));
                }
                let field = if enabled {
                    let helper = usage_reader(&directory).map_err(err)?;
                    Some(managed_field(&helper, &directory, FsPath::new(&status.config_path), uuid::Uuid::new_v4()))
                } else {
                    None
                };
                let preview = manager.prepare_claude_statusline(&expected_revision, field.clone()).map_err(config_error)?;
                return json_value(json!({"reviewId":preview.id,"configPath":status.config_path,"enabled":enabled,"statusLine":field}));
            }
            Action::Apply { review_id } => {
                let _guard = capture_lock(&directory).map_err(err)?;
                manager.apply_claude_statusline(&review_id).map_err(config_error)?;
            }
            Action::Cancel { review_id } => {
                let status = manager.claude_statusline().map_err(config_error)?;
                if status.pending.as_deref() != Some(review_id.as_str()) {
                    return Err(err("找不到待应用的状态栏操作"));
                }
                manager.cancel(&review_id).map_err(config_error)?;
            }
        }
        let status = manager.claude_statusline().map_err(config_error)?;
        json_value(json!({"config":status, "received":read_managed(&directory,&home).is_some()}))
    }).await
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ManagedQuota {
    field: String,
    snapshot: AccountSnapshot,
}

fn managed_field(
    helper: &FsPath,
    directory: &FsPath,
    settings: &FsPath,
    generation: uuid::Uuid,
) -> Value {
    let command = format!(
        "{} --claude-quota-stdin {} --managed {} {generation}",
        quote(&helper.to_string_lossy()),
        quote(&directory.to_string_lossy()),
        quote(&settings.to_string_lossy()),
    );
    json!({"type":"command","command":command})
}

fn current_field(settings: &FsPath) -> Option<Value> {
    let snapshot = files::read(settings).ok()?;
    let value: Value = serde_json::from_slice(snapshot.bytes.as_deref()?).ok()?;
    value.get("statusLine").cloned()
}

fn capture_lock(directory: &FsPath) -> Result<std::fs::File, &'static str> {
    files::private_dir(directory).map_err(|_| "额度目录不可用")?;
    files::exclusive_lock(&directory.join("claude-statusline.lock"))
        .map_err(|_| "Claude 状态栏正在更新，请稍后重试")
}

pub(super) fn read_managed(directory: &FsPath, home: &FsPath) -> Option<AccountSnapshot> {
    let file = files::read(&directory.join(MANAGED_QUOTA)).ok()?;
    if file.stamp.mode != 0o600 {
        return None;
    }
    let saved: ManagedQuota = serde_json::from_slice(file.bytes.as_deref()?).ok()?;
    let current = current_field(&home.join(".claude/settings.json"))?;
    (saved.field == files::hash(&serde_json::to_vec(&current).ok()?)).then_some(saved.snapshot)
}

pub(crate) fn capture_managed_stdin(
    directory: &FsPath,
    settings: &FsPath,
    generation: &str,
) -> Result<(), &'static str> {
    let generation = uuid::Uuid::parse_str(generation).map_err(|_| "Claude 额度入口无效")?;
    let executable = std::env::current_exe().map_err(|_| "采集程序不可用")?;
    let expected = managed_field(&executable, directory, settings, generation);
    let (snapshot, text) = parse_claude_capture(&read_quota_stdin()?)?;
    let _lock = capture_lock(directory)?;
    if current_field(settings).as_ref() != Some(&expected) {
        return Ok(());
    }
    let file = ManagedQuota {
        field: files::hash(&serde_json::to_vec(&expected).map_err(|_| "采集入口格式无效")?),
        snapshot,
    };
    files::atomic_write(
        &directory.join(MANAGED_QUOTA),
        &serde_json::to_vec(&file).map_err(|_| "采集格式无效")?,
        0o600,
    )
    .map_err(|_| "采集快照未保存")?;
    println!("{text}");
    Ok(())
}
