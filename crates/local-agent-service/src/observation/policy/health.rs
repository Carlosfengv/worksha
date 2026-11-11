use super::RECOVERY_PROBE_PREFIX;
use crate::AppState;
use local_agent_core::{
    Provider, Store,
    model_services::policy::{access_scopes, scopes},
    providers::{ConfigError, upstream_accounts::AccountRef},
};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum RecoveryState {
    WaitingForProbe,
    Probing,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Cooldown {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<RecoveryState>,
    until_ms: i64,
    reason: String,
}

impl Cooldown {
    fn order(&self) -> (u8, i64) {
        let rank = match self.state {
            Some(RecoveryState::WaitingForProbe) => 0,
            Some(RecoveryState::Probing) => 1,
            None => 2,
        };
        (rank, self.until_ms)
    }
}

pub(crate) struct CooldownSnapshot {
    rows: BTreeMap<String, (i64, String)>,
    probing: HashSet<String>,
    now: i64,
}

impl CooldownSnapshot {
    /// The caller holds the store lock, as does admission of a recovery probe.
    pub(crate) fn read(state: &AppState, store: &Store) -> Option<Self> {
        let rows = store.service_cooldowns().ok()?;
        let turns = state.observation.policy.turns.lock().ok()?;
        Some(Self {
            rows,
            probing: turns
                .iter()
                .filter_map(|key| key.strip_prefix(RECOVERY_PROBE_PREFIX).map(str::to_owned))
                .collect(),
            now: chrono::Utc::now().timestamp_millis(),
        })
    }

    pub(crate) fn scoped<const N: usize>(
        &self,
        account_scope: &str,
        keys: [&str; N],
    ) -> Option<Cooldown> {
        let (until_ms, reason) = keys
            .into_iter()
            .filter_map(|key| self.rows.get(key))
            // An active restriction takes precedence over an expired marker.
            .min_by_key(|(until_ms, _)| *until_ms <= self.now)?;
        let state = (*until_ms <= self.now).then(|| {
            if self.probing.contains(account_scope) {
                RecoveryState::Probing
            } else {
                RecoveryState::WaitingForProbe
            }
        });
        Some(Cooldown {
            state,
            until_ms: *until_ms,
            reason: reason.clone(),
        })
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub(crate) fn target(&self, provider: &Provider) -> Option<Cooldown> {
        let (account, model) = scopes(provider);
        let (access_account, access_model) = access_scopes(provider);
        self.scoped(&account, [&access_account, &access_model, &account, &model])
    }

    pub(crate) fn pool(
        &self,
        store: &Store,
        provider: &Provider,
        accounts: &[AccountRef],
    ) -> Value {
        summarize_pool(store, provider, accounts, |target| Ok(self.target(target)))
    }
}

pub(crate) fn unknown_health() -> Value {
    json!({"state":"unknown","untilMs":null})
}

fn ready_health() -> Value {
    json!({"state":"ready","untilMs":null})
}

fn cooldown_health(cooldown: Cooldown) -> Value {
    let state = cooldown
        .state
        .map_or_else(|| json!("cooling"), |state| json!(state));
    json!({"state":state,"untilMs":cooldown.until_ms,"reason":cooldown.reason})
}

// External policy plugins receive the existing availability categories.
pub(super) fn pool_health(
    store: &Store,
    provider: &Provider,
    accounts: &[AccountRef],
    include_access: bool,
) -> Value {
    summarize_pool(store, provider, accounts, |target| {
        let (account, model) = scopes(target);
        let mut cooldown = None;
        if include_access {
            let (account, model) = access_scopes(target);
            cooldown = store
                .service_cooldown(&account)?
                .or(store.service_cooldown(&model)?);
        }
        if cooldown.is_none() {
            cooldown = store
                .service_cooldown(&account)?
                .or(store.service_cooldown(&model)?);
        }
        Ok(cooldown.map(|(until_ms, reason)| Cooldown {
            state: None,
            until_ms,
            reason,
        }))
    })
}

fn summarize_pool(
    store: &Store,
    provider: &Provider,
    accounts: &[AccountRef],
    status: impl Fn(&Provider) -> Result<Option<Cooldown>, ConfigError>,
) -> Value {
    if accounts.is_empty() {
        return match status(provider) {
            Ok(None) => ready_health(),
            Ok(Some(cooldown)) => cooldown_health(cooldown),
            Err(_) => unknown_health(),
        };
    }
    let mut best: Option<Cooldown> = None;
    let mut unknown = false;
    for account in accounts {
        match store.upstream_account_target(provider, account) {
            Ok((target, _)) => match status(&target) {
                Ok(None) => return ready_health(),
                Ok(Some(cooldown)) => {
                    if best
                        .as_ref()
                        .is_none_or(|current| cooldown.order() < current.order())
                    {
                        best = Some(cooldown);
                    }
                }
                Err(_) => unknown = true,
            },
            Err(error) if error.code == "account_unavailable" => {}
            Err(_) => unknown = true,
        }
    }
    if unknown {
        return unknown_health();
    }
    best.map(cooldown_health)
        .unwrap_or_else(|| json!({"state":"unknown","untilMs":null,"reason":"account_unavailable"}))
}
