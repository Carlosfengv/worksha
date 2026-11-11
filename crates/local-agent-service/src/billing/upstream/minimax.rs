use super::{Observation, UpstreamBudget, api};
use local_agent_core::{
    Provider, Store,
    billing::AccountSnapshot,
    model_services::policy::{QuotaProbeSource, scopes},
    providers::{ConfigError, subscription::MINIMAX_PLAN},
};

pub(crate) fn supported(target: &Provider) -> bool {
    api::adapter(target).is_some_and(|(kind, _)| kind == MINIMAX_PLAN)
}

pub(in crate::billing) fn record(
    store: &Store,
    target: &Provider,
    snapshot: &AccountSnapshot,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), ConfigError> {
    let Some(observed) = Observation::from_snapshot(snapshot)
        .filter(|observed| observed.at <= now && now <= observed.expires)
    else {
        return Err(ConfigError::new(
            "stale_quota",
            "MiniMax 额度已过期，请刷新",
        ));
    };
    store.record_minimax_allowance(&scopes(target).0, &snapshot.windows, observed.at)
}

pub(super) fn budget(store: &Store, target: &Provider, now: i64) -> UpstreamBudget {
    let account = scopes(target).0;
    let Ok(windows) = store.minimax_exhausted_windows(&account) else {
        return UpstreamBudget::unavailable();
    };
    let mut budget = UpstreamBudget {
        source: QuotaProbeSource::MiniMax,
        track_recovery: !windows.is_empty(),
        ..UpstreamBudget::default()
    };
    for (id, reset_at) in windows {
        if let Some(reset_at) = reset_at.filter(|&at| at <= now) {
            budget.layers.push((
                format!("{account}:{id}"),
                Some(super::BudgetReset {
                    reset_at,
                    valid_until: now.saturating_add(300),
                }),
            ));
        } else {
            budget.exhausted = true;
        }
    }
    budget
}
