//! Quota reads use explicit host bindings. Transcript usage never becomes an account balance.
use super::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{future::Future, pin::Pin};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuotaRequest {
    pub invocation: Invocation,
    pub account_ref: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuotaWindow {
    pub id: String,
    pub label: String,
    pub used_percent: Option<String>,
    pub remaining_percent: Option<String>,
    pub resets_at: Option<String>,
    pub duration_minutes: Option<Decimal>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Money {
    pub label: String,
    pub amount: Option<String>,
    pub currency: String,
    pub kind: String,
    pub period: Option<String>,
    pub resets_at: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResetCredits {
    pub available_count: Option<Decimal>,
    pub details_complete: bool,
    pub expirations: Vec<Option<String>>,
    pub access_mode: String,
    pub reason: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountQuota {
    pub account_ref: String,
    pub identity: Option<String>,
    pub plan: Option<String>,
    pub account_type: String,
    pub windows: Vec<QuotaWindow>,
    pub money: Vec<Money>,
    pub reset_credits: ResetCredits,
    pub source_as_of: String,
    pub valid_for_seconds: Decimal,
    pub source: String,
    pub notes: Vec<String>,
}
pub type QuotaFuture<'a> = Pin<Box<dyn Future<Output = Result<AccountQuota>> + Send + 'a>>;
pub trait AccountSource {
    fn refresh_quota<'a>(&'a mut self, request: &'a QuotaRequest) -> QuotaFuture<'a>;
}
impl AccountQuota {
    pub fn validate(&self) -> Result<()> {
        let text = |s: &str, max: usize| {
            !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control)
        };
        let percent = |p: &Option<String>| {
            p.as_ref().is_none_or(|s| {
                s.len() <= 64
                    && s.parse::<f64>()
                        .is_ok_and(|n| n.is_finite() && (0.0..=100.0).contains(&n))
            })
        };
        let money = |m: &Option<String>| {
            m.as_ref().is_none_or(|s| {
                let raw = s.strip_prefix('-').unwrap_or(s);
                let parts: Vec<_> = raw.split('.').collect();
                s.len() <= 48
                    && parts.len() <= 2
                    && parts
                        .iter()
                        .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            })
        };
        if !text(&self.account_ref, 256)
            || chrono::DateTime::parse_from_rfc3339(&self.source_as_of).is_err()
            || self.windows.len() > 128
            || self.money.len() > 128
            || self.reset_credits.expirations.len() > 100
            || self.windows.iter().any(|w| {
                !text(&w.id, 256)
                    || !text(&w.label, 512)
                    || !percent(&w.used_percent)
                    || !percent(&w.remaining_percent)
            })
            || self
                .money
                .iter()
                .any(|m| !text(&m.currency, 16) || !text(&m.kind, 64) || !money(&m.amount))
        {
            return Err(Failure::new(ErrorCode::InvalidInput));
        }
        Ok(())
    }
    pub fn from_native(account_ref: &str, v: &crate::billing::AccountSnapshot) -> Self {
        Self {
            account_ref: account_ref.into(),
            identity: v.identity.clone(),
            plan: v.plan.clone(),
            account_type: v.account_type.clone(),
            windows: v
                .windows
                .iter()
                .map(|w| QuotaWindow {
                    id: w.id.clone(),
                    label: w.label.clone(),
                    used_percent: w.used_percent.map(|p| p.to_string()),
                    remaining_percent: w.remaining_percent.map(|p| p.to_string()),
                    resets_at: w.resets_at.map(|t| t.to_string()),
                    duration_minutes: w.duration_mins.map(Into::into),
                })
                .collect(),
            money: v
                .money
                .iter()
                .map(|m| Money {
                    label: m.label.clone(),
                    amount: m.amount.clone(),
                    currency: m.currency.clone(),
                    kind: m.kind.clone(),
                    period: m.period.clone(),
                    resets_at: m.resets_at.clone(),
                })
                .collect(),
            reset_credits: ResetCredits {
                available_count: v.reset_credits.available_count.map(Into::into),
                details_complete: v.reset_credits.details_complete,
                expirations: v
                    .reset_credits
                    .expirations
                    .iter()
                    .map(|t| t.map(|t| t.to_string()))
                    .collect(),
                access_mode: v.reset_credits.access_mode.clone(),
                reason: v.reset_credits.reason.clone(),
            },
            source_as_of: v.as_of.clone(),
            valid_for_seconds: v.valid_for_seconds.into(),
            source: v.source.clone(),
            notes: v.notes.clone(),
        }
    }
    /// A pure clock check; stale source evidence never becomes a fresh zero balance.
    pub fn is_stale_at(&self, now: chrono::DateTime<chrono::Utc>) -> bool {
        chrono::DateTime::parse_from_rfc3339(&self.source_as_of).map_or(true, |as_of| {
            let age = now.signed_duration_since(as_of).num_seconds();
            age < 0 || age as u64 > self.valid_for_seconds.value()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn quota_contract_preserves_expiry_independent_currencies_zero_and_unknowns() {
        let mut legacy = crate::billing::api("deepseek", &json!({"balance_infos":[{"currency":"CNY","total_balance":"0.000000"},{"currency":"USD","total_balance":"12.123456"}]})).unwrap();
        legacy.as_of = "2026-10-30T00:00:00Z".into();
        let quota = AccountQuota::from_native("explicit-account-one", &legacy);
        quota.validate().unwrap();
        assert_eq!(quota.money[0].amount.as_deref(), Some("0.000000"));
        assert_eq!(quota.money[1].amount.as_deref(), Some("12.123456"));
        assert_ne!(quota.money[0].currency, quota.money[1].currency);
        assert!(quota.reset_credits.available_count.is_none());
        assert!(
            quota.is_stale_at(
                chrono::DateTime::parse_from_rfc3339("2026-10-31T00:00:00Z")
                    .unwrap()
                    .into()
            )
        );
        let missing = AccountQuota::from_native(
            "explicit-account-two",
            &crate::billing::AccountSnapshot::new("subscription", "fixture"),
        );
        assert_ne!(quota.account_ref, missing.account_ref);
        assert!(missing.windows.is_empty());
        assert!(missing.money.is_empty());
        assert!(missing.plan.is_none());
        let mut bad = quota;
        bad.money[0].amount = Some("NaN".into());
        assert!(bad.validate().is_err());
    }
}
