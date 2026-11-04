pub(crate) mod catalog_connection;
pub(crate) mod catalog_source;
pub(crate) mod credential_scope;
pub(crate) mod draft_catalog;
pub(crate) mod draft_grant;
pub(crate) mod draft_recovery;
mod installation;
pub(crate) mod operations;
pub(crate) mod presets;
mod process;
pub(crate) mod source_operation;

use self::{installation::Compatibility, process::output};
use crate::{ApiResult, AppState, json_value};
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use local_agent_core::{
    Provider,
    providers::{self, BindingDraft, ConfigError, Verification, files, fingerprint, select_model},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    path::{Path as FilePath, PathBuf},
    time::Duration,
};
use zeroize::Zeroizing;

pub(crate) fn error(e: ConfigError) -> (StatusCode, Json<Value>) {
    let policy = providers::error_policy(&e.code);
    let status = StatusCode::from_u16(policy.http_status).expect("bundled provider error status");
    let mut value = json!(e);
    if policy.outcome_unknown {
        value["outcomeUnknown"] = json!(true);
    }
    (status, Json(value))
}
fn unavailable() -> ConfigError {
    ConfigError::new("unavailable", "配置服务暂不可用")
}

#[cfg(test)]
mod error_contract_tests {
    use super::*;

    #[test]
    fn http_status_and_unknown_outcome_follow_the_shared_contract() {
        let contract: Value = serde_json::from_str(include_str!(
            "../../../resources/provider-error-contract.json"
        ))
        .unwrap();
        for (code, policy) in contract["errors"].as_object().unwrap() {
            let (status, Json(body)) = error(ConfigError::new(code, "fixture"));
            assert_eq!(
                status.as_u16() as u64,
                policy["httpStatus"].as_u64().unwrap(),
                "{code}"
            );
            assert_eq!(body["code"], *code);
            assert_eq!(
                body["outcomeUnknown"].as_bool().unwrap_or(false),
                policy["outcomeUnknown"].as_bool().unwrap(),
                "{code}"
            );
        }
        let (status, Json(body)) = error(ConfigError::new("future_error", "fixture"));
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(!body["outcomeUnknown"].as_bool().unwrap_or(false));
    }
}
pub async fn import_candidates(State(state): State<AppState>) -> ApiResult {
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| error(unavailable()))?;
        let saved = state
            .store
            .lock()
            .map_err(|_| error(unavailable()))?
            .providers()
            .map_err(crate::error)?;
        json_value(manager.discover_imports(&saved))
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}
pub async fn import_native(
    State(state): State<AppState>,
    Json(input): Json<providers::imports::ImportRequest>,
) -> ApiResult {
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| error(unavailable()))?;
        let store = state.store.lock().map_err(|_| error(unavailable()))?;
        json_value(manager.import_provider(&store, input).map_err(error)?)
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}
pub async fn import_protocol(State(state): State<AppState>) -> ApiResult {
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| error(unavailable()))?;
        let store = state.store.lock().map_err(|_| error(unavailable()))?;
        json_value(manager.native_import_protocol(&store).map_err(error)?)
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}
async fn import_result(
    state: AppState,
    input: providers::imports::ImportRequest,
    cancel: bool,
) -> ApiResult {
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| error(unavailable()))?;
        let store = state.store.lock().map_err(|_| error(unavailable()))?;
        json_value(
            manager
                .native_import_operation(&store, &input, cancel)
                .map_err(error)?,
        )
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}
pub async fn query_import(
    State(state): State<AppState>,
    Json(input): Json<providers::imports::ImportRequest>,
) -> ApiResult {
    import_result(state, input, false).await
}
pub async fn cancel_import(
    State(state): State<AppState>,
    Json(input): Json<providers::imports::ImportRequest>,
) -> ApiResult {
    import_result(state, input, true).await
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Runtime {
    agent_id: String,
    version: Option<String>,
    supported: bool,
    processes: Option<usize>,
    reason: String,
}
pub(crate) fn binary(agent: &str) -> Option<PathBuf> {
    installation::binary(agent)
}

const AGENTS: [&str; 4] = ["claude", "codex", "opencode", "claude-desktop"];

fn fixture_version(agent: &str) -> Option<&'static str> {
    #[cfg(debug_assertions)]
    if std::env::var("LOCAL_AGENT_TEST_MEMORY_VAULT").as_deref() == Ok("1")
        && std::env::var("LOCAL_AGENT_TEST_IDLE_AGENTS").as_deref() == Ok("1")
    {
        return match agent {
            "claude" => Some("2.1.286"),
            "codex" => Some("0.163.0"),
            "opencode" => Some("1.18.15"),
            "claude-desktop" => Some("2.2553.1"),
            _ => None,
        };
    }
    let _ = agent;
    None
}

