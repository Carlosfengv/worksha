use super::{ConfigError, Journal, Plan, ProviderManager, Transaction, config, fail, files};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::Path;

const FIELD: &str = "statusLine";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Change {
    digest: Option<String>,
    created_file: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeStatusLine {
    pub config_path: String,
    pub revision: String,
    pub state: &'static str,
    pub pending: Option<String>,
}

fn digest(value: Option<&Value>) -> Option<String> {
    value.map(|value| files::hash(&serde_json::to_vec(value).expect("JSON value")))
}

fn latest<'a>(journal: &'a Journal, path: &Path) -> Option<&'a Change> {
    journal.transactions.iter().rev().find_map(|transaction| {
        (transaction.state == "applied" && Path::new(&transaction.before.path) == path)
            .then_some(transaction.claude_statusline.as_ref())
            .flatten()
    })
}

fn settings(value: &Value) -> Result<Vec<u8>, ConfigError> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|_| ConfigError::new("invalid_config", "无法生成 Claude 状态栏配置"))?;
    bytes.push(b'\n');
    if bytes.len() as u64 > files::LIMIT {
        return fail("invalid_config", "生成的配置超过 1 MiB，原文件未修改");
    }
    Ok(bytes)
}

impl ProviderManager {
    pub fn claude_statusline(&self) -> Result<ClaudeStatusLine, ConfigError> {
        let _lock = self.lock()?;
        let mut journal = self.journal()?;
        self.recover(&mut journal)?;
        let path = self.adapters.resolve("claude")?.path(&self.home)?;
        let snapshot = files::read(&path)?;
        let value = config::parse_json(snapshot.bytes.as_deref())?;
        let field = digest(value.get(FIELD));
        let saved = latest(&journal, &path);
        let state = match field {
            Some(field) if saved.and_then(|change| change.digest.as_ref()) == Some(&field) => {
                "installed"
            }
            _ if saved.is_some_and(|change| change.digest.is_some()) => "replaced",
            Some(_) => "custom",
            None => "disabled",
        };
        Ok(ClaudeStatusLine {
            config_path: path.to_string_lossy().into_owned(),
            revision: snapshot.stamp.digest.unwrap_or_else(|| "absent".into()),
            state,
            pending: journal
                .transactions
                .iter()
                .rev()
                .find(|transaction| {
                    transaction.agent_id == "claude"
                        && transaction.state == "pending"
                        && transaction.claude_statusline.is_some()
                })
                .map(|transaction| transaction.id.clone()),
        })
    }

    pub fn prepare_claude_statusline(
        &self,
        expected_revision: &str,
        desired: Option<Value>,
    ) -> Result<Transaction, ConfigError> {
        let _lock = self.lock()?;
        let mut journal = self.journal()?;
        self.recover(&mut journal)?;
        if journal
            .transactions
            .iter()
            .any(|transaction| transaction.agent_id == "claude" && transaction.state == "pending")
        {
            return fail("conflict", "Claude 有待应用的配置事务，请先应用或撤销");
        }
        let path = self.adapters.resolve("claude")?.path(&self.home)?;
        let before = files::read(&path)?;
        if before.stamp.digest.as_deref().unwrap_or("absent") != expected_revision {
            return fail("conflict", "Claude 配置版本已变化，请重新预览");
        }
        let mut next = config::parse_json(before.bytes.as_deref())?;
        let current = digest(next.get(FIELD));
        let previous = latest(&journal, &path);
        let owned = current.is_some() && previous.is_some_and(|saved| saved.digest == current);
        let enable = desired.is_some();
        let change = Change {
            digest: digest(desired.as_ref()),
            created_file: enable
                && (before.bytes.is_none()
                    || (owned && previous.is_some_and(|saved| saved.created_file))),
        };
        let after = if let Some(value) = desired {
            if current.is_some() && !owned {
                return fail("conflict", "Claude 已配置自定义状态栏，保留原配置");
            }
            if value["type"] != "command" || value["command"].as_str().is_none_or(str::is_empty) {
                return fail("invalid_config", "受管状态栏命令无效");
            }
            next[FIELD] = value;
            Some(settings(&next)?)
        } else if owned {
            let next = next.as_object_mut().expect("validated settings");
            next.remove(FIELD);
            if next.is_empty()
                && (previous.is_some_and(|saved| saved.created_file)
                    || self.statusline_only_restore(&journal, &path, &before)?)
            {
                None
            } else {
                Some(settings(&Value::Object(std::mem::take(next)))?)
            }
        } else {
            // A user replacement already retired the helper. Journal the
            // disable without rewriting any part of their settings.
            before.bytes.clone()
        };
        let after_mode = if before.bytes.is_none() {
            0o600
        } else {
            before.stamp.mode
        };
        self.add(
            &mut journal,
            Transaction {
                local_token_operation: None,
                claude_statusline: Some(change),
                id: uuid::Uuid::new_v4().to_string(),
                agent_id: "claude".into(),
                state: "pending".into(),
                created_at: Utc::now().to_rfc3339(),
                applied_at: None,
                before: self.inspect_target("claude", &path),
                target_name: if enable {
                    "开启 Claude 额度状态栏"
                } else {
                    "关闭 Claude 额度状态栏"
                }
                .into(),
                provider: None,
                model: None,
                backup_path: String::new(),
                restore_of: None,
                error: None,
                routing: None,
                activation: None,
            },
            Plan {
                before: before.bytes,
                after,
                before_stamp: before.stamp,
                after_mode,
            },
        )
    }

