//! Reserved plan contracts. No cleanup or redemption executor is registered in M3.
use serde::{Deserialize, Serialize};
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CleanupPlanRequest {
    pub scope: String,
    pub frozen_session_ids: Vec<String>,
    pub include_content: bool,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RedemptionPlanRequest {
    pub account_ref: String,
    pub snapshot_revision: String,
    pub credit_ref: Option<String>,
    pub idempotency_key: String,
}
#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct MissingPlanCapability {
    pub capability: &'static str,
    pub state: &'static str,
    pub reason: &'static str,
}
pub const CLEANUP: MissingPlanCapability = MissingPlanCapability {
    capability: "cleanup.plan-provider@1",
    state: "unsupported",
    reason: "清理预览、资产复核和可恢复执行尚未实现",
};
pub const REDEMPTION: MissingPlanCapability = MissingPlanCapability {
    capability: "account.credit-plan@1",
    state: "unsupported",
    reason: "重置券直接兑换与幂等结果核查尚未实现；可使用已有官方入口",
};
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_executors_never_report_an_operation_or_success() {
        for capability in [&CLEANUP, &REDEMPTION] {
            let value = serde_json::to_value(capability).unwrap();
            assert_eq!(value["state"], "unsupported");
            assert!(value.get("operationId").is_none());
        }
    }
}