fn compatibility(agent: &str) -> Compatibility {
    if let Some(version) = fixture_version(agent) {
        return Compatibility {
            version: Some(version.into()),
            result: Ok(()),
        };
    }
    installation::compatibility(agent)
}

fn runtimes() -> Vec<Runtime> {
    let fixture = fixture_version("claude").is_some();
    let (processes, node_commands) = if fixture {
        (Some(String::new()), Some(String::new()))
    } else {
        process_snapshot(true)
    };
    AGENTS
        .into_iter()
        .map(|agent| {
            let Compatibility { version, result } = compatibility(agent);
            Runtime {
                agent_id: agent.into(),
                version,
                supported: result.is_ok(),
                processes: processes
                    .as_deref()
                    .and_then(|text| process_count(agent, text, node_commands.as_deref())),
                reason: if fixture {
                    "隔离临时目录测试进程状态".into()
                } else {
                    result.err().map_or_else(String::new, |error| error.message)
                },
            }
        })
        .collect()
}

pub(crate) fn require_supported(agent: &str) -> Result<(), ConfigError> {
    compatibility(agent).result
}

fn agent_idle(agent: &str) -> bool {
    if fixture_version(agent).is_some() {
        return true;
    }
    let (processes, node_commands) = process_snapshot(agent == "claude");
    processes
        .as_deref()
        .and_then(|text| process_count(agent, text, node_commands.as_deref()))
        == Some(0)
}

fn process_snapshot(need_node_commands: bool) -> (Option<String>, Option<String>) {
    let uid = providers::current_uid().to_string();
    let processes = output(FilePath::new("/bin/ps"), &["-U", &uid, "-o", "pid=,comm="]);
    if !need_node_commands {
        return (processes, None);
    }
    let node_pids = processes
        .as_deref()
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let (pid, name) = line.trim().split_once(char::is_whitespace)?;
            (FilePath::new(name.trim()).file_name()?.to_str()? == "node").then_some(pid)
        })
        .collect::<Vec<_>>()
        .join(",");
    let node_commands = if node_pids.is_empty() {
        Some(String::new())
    } else {
        output(
            FilePath::new("/bin/ps"),
            &["-p", &node_pids, "-o", "pid=,args="],
        )
    };
    (processes, node_commands)
}
fn process_count(agent: &str, processes: &str, node_commands: Option<&str>) -> Option<usize> {
    let mut count = 0;
    for line in processes.lines() {
        let (pid, executable) = line.trim().split_once(char::is_whitespace)?;
        let executable = executable.trim();
        let base = std::path::Path::new(executable).file_name()?.to_str()?;
        if base == agent
            || agent == "codex" && ["Codex", "ChatGPT"].contains(&base)
            || agent == "opencode" && base == "OpenCode"
            || agent == "claude-desktop" && base == "Claude"
            || agent == "claude" && executable.contains("/claude/versions/")
        {
            count += 1;
        } else if agent == "claude" && base == "node" {
            // Read command lines only inside this process. Never return or persist their contents.
            let command = node_commands?.lines().find_map(|line| {
                let (id, args) = line.trim().split_once(char::is_whitespace)?;
                (id == pid).then_some(args.trim())
            })?;
            if command.is_empty() {
                return None;
            }
            if command.contains("@anthropic-ai/claude-code/")
                || command.contains("/claude-code/cli.js")
            {
                count += 1;
            }
        }
    }
    Some(count)
}

