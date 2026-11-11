use super::{
    AccountQuotaHint, ModelLimit, RouteDecisionReason as Reason, RoutingPolicy, error, quota,
};
use crate::{
    Provider, Store,
    providers::{ConfigError, upstream_accounts::AccountRef},
    routing::Selection,
};
use rusqlite::{Connection, OptionalExtension, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AccountStrategy {
    #[default]
    Priority,
    RoundRobin,
    Weighted,
    QuotaReset,
    QuotaRemaining,
    QuotaPace,
}

impl AccountStrategy {
    pub fn is_priority(&self) -> bool {
        *self == Self::Priority
    }

    pub fn uses_quota(self) -> bool {
        matches!(
            self,
            Self::QuotaReset | Self::QuotaRemaining | Self::QuotaPace
        )
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AccountAffinity {
    #[default]
    Off,
    Prefer,
    Strict,
}

impl AccountAffinity {
    pub fn is_off(&self) -> bool {
        *self == Self::Off
    }
}

pub fn scopes(provider: &Provider) -> (String, String) {
    let endpoint = url::Url::parse(&provider.endpoint)
        .ok()
        .map(|u| {
            format!(
                "{}://{}:{}",
                u.scheme(),
                u.host_str().unwrap_or(""),
                u.port_or_known_default().unwrap_or(0)
            )
        })
        .unwrap_or_default();
    let account = crate::providers::files::hash(
        format!(
            "{endpoint}|{}",
            provider.credential_ref.as_deref().unwrap_or("anonymous")
        )
        .as_bytes(),
    );
    let model = crate::providers::files::hash(format!("{account}|{}", provider.model).as_bytes());
    (format!("account:{account}"), format!("model:{model}"))
}

/// Access decisions are local to the exact endpoint, wire protocol and credential.
/// Unlike quota and capacity, authorization can differ across paths on the same host.
pub fn access_scopes(provider: &Provider) -> (String, String) {
    let account = crate::providers::files::hash(
        &serde_json::to_vec(&(
            &provider.endpoint,
            &provider.protocol,
            &provider.credential_ref,
        ))
        .expect("infallible access scope"),
    );
    let model = crate::providers::files::hash(format!("{account}|{}", provider.model).as_bytes());
    (
        format!("access-account:{account}"),
        format!("access-model:{model}"),
    )
}

pub(super) struct Order {
    pub accounts: Vec<AccountRef>,
    pub deprioritized: Vec<(u8, Reason)>,
    pub reason: Option<&'static str>,
    pub strict: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AccountPlanningHint {
    pub quota: Option<AccountQuotaHint>,
    pub blocked: Option<Reason>,
}

pub(super) fn order(
    store: &Store,
    tx: &Connection,
    policy: &RoutingPolicy,
    candidate: &Selection,
    binding: &str,
    session: &str,
    hint: impl Fn(&Provider, &AccountRef, &ModelLimit) -> AccountPlanningHint,
) -> Result<Order, ConfigError> {
    let service = candidate
        .service
        .as_ref()
        .ok_or_else(|| error("missing service selection"))?;
    let provider = &candidate.version.provider;
    let pool = policy
        .pool(&service.model_id)
        .ok_or_else(|| error("missing account pool"))?;
    let limit = policy.limit(&service.model_id);
    let strict = pool.affinity == AccountAffinity::Strict;
    let previous: Option<AccountRef> = if pool.affinity.is_off() {
        None
    } else {
        // A strict pin follows the logical model across publications. A
        // changed source or upstream model must still honor that account.
        let raw: Option<String> = tx.query_row(
            "SELECT json_extract(l.service_selection,'$.upstreamAccount')
             FROM gateway_route_leases l
             JOIN service_route_dispatch d ON d.binding=l.binding AND d.session=l.session AND d.turn=l.turn
             JOIN gateway_route_versions v ON v.id=l.version AND v.binding=l.binding
             WHERE l.binding=?1 AND l.session=?2 AND l.trusted=1 AND d.state='committed'
               AND json_extract(l.service_selection,'$.nestedScope') IS ?7
               AND COALESCE(json_extract(l.service_selection,'$.nestedRoute[#-1].serviceId'),
                            json_extract(l.service_selection,'$.serviceId'))=?3
               AND COALESCE(json_extract(l.service_selection,'$.nestedRoute[#-1].modelId'),
                            json_extract(l.service_selection,'$.modelId'))=?4
               AND (?8 OR (
                 json_extract(v.value,'$.provider.id')=?5
                 AND json_extract(v.value,'$.provider.model')=?6
               ))
               AND json_type(l.service_selection,'$.upstreamAccount')='object'
             ORDER BY l.seq DESC LIMIT 1",
            params![binding, session, service.service_id, pool.model_id, provider.id, provider.model, service.nested_scope, strict],
            |r| r.get(0),
        ).optional().map_err(error)?;
        raw.map(|value| serde_json::from_str(&value).map_err(error))
            .transpose()?
    };
    let mut accounts = pool.accounts.clone();
    if strict && let Some(previous) = &previous {
        accounts.retain(|account| account == previous);
        return Ok(Order {
            accounts,
            deprioritized: vec![],
            reason: Some("strict_affinity"),
            strict: true,
        });
    }
    if pool.strategy.is_priority() && previous.is_none() {
        return Ok(Order {
            accounts,
            deprioritized: vec![],
            reason: None,
            strict: false,
        });
    }
    let signals = accounts
        .iter()
        .map(|account| {
            let target = match store.upstream_account_target(provider, account) {
                Ok((target, _)) => target,
                Err(e) if e.code == "account_unavailable" => {
                    return Ok((Some(Reason::AccountUnavailable), None));
                }
                Err(e) => return Err(e),
            };
            if policy.fallback_on_access_denied {
                let (scope, model) = access_scopes(&target);
                if store.service_cooldown(&scope)?.is_some() {
                    return Ok((Some(Reason::AccessCredentialRejected), None));
                }
                if store.service_cooldown(&model)?.is_some() {
                    return Ok((Some(Reason::AccessModelDenied), None));
                }
            }
            let hint = hint(&target, account, &limit);
            if hint.quota.is_some_and(AccountQuotaHint::exhausted) {
                return Ok((Some(Reason::AccountQuotaExhausted), None));
            }
            if let Some(reason) = hint.blocked {
                return Ok((Some(reason), None));
            }
            let (scope, model) = scopes(&target);
            let cooling = store
                .service_cooldown(&scope)?
                .or(store.service_cooldown(&model)?);
            let blocked = cooling.map(|(_, cause)| {
                if cause == "upstream_rate_limit" {
                    Reason::RateLimitCooldown
                } else {
                    Reason::FailureCooldown
                }
            });
            Ok((blocked, hint.quota))
        })
        .collect::<Result<Vec<_>, ConfigError>>()?;
    let availability: Vec<_> = signals
        .iter()
        .map(|&(reason, quota)| (reason.is_none(), quota))
        .collect();
    let deprioritized: Vec<_> = signals
        .into_iter()
        .enumerate()
        .filter_map(|(index, (reason, _))| reason.map(|reason| (index as u8 + 1, reason)))
        .collect();
    let preferred = previous
        .and_then(|prior| accounts.iter().position(|a| *a == prior))
        .filter(|&index| availability[index].0);
    if pool.strategy.uses_quota() {
        let (mut ranked, comparable) = quota::order(pool.strategy, &availability);
        if let Some(prior) = preferred {
            ranked.retain(|&index| index != prior);
            ranked.insert(0, prior);
        }
        let reason = if preferred.is_some() {
            "account_affinity"
        } else if !comparable {
            "quota_unknown"
        } else {
            match pool.strategy {
                AccountStrategy::QuotaReset => "quota_reset",
                AccountStrategy::QuotaPace => "quota_pace",
                _ => "quota_remaining",
            }
        };
        return Ok(Order {
            accounts: ranked
                .into_iter()
                .map(|index| accounts[index].clone())
                .collect(),
            deprioritized,
            reason: Some(reason),
            strict: false,
        });
    }
    if let Some(index) = preferred {
        let prior = accounts.remove(index);
        accounts.insert(0, prior);
        return Ok(Order {
            accounts,
            deprioritized,
            reason: Some("account_affinity"),
            strict: false,
        });
    }
    let ready: Vec<_> = availability.iter().map(|&(ready, _)| ready).collect();
    if pool.strategy.is_priority() || !ready.iter().any(|available| *available) {
        return Ok(Order {
            accounts,
            deprioritized,
            reason: None,
            strict: false,
        });
    }
    let version = &service.service_version_id;
    let old: Option<String> = tx
        .query_row(
            "SELECT scores FROM service_account_rotation WHERE version=?1 AND model=?2",
            params![version, pool.model_id],
            |r| r.get(0),
        )
        .optional()
        .map_err(error)?;
    let mut scores: Vec<i32> = old
        .map(|raw| serde_json::from_str(&raw).map_err(error))
        .transpose()?
        .unwrap_or_else(|| vec![0; accounts.len()]);
    if scores.len() != accounts.len()
        || scores
            .iter()
            .any(|score| !(-64_000..=64_000).contains(score))
    {
        return Err(error("invalid account rotation scores"));
    }
    let weights: Vec<i32> = accounts
        .iter()
        .map(|account| i32::from(pool.weights.get(&account.id).copied().unwrap_or(1)))
        .collect();
    let ranked = smooth_order(&mut scores, &weights, &ready);
    tx.execute(
        "INSERT INTO service_account_rotation VALUES(?1,?2,?3)
         ON CONFLICT(version,model) DO UPDATE SET scores=excluded.scores",
        params![
            version,
            pool.model_id,
            serde_json::to_string(&scores).map_err(error)?
        ],
    )
    .map_err(error)?;
    Ok(Order {
        accounts: ranked
            .into_iter()
            .map(|index| accounts[index].clone())
            .collect(),
        deprioritized,
        reason: Some(if pool.strategy == AccountStrategy::Weighted {
            "weighted_accounts"
        } else {
            "round_robin_accounts"
        }),
        strict: false,
    })
}

fn smooth_order(scores: &mut [i32], weights: &[i32], ready: &[bool]) -> Vec<usize> {
    let mut total = 0;
    for ((score, weight), ready) in scores.iter_mut().zip(weights).zip(ready) {
        if *ready {
            *score += weight;
            total += weight;
        } else {
            *score = 0;
        }
    }
    let mut order: Vec<_> = (0..scores.len()).collect();
    order.sort_by_key(|&index| (Reverse(ready[index]), Reverse(scores[index])));
    if let Some(&winner) = order.first() {
        scores[winner] -= total;
    }
    order
}

#[cfg(test)]
mod tests {
    use super::smooth_order;

    #[test]
    fn weighted_assignments_are_smooth_and_unavailable_accounts_do_not_accrue_credit() {
        let mut scores = [0; 2];
        let winners: Vec<_> = (0..8)
            .map(|_| smooth_order(&mut scores, &[3, 1], &[true, true])[0])
            .collect();
        assert_eq!(winners, [0, 0, 1, 0, 0, 0, 1, 0]);
        for _ in 0..10 {
            assert_eq!(smooth_order(&mut scores, &[3, 1], &[false, true]), [1, 0]);
        }
        assert_eq!(scores, [0, 0]);
        let round_robin: Vec<_> = (0..4)
            .map(|_| smooth_order(&mut scores, &[1, 1], &[true, true])[0])
            .collect();
        assert_eq!(round_robin, [0, 1, 0, 1]);
    }
}
