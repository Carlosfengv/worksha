use serde::Deserialize;
use std::{collections::BTreeMap, sync::LazyLock};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorPolicy {
    pub http_status: u16,
    pub outcome_unknown: bool,
    pub initial_provider_write: InitialProviderWrite,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InitialProviderWrite {
    Reject,
    Query,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Contract {
    version: u8,
    fallback: ErrorPolicy,
    errors: BTreeMap<String, ErrorPolicy>,
}
static CONTRACT: LazyLock<Contract> = LazyLock::new(|| {
    let contract: Contract = serde_json::from_str(include_str!(
        "../../../../resources/provider-error-contract.json"
    ))
    .expect("bundled provider error contract");
    assert_eq!(contract.version, 1);
    assert!(
        contract
            .errors
            .values()
            .chain([&contract.fallback])
            .all(|policy| (400..=599).contains(&policy.http_status))
    );
    contract
});

pub fn error_policy(code: &str) -> ErrorPolicy {
    CONTRACT
        .errors
        .get(code)
        .copied()
        .unwrap_or(CONTRACT.fallback)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_keeps_unknown_errors_queryable_and_unknown_results_never_rejected() {
        assert_eq!(error_policy("future_error"), CONTRACT.fallback);
        assert_eq!(
            CONTRACT.fallback.initial_provider_write,
            InitialProviderWrite::Query
        );
        for policy in CONTRACT.errors.values() {
            if policy.outcome_unknown {
                assert_eq!(policy.initial_provider_write, InitialProviderWrite::Query);
            }
        }
        assert_eq!(error_policy("operation_conflict").http_status, 409);
        assert_eq!(error_policy("operation_not_found").http_status, 404);
        assert!(error_policy("credential_result_unknown").outcome_unknown);
    }
}
