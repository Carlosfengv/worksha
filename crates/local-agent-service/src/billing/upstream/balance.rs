use super::{BudgetReset, Observation, UpstreamBudget, api};
use local_agent_core::{
    Provider, Store,
    billing::AccountSnapshot,
    model_services::policy::{BalanceState, QuotaProbeSource, scopes},
    providers::ConfigError,
};

#[derive(Clone, Copy)]
pub(crate) enum BalanceProvider {
    DeepSeek,
    Kimi,
}

pub(crate) fn target(provider: &Provider) -> Option<(BalanceProvider, String)> {
    provider.credential_ref.as_ref()?;
    let kind = match api::adapter(provider)?.0 {
        "deepseek" => BalanceProvider::DeepSeek,
        "kimi-cn" | "kimi-global" => BalanceProvider::Kimi,
        _ => return None,
    };
    Some((kind, scopes(provider).0))
}

pub(in crate::billing) fn record(
    store: &Store,
    provider: &Provider,
    snapshot: &AccountSnapshot,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), ConfigError> {
    let Some(available) = snapshot.inference_available else {
        return Ok(());
    };
    let Some((_, credential)) = target(provider) else {
        return Err(ConfigError::new("quota_scope_changed", "余额凭据已变更"));
    };
    let Some(observed) = Observation::from_snapshot(snapshot)
        .filter(|observed| observed.at <= now && now <= observed.expires)
    else {
        return Err(ConfigError::new("stale_quota", "余额状态已过期，请刷新"));
    };
    store.record_upstream_balance(&credential, observed.at.to_utc(), available)
}

pub(super) fn budget(store: &Store, credential: &str, now: i64) -> UpstreamBudget {
    match store.upstream_balance_state(credential) {
        Ok(BalanceState::Open) => UpstreamBudget::default(),
        Ok(BalanceState::Exhausted) => UpstreamBudget {
            exhausted: true,
            ..UpstreamBudget::default()
        },
        Ok(BalanceState::Recovery(observed_at)) => UpstreamBudget {
            source: QuotaProbeSource::Balance,
            track_recovery: true,
            // Balance has no recurring reset. The first positive observation
            // identifies this recovery attempt across refreshes and restarts.
            key_reset: Some(BudgetReset {
                reset_at: observed_at,
                valid_until: now.saturating_add(300),
            }),
            ..UpstreamBudget::default()
        },
        Err(_) => UpstreamBudget::unavailable(),
    }
}