    pub fn apply_claude_statusline(&self, id: &str) -> Result<Transaction, ConfigError> {
        self.apply_checked_kind(id, || Ok(None), || Ok(()), true)
    }

    fn statusline_only_restore(
        &self,
        journal: &Journal,
        path: &Path,
        current: &files::FileSnapshot,
    ) -> Result<bool, ConfigError> {
        let Some(restore) = journal.transactions.iter().rev().find(|transaction| {
            transaction.state == "applied"
                && Path::new(&transaction.before.path) == path
                && transaction.restore_of.is_some()
        }) else {
            return Ok(false);
        };
        let restored = self.plan(&restore.id)?;
        if restored.after.as_deref() != current.bytes.as_deref() {
            return Ok(false);
        }
        Ok(self
            .plan(restore.restore_of.as_deref().expect("filtered restore"))?
            .before
            .is_none())
    }
}

pub(super) fn restore_route(
    journal: &Journal,
    path: &Path,
    current: &files::FileSnapshot,
    route: Plan,
) -> Result<(Option<Vec<u8>>, u32), ConfigError> {
    let conflict = || ConfigError::new("conflict", "Claude 配置在路由切换后有外部修改，保留原文件");
    let saved = latest(journal, path).ok_or_else(conflict)?;
    let mut applied = config::parse_json(route.after.as_deref()).map_err(|_| conflict())?;
    let mut live = config::parse_json(current.bytes.as_deref()).map_err(|_| conflict())?;
    let applied_field = applied
        .as_object_mut()
        .expect("validated settings")
        .remove(FIELD);
    let live_field = live
        .as_object_mut()
        .expect("validated settings")
        .remove(FIELD);
    let old_digest = digest(applied_field.as_ref());
    let known_old = old_digest.is_none()
        || journal.transactions.iter().any(|transaction| {
            transaction.state == "applied"
                && Path::new(&transaction.before.path) == path
                && transaction
                    .claude_statusline
                    .as_ref()
                    .is_some_and(|change| change.digest == old_digest)
        });
    if !known_old
        || live != applied
        || digest(live_field.as_ref()) != saved.digest
        || live_field == applied_field
        || current.stamp.mode != route.after_mode
    {
        return Err(conflict());
    }
    let mut restored = config::parse_json(route.before.as_deref()).map_err(|_| conflict())?;
    let data = restored.as_object_mut().expect("validated settings");
    if let Some(field) = live_field {
        data.insert(FIELD.to_owned(), field);
    } else {
        data.remove(FIELD);
    }
    if route.before.is_none() && data.is_empty() {
        Ok((None, route.before_stamp.mode))
    } else {
        let mode = if route.before.is_none() {
            current.stamp.mode
        } else {
            route.before_stamp.mode
        };
        Ok((Some(settings(&restored)?), mode))
    }
}