#[cfg(test)]
mod runtime_tests {
    use super::process_count;
    #[test]
    fn only_claude_hosts_count_and_unreadable_nodes_remain_unknown() {
        let processes = "10 /usr/bin/node\n11 /Users/test/.local/share/claude/versions/2.1.286\n12 /usr/bin/node\n13 /app/codex\n14 /app/claude\n";
        let commands = "10 node /project/node_modules/vite/bin/vite.js\n12 node /npm/@anthropic-ai/claude-code/cli.js\n";
        assert_eq!(process_count("claude", processes, Some(commands)), Some(3));
        assert_eq!(process_count("codex", processes, None), Some(1));
        assert_eq!(
            process_count(
                "codex",
                "10 /Applications/ChatGPT.app/Contents/MacOS/ChatGPT\n11 /app/codex\n12 /Applications/Codex.app/Contents/MacOS/Codex\n",
                None
            ),
            Some(3),
            "shared config writes must guard both desktop and CLI"
        );
        assert_eq!(process_count("claude", processes, None), None);
        assert_eq!(
            process_count("claude", processes, Some("10 node app.js")),
            None
        );
        assert_eq!(
            process_count(
                "claude",
                "10 /usr/bin/node",
                Some("10 node /project/server.js")
            ),
            Some(0)
        );
    }
}
pub async fn status(State(state): State<AppState>) -> ApiResult {
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| error(unavailable()))?;
        let mut value = serde_json::to_value(manager.status().map_err(error)?)
            .map_err(|_| error(unavailable()))?;
        let mut verifications = manager.verifications().map_err(error)?;
        {
            let store = state.store.lock().map_err(|_| error(unavailable()))?;
            value["localTokenProtocol"] = json!(manager.local_token_protocol(&store).map_err(error)?);
        }
        drop(manager);
        let observed = state.observation.dynamic_route_observations();
        {
            let store = state.store.lock().map_err(|_| error(unavailable()))?;
            let bindings = store.dynamic_bindings().map_err(error)?;
            let service_bindings: std::collections::HashSet<_> = store
                .model_service_binding_ids()
                .map_err(error)?
                .into_iter()
                .collect();
            value["dynamicBindings"] = json!(
                bindings
                    .iter()
                    .map(|b| {
                        let mut v = json!(b);
                        v["localEndpoint"] =
                            json!(crate::gateway::dynamic::configured_endpoint(&state, b));
                        if service_bindings.contains(&b.id) {
                            v["modelService"] = json!(true);
                        }
                        if let Some(row)=observed.iter().find(|row|row.routing.as_ref().is_some_and(|r|r.binding_id==b.id)) {
                            v["lastObserved"]=json!({"providerName":row.provider,"model":row.routed_model,"status":row.status,"routeVersionId":row.routing.as_ref().map(|r|&r.route_version_id),"startedAt":row.started_at});
                        }
                        v
                    })
                    .collect::<Vec<_>>()
            );
            value["dynamicChanges"] = json!(store.dynamic_changes().map_err(error)?);
            value["dynamicLeases"] = json!(store.dynamic_lease_summary().map_err(error)?);
        }
        let providers = state
            .store
            .lock()
            .map_err(|_| error(unavailable()))?
            .providers()
            .map_err(crate::error)?;
        for v in &mut verifications {
            if !providers.iter().any(|p| {
                p.id == v.provider_id
                    && select_model(p, v.model.as_deref())
                        .is_ok_and(|p| fingerprint(&p) == v.fingerprint)
            }) {
                v.state = "outdated".into();
            } else if v.state == "verified"
                && !chrono::DateTime::parse_from_rfc3339(&v.checked_at).is_ok_and(|date| {
                    chrono::Utc::now().signed_duration_since(date) < chrono::Duration::minutes(15)
                })
            {
                v.state = "expired".into();
            }
        }
        value["verifications"] = json!(verifications);
        value["runtimes"] = json!(runtimes());
        Ok(Json(value))
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Credential {
    label: String,
    secret: String,
    operation_id: Option<String>,
}
pub async fn credential(
    State(state): State<AppState>,
    input: Result<Json<Credential>, axum::extract::rejection::JsonRejection>,
) -> ApiResult {
    let Json(input) =
        input.map_err(|_| error(ConfigError::new("invalid_credential", "凭据请求格式无效")))?;
    let secret = Zeroizing::new(input.secret);
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| error(unavailable()))?;
        if let Some(id) = input.operation_id {
            json_value(
                manager
                    .create_credential_operation(&id, &input.label, secret)
                    .map_err(error)?,
            )
        } else {
            let scope = {
                let store = state.store.lock().map_err(|_| error(unavailable()))?;
                let id = store.database_id().map_err(error)?;
                store.credential_scope(&id).map_err(error)?
            };
            // Keep the original two-field response. A server-owned v3 intent
            // makes a lost reply discoverable without claiming idempotence.
            json_value(
                manager
                    .compatibility_credential(&scope, &input.label, secret)
                    .map_err(error)?,
            )
        }
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}
pub async fn credential_operation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult {
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| error(unavailable()))?;
        json_value(manager.credential_operation(&id).map_err(error)?)
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}
pub async fn cancel_credential_operation(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult {
    crate::blocking::run(move || {
        let manager = state.providers.lock().map_err(|_| error(unavailable()))?;
        json_value(manager.cancel_credential_operation(&id).map_err(error)?)
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}
fn provider(state: &AppState, id: &str) -> Result<Provider, (StatusCode, Json<Value>)> {
    state
        .store
        .lock()
        .map_err(|_| error(unavailable()))?
        .providers()
        .map_err(crate::error)?
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| error(ConfigError::new("not_found", "服务商不存在")))
}
pub async fn prepare(State(state): State<AppState>, Json(draft): Json<BindingDraft>) -> ApiResult {
    crate::blocking::run(move || {
        let m = state.providers.lock().map_err(|_| error(unavailable()))?;
        let store = state.store.lock().map_err(|_| error(unavailable()))?;
        if let Some(t) = crate::local_tokens::replay_draft(
            &m,
            &store,
            &draft,
            providers::local_tokens::Purpose::FixedRoute,
        )
        .map_err(error)?
        {
            return json_value(t);
        }
        if draft.provider_id.is_none() && draft.operation.is_some() {
            return Err(error(ConfigError::new(
                "invalid_local_token",
                "官方登录不创建本机令牌，请重新核对草稿",
            )));
        }
        let p = draft
            .provider_id
            .as_ref()
            .map(|id| {
                store
                    .providers()
                    .map_err(crate::error)?
                    .into_iter()
                    .find(|p| &p.id == id)
                    .ok_or_else(|| error(ConfigError::new("not_found", "服务商不存在")))
            })
            .transpose()?;
        if draft
            .expected_provider_revision
            .is_some_and(|revision| p.as_ref().is_none_or(|p| p.revision != revision))
        {
            return Err(error(ConfigError::new(
                "conflict",
                "供应商已改变，请重新选择连接",
            )));
        }
        require_supported(&draft.agent_id).map_err(error)?;
        if p.is_none() && draft.model.is_some() {
            return Err(error(ConfigError::new(
                "invalid_model",
                "官方原生登录的模型由 Agent 管理",
            )));
        }
        let selected = p
            .as_ref()
            .map(|p| select_model(p, draft.model.as_deref()).map_err(error))
            .transpose()?;
        if let Some(source) = &p {
            if ["opencode", "claude-desktop", "codex"].contains(&draft.agent_id.as_str()) {
                for model in source.enabled_model_ids() {
                    m.require_verification(&select_model(source, Some(&model)).map_err(error)?)
                        .map_err(error)?;
                }
            } else {
                m.require_verification(selected.as_ref().unwrap())
                    .map_err(error)?;
            }
        }
        // API configuration switches use an owned route for timing and Session
        // correlation even when the upstream already speaks the Agent protocol.
        match (p.as_ref(), selected.as_ref()) {
            (Some(source), Some(selected)) => json_value(
                crate::local_tokens::fixed(&state, &m, &store, draft, source, selected)
                    .map_err(error)?,
            ),
            _ => json_value(m.prepare_routed(draft, None, None).map_err(error)?),
        }
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}
pub async fn cancel(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    crate::blocking::run(move || {
        json_value(
            state
                .providers
                .lock()
                .map_err(|_| error(unavailable()))?
                .cancel(&id)
                .map_err(error)?,
        )
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}
pub async fn restore(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    crate::blocking::run(move || {
        json_value(
            state
                .providers
                .lock()
                .map_err(|_| error(unavailable()))?
                .prepare_restore(&id)
                .map_err(error)?,
        )
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}
pub async fn apply(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    switch(state, id, true).await
}
pub async fn check_switch(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    switch(state, id, false).await
}
pub async fn defer_switch(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    activation(state, id, "deferred").await
}
pub async fn launch_requested(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult {
    activation(state, id, "launch_requested").await
}
async fn activation(state: AppState, id: String, intent: &'static str) -> ApiResult {
    crate::blocking::run(move || {
        json_value(
            state
                .providers
                .lock()
                .map_err(|_| error(unavailable()))?
                .activation(&id, intent)
                .map_err(error)?,
        )
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}
async fn switch(state: AppState, id: String, commit: bool) -> ApiResult {
    crate::blocking::run(move || {
        let t = {
            let m = state.providers.lock().map_err(|_| error(unavailable()))?;
            m.status()
                .map_err(error)?
                .transactions
                .into_iter()
                .find(|t| t.id == id)
                .ok_or_else(|| error(ConfigError::new("not_found", "找不到配置事务")))?
        };
        let current = t
            .provider
            .as_ref()
            .map(|p| provider(&state, &p.id))
            .transpose()?;
        if commit && t.state == "applied" {
            return json_value(t);
        }
        if t.restore_of.is_none() {
            require_supported(&t.agent_id).map_err(error)?;
        }
        let m = state.providers.lock().map_err(|_| error(unavailable()))?;
        if t.state == "pending"
            && t.restore_of.is_none()
            && let Some(p) = &current
        {
            let selected = select_model(p, t.model.as_deref()).map_err(error)?;
            if t.routing
                .as_ref()
                .is_some_and(|route| !route.models.is_empty())
            {
                for model in p.enabled_model_ids() {
                    m.require_verification(&select_model(p, Some(&model)).map_err(error)?)
                        .map_err(error)?;
                }
            } else {
                m.require_verification(&selected).map_err(error)?;
            }
            if commit && let Some(route) = &t.routing {
                let binding = state
                    .store
                    .lock()
                    .map_err(|_| error(unavailable()))?
                    .dynamic_binding(&route.id)
                    .map_err(error)?;
                if let Some(b) = binding {
                    let store = state.store.lock().map_err(|_| error(unavailable()))?;
                    crate::model_services::ensure_enrollment(&store, &m, &b.id).map_err(error)?;
                    crate::gateway::dynamic::ensure(&b, route, &selected, &t.before.path)
                        .map_err(error)?;
                } else {
                    state.gateway.ensure(route, &selected).map_err(error)?;
                }
            }
        }
        // Re-check the full provider fingerprint while holding the store lock through the commit.
        let store = state.store.lock().map_err(|_| error(unavailable()))?;
        if !commit {
            return json_value(m.check_switch(&id, current.as_ref()).map_err(error)?);
        }
        json_value(
            m.apply_with_provider(
                &id,
                || {
                    t.provider
                        .as_ref()
                        .map(|old| {
                            store
                                .providers()
                                .map_err(|_| unavailable())?
                                .into_iter()
                                .find(|p| p.id == old.id)
                                .ok_or_else(|| ConfigError::new("conflict", "服务商已删除"))
                        })
                        .transpose()
                },
                agent_idle(&t.agent_id)
                    && state
                        .observation
                        .active_count(if t.agent_id == "claude-desktop" {
                            "claude"
                        } else {
                            &t.agent_id
                        })
                        == 0,
            )
            .map_err(error)?,
        )
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Verify {
    pub(crate) expected_revision: u64,
    #[serde(default)]
    pub(crate) model: Option<String>,
}
pub async fn verify(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<Verify>,
) -> ApiResult {
    let _permit = state
        .verification_capacity
        .clone()
        .try_acquire_owned()
        .map_err(|_| error(ConfigError::new("busy", "已有连接验证正在进行，请稍后重试")))?;
    let p = {
        let state = state.clone();
        let id = id.clone();
        crate::blocking::run(move || {
            let p = provider(&state, &id)?;
            if p.revision != input.expected_revision {
                return Err(error(ConfigError::new(
                    "conflict",
                    "服务商版本已改变，请重新读取",
                )));
            }
            let p = select_model(&p, input.model.as_deref()).map_err(error)?;
            providers::validate_source(&p).map_err(error)?;
            Ok(p)
        })
        .await
        .map_err(|e| error(crate::blocking::config_error(e)))??
    };
    let secret = if let Some(reference) = &p.credential_ref {
        Some(
            crate::blocking::secret(state.credentials.clone(), reference.clone())
                .await
                .map_err(error)?,
        )
    } else {
        None
    };
    let result = probe_with(&state.provider_adapters, &state.provider_client, &p, secret).await;
    let now = chrono::Utc::now().to_rfc3339();
    let v = Verification {
        provider_id: id.clone(),
        model: Some(p.model.clone()),
        fingerprint: fingerprint(&p),
        state: if result.is_ok() { "verified" } else { "failed" }.into(),
        checked_at: now,
        message: result.unwrap_or_else(|e| e.message),
    };
    crate::blocking::run(move || {
        let current = select_model(&provider(&state, &id)?, v.model.as_deref()).map_err(error)?;
        if fingerprint(&current) != v.fingerprint {
            return Err(error(ConfigError::new(
                "conflict",
                "验证期间服务商已改变，未保存旧验证结果",
            )));
        }
        state
            .providers
            .lock()
            .map_err(|_| error(unavailable()))?
            .save_verification(v.clone())
            .map_err(error)?;
        json_value(v)
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}
#[cfg(test)]
pub(crate) async fn probe(
    client: &reqwest::Client,
    p: &Provider,
    secret: Option<Zeroizing<Vec<u8>>>,
) -> Result<String, ConfigError> {
    probe_with(
        &crate::provider_models::adapters::builtin(),
        client,
        p,
        secret,
    )
    .await
}
async fn probe_with(
    adapters: &crate::provider_models::adapters::Adapters,
    client: &reqwest::Client,
    p: &Provider,
    secret: Option<Zeroizing<Vec<u8>>>,
) -> Result<String, ConfigError> {
    probe_detailed_with(adapters, client, p, secret)
        .await
        .map_err(|e| {
            if e.code.starts_with("probe_") {
                ConfigError::new("verification_failed", &e.message)
            } else {
                e
            }
        })
}
#[cfg(test)]
pub(crate) async fn probe_detailed(
    client: &reqwest::Client,
    p: &Provider,
    secret: Option<Zeroizing<Vec<u8>>>,
) -> Result<String, ConfigError> {
    probe_detailed_with(
        &crate::provider_models::adapters::builtin(),
        client,
        p,
        secret,
    )
    .await
}
pub(crate) async fn probe_detailed_with(
    adapters: &crate::provider_models::adapters::Adapters,
    client: &reqwest::Client,
    p: &Provider,
    secret: Option<Zeroizing<Vec<u8>>>,
) -> Result<String, ConfigError> {
    let wire = crate::gateway::capabilities::builtin()
        .resolve(&p.protocol)
        .map_err(|message| ConfigError::new("incompatible", message))?;
    let adapter = adapters
        .resolve(&p.protocol)
        .map_err(|e| ConfigError::new("incompatible", &e.message))?;
    if adapter.verification_protocol() != wire.id() {
        return Err(ConfigError::new(
            "incompatible",
            "Provider 验证规则与所选协议不匹配",
        ));
    }
    let body = wire.verification_body(&p.model);
    let mut request = client
        .post(wire.endpoint(&p.endpoint))
        .timeout(Duration::from_secs(6))
        .json(&body);
    if let Some(key) = secret.as_deref() {
        let key = std::str::from_utf8(key)
            .map_err(|_| ConfigError::new("invalid_credential", "凭据不是有效的 API Key"))?;
        request = request.bearer_auth(key);
        if wire.api_key_header() {
            request = request.header("x-api-key", key);
        }
    }
    for (name, value) in wire.headers() {
        request = request.header(*name, *value);
    }
    let mut response = request
        .send()
        .await
        .map_err(|_| ConfigError::new("probe_network", "网络、证书或连接超时；未保存成功验证"))?;
    adapter
        .ensure_active()
        .map_err(|e| ConfigError::new("incompatible", &e.message))?;
    let status = response.status();
    if !status.is_success() {
        return Err(ConfigError::new(
            match status.as_u16() {
                401 | 403 => "probe_auth",
                404 | 405 => "probe_path",
                429 => "probe_rate_limit",
                300..=399 => "probe_redirect",
                500..=599 => "probe_upstream",
                _ => "probe_rejected",
            },
            match status.as_u16() {
                401 | 403 => "鉴权被拒绝，请检查凭据与模型权限",
                404 | 405 => "目标不支持该协议路径，请检查端点",
                429 => "服务商限流或额度不足，本次未验证成功",
                300..=399 => "目标要求重定向；为保护凭据，未自动跟随",
                _ => "服务商拒绝验证请求；未保存成功状态",
            },
        ));
    }
    let mut bytes = Zeroizing::new(vec![]);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| ConfigError::new("probe_network", "验证响应中断"))?
    {
        adapter
            .ensure_active()
            .map_err(|e| ConfigError::new("incompatible", &e.message))?;
        if bytes.len() + chunk.len() > files::LIMIT as usize {
            return Err(ConfigError::new("probe_shape", "验证响应超过 1 MiB"));
        }
        bytes.extend_from_slice(&chunk);
    }
    let body: Value = serde_json::from_slice(&bytes)
        .map_err(|_| ConfigError::new("probe_shape", "响应不是该协议的 JSON 结果"))?;
    let valid = wire.verification_valid(&body);
    adapter
        .ensure_active()
        .map_err(|e| ConfigError::new("incompatible", &e.message))?;
    if !valid {
        return Err(ConfigError::new(
            "probe_shape",
            "响应结构不匹配，不能确认所选协议与模型可调用",
        ));
    }
    Ok("所选协议与模型的短请求验证成功；未验证完整工具循环、MCP 或所有模型能力".into())
}

pub(crate) async fn save_provider(
    State(state): State<AppState>,
    Json(provider): Json<Provider>,
) -> ApiResult {
    crate::blocking::run(move || {
        let manager = state
            .providers
            .lock()
            .map_err(|_| crate::error("配置服务暂不可用".into()))?;
        let _guard = manager
            .provider_edit_guard()
            .map_err(|e| crate::error(e.message))?;
        state
            .store
            .lock()
            .map_err(|_| crate::error("数据服务暂不可用".into()))?
            .save_provider(provider)
            .map_err(crate::error)?;
        Ok(Json(serde_json::json!({"saved":true})))
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}
