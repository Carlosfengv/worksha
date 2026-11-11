//! Durable admission facts. Display amounts and credentials stay outside this record.
use super::*;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Evidence {
    accounts: BTreeMap<String, AccountEvidence>,
    shared: BTreeMap<String, BudgetFact>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AccountEvidence {
    revision: String,
    observed: Stamp,
    key: Option<BudgetFact>,
    layers: Vec<LayerEvidence>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LayerEvidence {
    id: String,
    shared: bool,
    fact: BudgetFact,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BudgetFact {
    observed: Stamp,
    exhausted: bool,
    reset: Option<i64>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stamp {
    seconds: i64,
    nanos: u32,
    ttl: i64,
}

fn unavailable() -> ConfigError {
    ConfigError::new("quota_evidence_unavailable", "OpenRouter 额度证据暂不可用")
}

impl From<Observation> for Stamp {
    fn from(observed: Observation) -> Self {
        Self {
            seconds: observed.at.timestamp(),
            nanos: observed.at.timestamp_subsec_nanos(),
            ttl: (observed.expires - observed.at).num_seconds(),
        }
    }
}

impl Stamp {
    fn observation(&self) -> Result<Observation, ConfigError> {
        let at = chrono::DateTime::from_timestamp(self.seconds, self.nanos)
            .filter(|_| (0..=3600).contains(&self.ttl))
            .ok_or_else(unavailable)?
            .fixed_offset();
        let expires = at
            .checked_add_signed(chrono::Duration::seconds(self.ttl))
            .ok_or_else(unavailable)?;
        Ok(Observation { at, expires })
    }
}

impl BudgetFact {
    fn capture(window: &QuotaWindow, observed: Observation) -> Self {
        let exhausted = observed.confirmed_exhaustion(window);
        Self {
            observed: observed.into(),
            exhausted,
            reset: window.resets_at.filter(|_| exhausted),
        }
    }

    fn window(&self, id: &str) -> Result<(QuotaWindow, Observation), ConfigError> {
        let observation = self.observed.observation()?;
        if self
            .reset
            .is_some_and(|reset| !self.exhausted || reset <= observation.at.timestamp())
        {
            return Err(unavailable());
        }
        Ok((
            QuotaWindow {
                id: id.into(),
                label: String::new(),
                used_percent: None,
                remaining_percent: self.exhausted.then_some(0.0),
                resets_at: self.reset,
                duration_mins: None,
            },
            observation,
        ))
    }
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn restored_scope(store: &Store, id: &str, revision: &str) -> Result<Option<String>, ConfigError> {
    if let Some(scope) = id.strip_prefix("account:") {
        return (scope == revision)
            .then(|| Some(id.into()))
            .ok_or_else(unavailable);
    }
    let uuid = id
        .strip_prefix("upstream-")
        .and_then(|id| uuid::Uuid::parse_str(id).ok())
        .filter(|uuid| format!("upstream-{uuid}") == id)
        .ok_or_else(unavailable)?;
    let Some(account) = store.upstream_account(&uuid.to_string())? else {
        return Ok(None);
    };
    let provider = store
        .provider_by_id(&account.provider_id)
        .map_err(|_| unavailable())?;
    Ok(provider
        .and_then(|provider| account.quota_source(provider))
        .filter(|target| source_revision(target, &account.reference()) == revision)
        .and_then(|target| openrouter_scope(&target)))
}

impl Billing {
    #[cfg(test)]
    pub(crate) fn persist_openrouter(&self, store: &Store, epoch: u64) -> Result<(), ConfigError> {
        store.save_openrouter_admission_evidence(&self.openrouter_evidence(epoch)?)
    }

    pub(in crate::billing) fn persist_openrouter_refresh(
        &self,
        store: &Store,
        epoch: u64,
        target: &Provider,
        snapshot: &AccountSnapshot,
    ) -> Result<(), ConfigError> {
        let scope = openrouter_scope(target).ok_or_else(unavailable)?;
        let observed = Observation::from_snapshot(snapshot).ok_or_else(unavailable)?;
        let checked = snapshot.openrouter_budget_coverage;
        let key_checked = checked.is_some_and(|c| c.key_unlimited)
            || key_window(snapshot).is_some_and(|window| {
                window
                    .resets_at
                    .is_none_or(|reset| reset > observed.at.timestamp())
            });
        store.save_openrouter_admission_refresh(
            &self.openrouter_evidence(epoch)?,
            &scope,
            observed.at.to_utc(),
            key_checked,
            checked.is_some_and(|c| c.layers_complete),
        )
    }

    fn openrouter_evidence(&self, epoch: u64) -> Result<String, ConfigError> {
        if self.unavailable_budget_epoch == Some(epoch) {
            return Err(unavailable());
        }
        let accounts: BTreeMap<_, _> = self
            .openrouter_admission
            .iter()
            .filter(|(_, saved)| saved.epoch == epoch)
            .filter_map(|(scope, saved)| {
                let key = saved
                    .key
                    .as_ref()
                    .map(|(window, observed)| BudgetFact::capture(window, *observed));
                let layers: Vec<_> = saved
                    .layers
                    .iter()
                    .map(|(layer, observed)| LayerEvidence {
                        id: layer.id.clone(),
                        shared: layer.shared,
                        fact: BudgetFact::capture(&layer.window, *observed),
                    })
                    .collect();
                if key.is_none() && layers.is_empty() {
                    return None;
                }
                Some((
                    scope.clone(),
                    AccountEvidence {
                        revision: scope.strip_prefix("account:")?.into(),
                        observed: saved.observed.into(),
                        key,
                        layers,
                    },
                ))
            })
            .collect();
        let referenced: BTreeSet<_> = accounts
            .values()
            .flat_map(|account| &account.layers)
            .filter(|layer| layer.shared)
            .map(|layer| layer.id.as_str())
            .collect();
        let shared = referenced
            .into_iter()
            .filter_map(|id| {
                let saved = self.shared_budgets.get(id)?;
                (saved.epoch == epoch && saved.retain_until.is_none()).then(|| {
                    (
                        id.to_owned(),
                        BudgetFact::capture(&saved.window, saved.observation),
                    )
                })
            })
            .collect();
        serde_json::to_string(&Evidence { accounts, shared }).map_err(|_| unavailable())
    }

    pub(crate) fn restore_openrouter(&mut self, store: &Store) {
        let Ok(lease) = self.sources.lease("openrouter") else {
            return;
        };
        if self.load_openrouter(store, lease.epoch).is_err() {
            self.unavailable_budget_epoch = Some(lease.epoch);
        }
    }

    fn load_openrouter(&mut self, store: &Store, epoch: u64) -> Result<(), ConfigError> {
        let Some(value) = store.openrouter_admission_evidence()? else {
            return Ok(());
        };
        let evidence: Evidence = serde_json::from_str(&value).map_err(|_| unavailable())?;
        let mut accounts: HashMap<String, OpenRouterAdmission> = HashMap::new();
        let mut shared = HashMap::new();
        let mut referenced = BTreeSet::new();
        for (id, saved) in evidence.accounts {
            if !valid_digest(&saved.revision) || saved.layers.len() > 64 {
                return Err(unavailable());
            }
            let observed = saved.observed.observation()?;
            let key = saved
                .key
                .map(|fact| fact.window(OPENROUTER_KEY_BUDGET))
                .transpose()?;
            let mut layers = Vec::new();
            let mut layer_ids = BTreeSet::new();
            for layer in saved.layers {
                if !valid_digest(&layer.id) || !layer_ids.insert(layer.id.clone()) {
                    return Err(unavailable());
                }
                if layer.shared {
                    referenced.insert(layer.id.clone());
                }
                let (window, observed) = layer.fact.window("budget")?;
                layers.push((
                    OpenRouterBudgetLayer {
                        id: layer.id,
                        shared: layer.shared,
                        window,
                    },
                    observed,
                ));
            }
            let Some(scope) = restored_scope(store, &id, &saved.revision)? else {
                continue;
            };
            let entry = accounts
                .entry(scope)
                .or_insert_with(|| OpenRouterAdmission {
                    epoch,
                    observed,
                    key: None,
                    layers: Vec::new(),
                });
            if observed.at > entry.observed.at {
                entry.observed = observed;
            }
            if key.as_ref().is_some_and(|(_, incoming)| {
                entry
                    .key
                    .as_ref()
                    .is_none_or(|(_, prior)| incoming.at >= prior.at)
            }) {
                entry.key = key;
            }
            // Older versions could store several account rows for the same Key.
            // Each legacy row proves only the windows it actually saved.
            for (layer, observed) in layers {
                match entry
                    .layers
                    .iter_mut()
                    .find(|(prior, _)| prior.id == layer.id)
                {
                    Some(prior) if observed.at >= prior.1.at => *prior = (layer, observed),
                    Some(_) => (),
                    None => entry.layers.push((layer, observed)),
                }
            }
            if entry.layers.len() > 64 {
                return Err(unavailable());
            }
        }
        for (id, fact) in evidence.shared {
            if !referenced.contains(&id) {
                return Err(unavailable());
            }
            let (window, observation) = fact.window("budget")?;
            shared.insert(
                id,
                SharedBudget {
                    window,
                    epoch,
                    observation,
                    retain_until: None,
                },
            );
        }
        self.openrouter_admission.extend(accounts);
        self.shared_budgets.extend(shared);
        Ok(())
    }
}
