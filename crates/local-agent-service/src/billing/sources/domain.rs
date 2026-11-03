//! Private native broker for the same quota operation a worker adapter implements.
use super::*;
use a::AccountSource as DomainAccountSource;
use local_agent_core::domain::{self as d, accounts as a};

struct NativeRead<'a> {
    source: &'a dyn AccountSource,
    raw: Option<ReadRequest<'a>>,
    expected: a::QuotaRequest,
    legacy: Option<AccountSnapshot>,
    error: Option<&'static str>,
}
impl DomainAccountSource for NativeRead<'_> {
    fn refresh_quota<'a>(&'a mut self, request: &'a a::QuotaRequest) -> a::QuotaFuture<'a> {
        Box::pin(async move {
            if request != &self.expected {
                return Err(d::Failure::new(d::ErrorCode::StaleSnapshot));
            }
            let raw = self
                .raw
                .take()
                .ok_or_else(|| d::Failure::new(d::ErrorCode::InvalidInput))?;
            let legacy = self.source.read(raw).await.map_err(|message| {
                self.error = Some(message);
                d::Failure::new(d::ErrorCode::Unavailable)
            })?;
            let quota = a::AccountQuota::from_native(&request.account_ref, &legacy);
            quota.validate()?;
            self.legacy = Some(legacy);
            Ok(quota)
        })
    }
}
pub(super) async fn read(
    source: &dyn AccountSource,
    raw: ReadRequest<'_>,
    kind: &'static str,
    epoch: u64,
) -> Result<AccountSnapshot, &'static str> {
    let expected = a::QuotaRequest {
        invocation: d::Invocation {
            invocation_id: uuid::Uuid::new_v4().to_string(),
            provider_key: format!("builtin:domain.account-source@1:{kind}"),
            implementation_identity: format!(
                "local-agent-service@{}:account:{kind}",
                env!("CARGO_PKG_VERSION")
            ),
            epoch: epoch.into(),
            grant_revision: epoch.into(),
            catalog_revision: 1u64.into(),
            resource_ref: uuid::Uuid::new_v4().to_string(),
            deadline_ms: None,
            budget: d::Budget {
                batch_items: 1,
                max_records: 1u64.into(),
                max_document_bytes: 2_097_152u64.into(),
            },
        },
        // This single invocation is bound to the existing prepared account read;
        // no guessed identity or shared provider-name account is introduced.
        account_ref: uuid::Uuid::new_v4().to_string(),
    };
    let mut bridge = NativeRead {
        source,
        raw: Some(raw),
        expected: expected.clone(),
        legacy: None,
        error: None,
    };
    let quota = bridge
        .refresh_quota(&expected)
        .await
        .map_err(|_| bridge.error.unwrap_or("额度结果未通过领域契约核查"))?;
    if quota.account_ref != expected.account_ref {
        return Err("账户范围已变化，已丢弃额度结果");
    }
    bridge.legacy.take().ok_or("额度来源未完成读取")
}
