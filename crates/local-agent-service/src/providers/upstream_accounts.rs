use super::{ensure_write_enabled, error, unavailable};
use crate::{
    ApiResult, AppState,
    billing::routing_quota_status,
    observation::policy::{Cooldown, CooldownSnapshot},
};
use axum::{
    Json,
    extract::{Path, State},
};
use chrono::{DateTime, FixedOffset, Utc};
use local_agent_core::{
    Provider,
    model_services::policy::{access_scopes, scopes},
    providers::{
        ConfigError, Verification,
        upstream_accounts::{Account, AccountAction},
    },
};
use serde::Serialize;
use serde_json::json;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountView {
    #[serde(flatten)]
    account: Account,
    verifications: Option<Vec<ModelVerification>>,
    cooldowns: Option<Vec<ScopedCooldown>>,
    recoveries: Option<Vec<ScopedCooldown>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    routing_quota: Option<&'static str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelVerification {
    model: String,
    state: &'static str,
    checked_at: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScopedCooldown {
    model: Option<String>,
    #[serde(flatten)]
    status: Cooldown,
}

fn model_verifications(
    account: &Account,
    provider: Option<&Provider>,
    records: &[Verification],
) -> Vec<ModelVerification> {
    let now = Utc::now();
    let mut results: Vec<_> = records
        .iter()
        .filter(|record| record.provider_id == account.id)
        .filter_map(|record| {
            let model = record.model.clone()?;
            let (state, checked_at) = verification_state(
                record,
                provider.is_some_and(|p| account.matches_verification(p, record)),
                now,
            );
            Some((
                checked_at,
                ModelVerification {
                    model,
                    state,
                    checked_at: checked_at.map(|_| record.checked_at.clone()),
                },
            ))
        })
        .collect();
    results
        .sort_by(|(a_time, a), (b_time, b)| b_time.cmp(a_time).then_with(|| a.model.cmp(&b.model)));
    results.into_iter().map(|(_, record)| record).collect()
}

pub(crate) fn verification_state(
    record: &Verification,
    matches_target: bool,
    now: DateTime<Utc>,
) -> (&'static str, Option<DateTime<FixedOffset>>) {
    let checked_at = DateTime::parse_from_rfc3339(&record.checked_at).ok();
    let state = if !matches_target {
        "outdated"
    } else if let Some(time) = checked_at {
        let age = now.signed_duration_since(time);
        if age < chrono::Duration::zero() {
            "unknown"
        } else if record.state == "failed" {
            "failed"
        } else if record.state != "verified" {
            "unknown"
        } else if age >= chrono::Duration::minutes(15) {
            "expired"
        } else {
            "verified"
        }
    } else {
        "unknown"
    };
    (state, checked_at)
}

fn account_cooldowns(
    mut target: Provider,
    snapshot: &CooldownSnapshot,
    models: &[String],
) -> Vec<ScopedCooldown> {
    let mut results = Vec::new();
    if !snapshot.is_empty() {
        let (access_account, _) = access_scopes(&target);
        let (account_scope, _) = scopes(&target);
        if let Some(status) = snapshot.scoped(&account_scope, [&access_account, &account_scope]) {
            results.push(ScopedCooldown {
                model: None,
                status,
            });
        }
        for model in models {
            target.model.clone_from(model);
            let (_, access_model) = access_scopes(&target);
            let (_, model_scope) = scopes(&target);
            if let Some(status) = snapshot.scoped(&account_scope, [&access_model, &model_scope]) {
                results.push(ScopedCooldown {
                    model: Some(model.clone()),
                    status,
                });
            }
        }
    }
    results
}

pub(crate) async fn action(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
    Json(input): Json<AccountAction>,
) -> ApiResult {
    if let AccountAction::Verify {
        store_id,
        expected_revision,
        account,
        model,
    } = &input
    {
        let store_id = store_id.clone();
        let state_for_check = state.clone();
        crate::blocking::run(move || {
            let store = state_for_check
                .store
                .lock()
                .map_err(|_| error(unavailable()))?;
            ensure_write_enabled(&state_for_check).map_err(error)?;
            store.require_model_service_store(&store_id).map_err(error)
        })
        .await
        .map_err(|e| error(crate::blocking::config_error(e)))??;
        let Json(_) = super::verify(
            State(state.clone()),
            Path(provider_id.clone()),
            Json(super::Verify {
                expected_revision: *expected_revision,
                model: Some(model.clone()),
                upstream_account: Some(account.clone()),
            }),
        )
        .await?;
    }
    crate::blocking::run(move || {
        // An account can only refer to a Key already created in the vault.
        let key = if let AccountAction::Save { account, .. } = &input {
            if !local_agent_core::providers::operations::valid_id(&account.credential_ref) {
                return Err(error(ConfigError::new(
                    "invalid_credential",
                    "凭据引用无效",
                )));
            }
            let secret = state
                .credentials
                .get(&account.credential_ref)
                .map_err(error)?;
            if secret.is_empty()
                || std::str::from_utf8(&secret).is_ok_and(|key| account.label.contains(key))
            {
                return Err(error(ConfigError::new(
                    "invalid_credential",
                    "API Key 为空或出现在账号名称中",
                )));
            }
            Some(secret)
        } else {
            None
        };
        let records = state
            .providers
            .lock()
            .ok()
            .and_then(|manager| manager.verifications().ok());
        let store = state.store.lock().map_err(|_| error(unavailable()))?;
        let store_id = store.database_id().map_err(error)?;
        match input {
            AccountAction::List | AccountAction::Verify { .. } => {}
            AccountAction::Save {
                store_id: expected,
                account,
            } => {
                ensure_write_enabled(&state).map_err(error)?;
                if expected != store_id {
                    return Err(error(ConfigError::new(
                        "conflict",
                        "当前数据库已变化，请重新读取",
                    )));
                }
                let provider = store
                    .providers()
                    .map_err(|_| error(unavailable()))?
                    .into_iter()
                    .find(|p| p.id == provider_id)
                    .ok_or_else(|| error(ConfigError::new("not_found", "Provider 不存在")))?;
                local_agent_core::providers::subscription::validate_key(
                    &provider,
                    key.as_deref().map(Vec::as_slice),
                )
                .map_err(error)?;
                store
                    .save_upstream_account(&provider, &account)
                    .map_err(error)?;
            }
            AccountAction::SetState {
                store_id: expected,
                account,
                expected_state,
                state: target,
            } => {
                ensure_write_enabled(&state).map_err(error)?;
                if expected != store_id {
                    return Err(error(ConfigError::new(
                        "conflict",
                        "当前数据库已变化，请重新读取",
                    )));
                }
                store
                    .set_upstream_account_state(&provider_id, &account, &expected_state, &target)
                    .map_err(error)?;
            }
        }
        let accounts = store.upstream_accounts(&provider_id).map_err(error)?;
        if accounts.is_empty() {
            return Ok(Json(json!({"storeId":store_id,"accounts":accounts})));
        }
        let provider = store
            .provider_by_id(&provider_id)
            .map_err(|_| error(unavailable()))?;
        let snapshot = CooldownSnapshot::read(&state, &store);
        let billing = state.billing.lock().ok();
        let now = Utc::now();
        let models = provider
            .as_ref()
            .map_or_else(Vec::new, Provider::enabled_model_ids);
        let accounts: Vec<_> = accounts
            .into_iter()
            .map(|account| {
                let target = provider.as_ref().and_then(|provider| {
                    store
                        .upstream_account_target(provider, &account.reference())
                        .ok()
                        .map(|(target, _)| target)
                });
                let routing_quota = target.as_ref().and_then(|target| {
                    routing_quota_status(billing.as_deref(), &store, target, now)
                });
                let (cooldowns, recoveries) = target
                    .zip(snapshot.as_ref())
                    .map(|(target, snapshot)| {
                        let (cooling, recovering) = account_cooldowns(target, snapshot, &models)
                            .into_iter()
                            .partition(|entry| entry.status.state.is_none());
                        (Some(cooling), Some(recovering))
                    })
                    .unwrap_or_default();
                AccountView {
                    verifications: records
                        .as_ref()
                        .map(|records| model_verifications(&account, provider.as_ref(), records)),
                    cooldowns,
                    recoveries,
                    routing_quota,
                    account,
                }
            })
            .collect();
        Ok(Json(json!({"storeId":store_id,"accounts":accounts})))
    })
    .await
    .map_err(|e| error(crate::blocking::config_error(e)))?
}
