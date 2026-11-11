use super::*;
use local_agent_core::{
    Store,
    billing::{OPENROUTER_KEY_BUDGET, OpenRouterBudgetLayer, QuotaWindow},
    model_services::policy::{AccountQuotaHint, QuotaProbeSource},
    providers::{ConfigError, subscription::MINIMAX_PLAN, upstream_accounts::AccountRef},
};
pub(super) mod balance;
mod evidence;
pub(super) mod minimax;

#[derive(Default)]
pub(crate) struct UpstreamBudget {
    pub unavailable: bool,
    pub exhausted: bool,
    pub track_recovery: bool,
    pub source: QuotaProbeSource,
    pub key_reset: Option<BudgetReset>,
    layers: Vec<(String, Option<BudgetReset>)>,
}

impl UpstreamBudget {
    pub(crate) fn unavailable() -> Self {
        Self {
            unavailable: true,
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(crate) fn with_key_reset(reset: BudgetReset) -> Self {
        Self {
            key_reset: Some(reset),
            track_recovery: true,
            ..Self::default()
        }
    }

    pub(crate) fn scopes<'a>(
        &'a self,
        account: &'a str,
    ) -> impl Iterator<Item = (&'a str, Option<&'a BudgetReset>)> {
        std::iter::once((account, self.key_reset.as_ref())).chain(
            self.layers
                .iter()
                .map(|(scope, reset)| (scope.as_str(), reset.as_ref())),
        )
    }
}

pub(crate) struct BudgetReset {
    pub reset_at: i64,
    pub valid_until: i64,
}

pub(super) struct SharedBudget {
    window: QuotaWindow,
    epoch: u64,
    observation: Observation,
    retain_until: Option<i64>,
}

pub(super) struct OpenRouterAdmission {
    epoch: u64,
    observed: Observation,
    key: Option<(QuotaWindow, Observation)>,
    layers: Vec<(OpenRouterBudgetLayer, Observation)>,
}

pub(crate) fn route_budget_supported(target: &Provider) -> bool {
    api::adapter(target).is_some_and(|(kind, _)| {
        matches!(
            kind,
            "openrouter" | MINIMAX_PLAN | "deepseek" | "kimi-cn" | "kimi-global"
        )
    })
}

pub(crate) fn routing_quota_status(
    billing: Option<&Billing>,
    store: &Store,
    target: &Provider,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<&'static str> {
    if target.credential_ref.is_none() || !route_budget_supported(target) {
        return None;
    }
    billing
        .and_then(|billing| {
            crate::observation::policy::quota_admission(
                store,
                target,
                &billing.route_budget(store, target, now),
            )
        })
        .map_or(Some("unknown"), |admission| admission.display_key())
}

pub(crate) fn openrouter_scope(target: &Provider) -> Option<String> {
    if target.credential_ref.is_none()
        || api::adapter(target).map(|(kind, _)| kind) != Some("openrouter")
    {
        return None;
    }
    Some(local_agent_core::model_services::policy::scopes(target).0)
}

#[derive(Clone, Copy)]
struct Observation {
    at: chrono::DateTime<chrono::FixedOffset>,
    expires: chrono::DateTime<chrono::FixedOffset>,
}

impl Observation {
    fn from_snapshot(snapshot: &AccountSnapshot) -> Option<Self> {
        let at = chrono::DateTime::parse_from_rfc3339(&snapshot.as_of).ok()?;
        let valid = chrono::Duration::try_seconds(i64::try_from(snapshot.valid_for_seconds).ok()?)?;
        Some(Self {
            at,
            expires: at.checked_add_signed(valid)?,
        })
    }

