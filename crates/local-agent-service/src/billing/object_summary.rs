//! A projection of existing quota metadata, never a refresh or credential read.
use super::*;
use local_agent_core::domain::{self as d, object_summary as o};

impl Billing {
    pub(crate) fn object_summary(
        &self,
        id: &str,
        providers: &[Provider],
        configs: &[NativeConfig],
        fields: &[o::SummaryField],
    ) -> d::Result<o::ObjectSummaryRead> {
        let selector = o::ObjectSelector::Account { id: id.into() };
        selector.validate()?;
        o::validate_fields(o::ObjectTarget::Account, fields)?;
        let account = self
            .catalog(providers, configs)
            .into_iter()
            .find(|a| a.id == id)
            .ok_or_else(|| d::Failure::new(d::ErrorCode::NotFound))?;
        if account
            .source_kind
            .is_some_and(|kind| !self.sources.active(kind))
        {
            return Err(d::Failure::new(d::ErrorCode::GrantRevoked));
        }
        let (coverage, freshness, captured_at, value) = match &account.snapshot {
            Some(snapshot) => (
                d::Coverage::Complete,
                if account.state == "ready" {
                    d::Freshness::Current
                } else {
                    d::Freshness::Stale
                },
                snapshot.as_of.clone(),
                Some(o::SummaryValue::Count {
                    value: (snapshot.windows.len() as u64).into(),
                }),
            ),
            None => (
                d::Coverage::Unavailable,
                d::Freshness::Unread,
                chrono::Utc::now().to_rfc3339(),
                None,
            ),
        };
        let summary = o::ObjectSummary {
            api_version: o::API_VERSION.into(),
            target: o::ObjectTarget::Account,
            coverage,
            freshness,
            captured_at,
            metrics: fields
                .iter()
                .map(|field| o::SummaryMetric {
                    field: *field,
                    value: value.clone(),
                    missing_reason: value.is_none().then_some(o::MissingReason::Unread),
                })
                .collect(),
        };
        let epochs = self.sources.domain_capabilities();
        let result = o::ObjectSummaryRead {
            selector: selector.clone(),
            read_identity: local_agent_core::projects::hash(&json!([account, epochs]).to_string()),
            summary,
        };
        result.validate(&selector, fields)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn object_account_summary_does_not_refresh_and_stopped_sources_cannot_reuse_cache() {
        let home = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let mut billing = Billing::new(home.path().into(), data.path());
        let fields = [o::SummaryField::QuotaWindowCount];
        let unread = billing
            .object_summary("native-codex", &[], &[], &fields)
            .unwrap();
        assert_eq!(unread.summary.freshness, d::Freshness::Unread);
        assert!(unread.summary.metrics[0].value.is_none());
        // An explicitly refreshed snapshot with no reported windows is real zero.
        billing.cache.insert(
            "native-codex".into(),
            Cached {
                revision: String::new(),
                snapshot: AccountSnapshot::new("subscription", "owned-fixture"),
                error: None,
            },
        );
        let cached = billing
            .object_summary("native-codex", &[], &[], &fields)
            .unwrap();
        assert_eq!(
            cached.summary.metrics[0].value,
            Some(o::SummaryValue::Count {
                value: 0_u64.into()
            })
        );
        assert_ne!(unread.read_identity, cached.read_identity);
        assert!(billing.attempted.is_empty());
        assert!(billing.in_flight.is_empty());
        assert!(std::fs::read_dir(data.path()).unwrap().next().is_none());
        billing.sources.stop("codex");
        assert_eq!(
            billing
                .object_summary("native-codex", &[], &[], &fields)
                .unwrap_err()
                .code,
            d::ErrorCode::GrantRevoked
        );
    }
}