    fn confirmed_exhaustion(self, window: &QuotaWindow) -> bool {
        window.remaining_percent == Some(0.0)
            && window
                .resets_at
                .is_none_or(|reset| reset > self.at.timestamp())
    }
}

fn key_window(snapshot: &AccountSnapshot) -> Option<&QuotaWindow> {
    snapshot
        .windows
        .iter()
        .find(|window| window.id == OPENROUTER_KEY_BUDGET)
}

fn reset(
    window: &QuotaWindow,
    observation: Observation,
    now: i64,
) -> Result<Option<BudgetReset>, ()> {
    if !observation.confirmed_exhaustion(window) {
        return Ok(None);
    }
    let Some(reset) = window.resets_at else {
        return Err(());
    };
    if now < reset {
        return Err(());
    }
    // The first eligible request may arrive long after the reset. Give its
    // recovery record a retention window starting at the actual probe time.
    let probe_window = (observation.expires - observation.at).num_seconds();
    Ok(Some(BudgetReset {
        reset_at: reset,
        valid_until: now.saturating_add(probe_window),
    }))
}

impl Billing {
    pub(crate) fn route_budget(
        &self,
        store: &Store,
        target: &Provider,
        now: chrono::DateTime<chrono::Utc>,
    ) -> UpstreamBudget {
        if minimax::supported(target) {
            return minimax::budget(store, target, now.timestamp());
        }
        if let Some((_, credential)) = balance::target(target) {
            return balance::budget(store, &credential, now.timestamp());
        }
        let mut budget = self.upstream_budget(target, now);
        let Some(credential) = openrouter_scope(target) else {
            return budget;
        };
        let Ok(holds) = store.openrouter_reply_holds(&credential) else {
            return UpstreamBudget::unavailable();
        };
        let now = now.timestamp();
        for (scope, reset) in holds {
            budget.track_recovery = true;
            if let Some(reset_at) = reset.filter(|&reset| reset <= now) {
                budget.layers.push((
                    format!("{credential}:openrouter-reply:{scope}"),
                    Some(BudgetReset {
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

    pub(super) fn cache_snapshot(
        &mut self,
        id: String,
        revision: String,
        source_epoch: Option<u64>,
        target: Option<&Provider>,
        snapshot: AccountSnapshot,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), &'static str> {
        if let (Some(scope), Some(epoch)) = (target.and_then(openrouter_scope), source_epoch) {
            self.record_openrouter_budget(scope, epoch, &snapshot, now)?;
        }
        let current_read_scope = target.and_then(|target| {
            let (kind, _) = api::adapter(target)?;
            read_scope(target, kind)
        });
        self.cache.insert(
            id,
            Cached {
                revision,
                source_epoch,
                current_read_scope,
                snapshot,
                error: None,
            },
        );
        Ok(())
    }

    fn record_openrouter_budget(
        &mut self,
        scope: String,
        epoch: u64,
        snapshot: &AccountSnapshot,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), &'static str> {
        let Some(observed) = Observation::from_snapshot(snapshot)
            .filter(|observed| observed.at <= now && now <= observed.expires)
        else {
            return Err("OpenRouter 额度已过期，请刷新");
        };
        self.openrouter_admission.retain(|_, saved| {
            saved.epoch == epoch
                && (saved.observed.expires >= now
                    || saved.key.is_some()
                    || !saved.layers.is_empty())
        });
        let previous = self
            .openrouter_admission
            .get(&scope)
            .filter(|entry| entry.epoch == epoch);
        if previous.is_some_and(|entry| entry.observed.at > observed.at) {
            return Ok(());
        }
        let coverage = snapshot.openrouter_budget_coverage;
        let incoming_key = key_window(snapshot);
        let mut key = incoming_key
            .filter(|window| observed.confirmed_exhaustion(window))
            .map(|window| (window.clone(), observed));
        let mut layers: Vec<_> = snapshot
            .openrouter_budget_layers
            .iter()
            .filter(|layer| layer.shared || observed.confirmed_exhaustion(&layer.window))
            .map(|layer| (layer.clone(), observed))
            .collect();
        if let Some(previous) = previous {
            if incoming_key.is_none() && !coverage.is_some_and(|c| c.key_unlimited) {
                key = previous.key.clone();
            }
            if !coverage.is_some_and(|c| c.layers_complete) {
                layers.extend(
                    previous
                        .layers
                        .iter()
                        .filter(|(prior, _)| {
                            !snapshot
                                .openrouter_budget_layers
                                .iter()
                                .any(|layer| layer.id == prior.id)
                        })
                        .cloned(),
                );
            }
        }
        // A partial response cannot erase an earlier confirmed hold.
        if layers.len() > 64 {
            return Err("额度响应缺少完整预算清单，请稍后刷新");
        }
        self.update_shared_budgets(snapshot, epoch, now);
        self.openrouter_admission.insert(
            scope,
            OpenRouterAdmission {
                epoch,
                observed,
                key,
                layers,
            },
        );
        Ok(())
    }

    fn update_shared_budgets(
        &mut self,
        snapshot: &AccountSnapshot,
        epoch: u64,
        now: chrono::DateTime<chrono::Utc>,
    ) {
        if !snapshot
            .openrouter_budget_layers
            .iter()
            .any(|layer| layer.shared)
        {
            return;
        }
        let Some(observation) = Observation::from_snapshot(snapshot) else {
            return;
        };
        if observation.at > now || observation.expires < now {
            return;
        }
        self.shared_budgets.retain(|_, value| {
            value.epoch == epoch
                && value
                    .retain_until
                    .is_none_or(|until| until >= now.timestamp())
        });
        for layer in snapshot
            .openrouter_budget_layers
            .iter()
            .filter(|layer| layer.shared)
        {
            let prior = self.shared_budgets.get(&layer.id);
            if prior.map_or(self.shared_budgets.len() >= 4096, |prior| {
                prior.observation.at > observation.at
            }) {
                continue;
            }
            // A confirmed exhaustion still needs its first probe after the
            // amount expires. Its replacement must outlive older copies too.
            let retain_until = (!observation.confirmed_exhaustion(&layer.window))
                .then_some(observation.expires.timestamp());
            let retain_until = prior.map_or(retain_until, |prior| {
                prior
                    .retain_until
                    .zip(retain_until)
                    .map(|(old, new)| old.max(new))
            });
            self.shared_budgets.insert(
                layer.id.clone(),
                SharedBudget {
                    window: layer.window.clone(),
                    epoch,
                    observation,
                    retain_until,
                },
            );
        }
    }

    fn upstream_snapshot(
        &self,
        target: &Provider,
        reference: Option<&AccountRef>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Option<(&'static str, &Cached, Observation)> {
        let (kind, _) = api::adapter(target)?;
        let (id, revision) = match reference {
            Some(reference) => (
                format!("upstream-{}", reference.id),
                source_revision(target, reference),
            ),
            None => (format!("provider-{}", target.id), fingerprint(target)),
        };
        let cached = self.cache.get(&id)?;
        let lease = self.sources.lease(kind).ok()?;
        if cached.revision != revision || cached.source_epoch != Some(lease.epoch) {
            return None;
        }
        let observation = Observation::from_snapshot(&cached.snapshot)?;
        if now < observation.at {
            return None;
        }
        Some((kind, cached, observation))
    }

    fn layer_observation<'a>(
        &'a self,
        layer: &'a OpenRouterBudgetLayer,
        epoch: Option<u64>,
        observation: Observation,
        now: chrono::DateTime<chrono::Utc>,
    ) -> (&'a OpenRouterBudgetLayer, &'a QuotaWindow, Observation) {
        if layer.shared
            && let Some(latest) = self.shared_budgets.get(&layer.id)
            && Some(latest.epoch) == epoch
            && latest.observation.at >= observation.at
            && latest.observation.at <= now
        {
            return (layer, &latest.window, latest.observation);
        }
        (layer, &layer.window, observation)
    }

    pub(crate) fn upstream_quota(
        &self,
        target: &Provider,
        reference: Option<&AccountRef>,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Option<AccountQuotaHint> {
        let (kind, cached, observation) = self.upstream_snapshot(target, reference, now)?;
        if cached.current_read_scope.is_none()
            || cached.error.is_some()
            || now > observation.expires
        {
            return None;
        }
        match kind {
            "openrouter" => {
                if openrouter_scope(target)
                    .and_then(|scope| self.openrouter_admission.get(&scope))
                    .is_some_and(|latest| {
                        Some(latest.epoch) == cached.source_epoch
                            && latest.observed.at > observation.at
                    })
                {
                    return None;
                }
                let key = key_window(&cached.snapshot);
                let layers = cached
                    .snapshot
                    .openrouter_budget_layers
                    .iter()
                    .map(|layer| {
                        self.layer_observation(layer, cached.source_epoch, observation, now)
                    })
                    .filter(|(_, _, observation)| now <= observation.expires)
                    .map(|(_, window, _)| window);
                AccountQuotaHint::from_hard_windows(key.into_iter().chain(layers), now.timestamp())
            }
            MINIMAX_PLAN => {
                AccountQuotaHint::from_minimax_windows(&cached.snapshot.windows, now.timestamp())
                    // Durable per-window evidence is shared by aliases of a credential.
                    // A cached zero from an older alias cannot override that decision.
                    .filter(|hint| !hint.exhausted())
            }
            _ => None,
        }
    }

    pub(crate) fn upstream_budget(
        &self,
        target: &Provider,
        now: chrono::DateTime<chrono::Utc>,
    ) -> UpstreamBudget {
        let Some(account) = openrouter_scope(target) else {
            return UpstreamBudget::default();
        };
        let Ok(lease) = self.sources.lease("openrouter") else {
            return UpstreamBudget::default();
        };
        if self.unavailable_budget_epoch == Some(lease.epoch) {
            return UpstreamBudget::unavailable();
        }
        let mut budget = UpstreamBudget {
            track_recovery: true,
            ..UpstreamBudget::default()
        };
        let Some(saved) = self
            .openrouter_admission
            .get(&account)
            .filter(|saved| saved.epoch == lease.epoch && saved.observed.at <= now)
        else {
            return budget;
        };
        if let Some((window, observation)) = &saved.key {
            match reset(window, *observation, now.timestamp()) {
                Ok(reset) => budget.key_reset = reset,
                Err(()) => budget.exhausted = true,
            }
        }
        for (layer, observed) in &saved.layers {
            let (layer, window, observation) =
                self.layer_observation(layer, Some(saved.epoch), *observed, now);
            let scope = if layer.shared {
                format!("openrouter-budget:{}", layer.id)
            } else {
                format!("{account}:openrouter-budget:{}", layer.id)
            };
            match reset(window, observation, now.timestamp()) {
                Ok(reset) => budget.layers.push((scope, reset)),
                Err(()) => budget.exhausted = true,
            }
        }
        budget
    }

    #[cfg(test)]
    pub(crate) fn seed_openrouter_budget_snapshot(
        &mut self,
        target: &Provider,
        reference: &AccountRef,
        observed: chrono::DateTime<chrono::Utc>,
        value: &serde_json::Value,
    ) {
        self.seed_upstream_quota("openrouter", target, reference, observed, value);
    }

    #[cfg(test)]
    pub(crate) fn seed_upstream_quota(
        &mut self,
        kind: &'static str,
        target: &Provider,
        reference: &AccountRef,
        observed: chrono::DateTime<chrono::Utc>,
        value: &serde_json::Value,
    ) {
        let snapshot = local_agent_core::billing::api_at(kind, value, observed).unwrap();
        let source_epoch = Some(self.sources.lease(kind).unwrap().epoch);
        self.cache_snapshot(
            format!("upstream-{}", reference.id),
            source_revision(target, reference),
            source_epoch,
            Some(target),
            snapshot,
            observed,
        )
        .unwrap();
    }

    #[cfg(test)]
    pub(crate) fn seed_exhausted_budget(
        &mut self,
        target: &Provider,
        reference: &AccountRef,
        observed: chrono::DateTime<chrono::Utc>,
        reset: i64,
    ) {
        let mut snapshot = AccountSnapshot::new("api", "openrouter");
        snapshot.as_of = observed.to_rfc3339();
        snapshot
            .windows
            .push(local_agent_core::billing::QuotaWindow {
                id: OPENROUTER_KEY_BUDGET.into(),
                label: "fixture".into(),
                used_percent: Some(100.0),
                remaining_percent: Some(0.0),
                resets_at: Some(reset),
                duration_mins: Some(1440),
            });
        self.cache_snapshot(
            format!("upstream-{}", reference.id),
            source_revision(target, reference),
            Some(self.sources.lease("openrouter").unwrap().epoch),
            Some(target),
            snapshot,
            observed,
        )
        .unwrap();
    }
}

#[cfg(test)]
pub(crate) fn upstream_exhausted(state: &AppState, target: &Provider) -> bool {
    state.billing.lock().is_ok_and(|billing| {
        billing
            .upstream_budget(target, chrono::Utc::now())
            .exhausted
    })
}

fn source_revision(target: &Provider, reference: &AccountRef) -> String {
    files::hash(
        &serde_json::to_vec(&(
            &target.id,
            &target.endpoint,
            &target.protocol,
            &target.credential_ref,
            &reference.id,
            reference.revision,
        ))
        .expect("infallible upstream account identity"),
    )
}

fn row(target: &Provider, reference: AccountRef, label: &str) -> Option<Account> {
    let (kind, _) = api::adapter(target)?;
    Some(Account {
        source_kind: Some(kind),
        read_scope: read_scope(target, kind),
        source_revision: source_revision(target, &reference),
        id: format!("upstream-{}", reference.id),
        name: format!("{} · {label}", target.name),
        account_type: if kind == MINIMAX_PLAN {
            "subscription"
        } else {
            "api"
        }
        .into(),
        scope: if kind == MINIMAX_PLAN {
            "模型服务订阅账号 · 同一订阅的 Key 可能共享额度"
        } else {
            "模型服务上游账号 · 独立凭据"
        }
        .into(),
        provider_id: Some(target.id.clone()),
        upstream_account: Some(reference),
        routing_quota: None,
        declared_for: vec![],
        readable: true,
        official_url: official(target).map(str::to_owned),
        state: "not_loaded".into(),
        error: None,
        snapshot: None,
    })
}

pub(super) fn rows(
    store: &Store,
    providers: &[Provider],
    billing: &Billing,
) -> Result<Vec<Account>, ConfigError> {
    let mut rows = vec![];
    let now = chrono::Utc::now();
    for provider in providers
        .iter()
        .filter(|p| p.configured && api::adapter(p).is_some())
    {
        for account in store.upstream_accounts(&provider.id)? {
            let reference = account.reference();
            match store.upstream_account_target(provider, &reference) {
                Ok((target, _)) => {
                    if let Some(mut account) = row(&target, reference, &account.label) {
                        account.routing_quota =
                            routing_quota_status(Some(billing), store, &target, now);
                        rows.push(account);
                    }
                }
                Err(error) if error.code == "account_unavailable" => (),
                Err(error) => return Err(error),
            }
        }
    }
    Ok(rows)
}

pub(super) fn by_id(store: &Store, id: &str) -> Result<Option<Account>, ConfigError> {
    let Some(id) = id.strip_prefix("upstream-") else {
        return Ok(None);
    };
    let Some(account) = store.upstream_account(id)? else {
        return Ok(None);
    };
    let Some(provider) = store
        .provider_by_id(&account.provider_id)
        .map_err(|_| ConfigError::new("service_unavailable", "上游账号连接不可读取"))?
    else {
        return Ok(None);
    };
    let reference = account.reference();
    match store.upstream_account_target(&provider, &reference) {
        Ok((target, _)) if provider.configured => Ok(row(&target, reference, &account.label)),
        Ok(_) => Ok(None),
        Err(error) if error.code == "account_unavailable" => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn target(
    store: &Store,
    provider: Provider,
    account: &Account,
) -> Result<Provider, &'static str> {
    let Some(reference) = &account.upstream_account else {
        return (fingerprint(&provider) == account.source_revision)
            .then_some(provider)
            .ok_or("服务商配置已改变");
    };
    let (target, _) = store
        .upstream_account_target(&provider, reference)
        .map_err(|_| "上游账号已暂停、撤销或改变")?;
    if source_revision(&target, reference) != account.source_revision {
        return Err("上游账号版本已改变");
    }
    Ok(target)
}
