//! Versioned domain contracts. Execution, consent and native indexes stay in the host.
pub mod accounts;
pub mod activity;
mod capabilities;
mod catalog;
pub mod config;
mod contracts;
pub mod facts;
pub mod mcp;
pub mod memory;
pub mod object_summary;
pub mod protocol;
pub mod provider;
pub mod provider_access;
pub mod provider_connection;
pub mod provider_draft_catalog;
pub mod provider_draft_grant;
pub mod provider_draft_recovery;
pub mod provider_presets;
pub mod provider_source;
pub mod provider_source_operation;
mod query;
pub mod requests;
pub mod routing_policy;
pub mod routing_policy_recovery;
pub mod source;
pub use capabilities::{CapabilityCatalog, CapabilityEntry, ReadOperation};
pub use catalog::{Catalog, CatalogSnapshot, Product, Provider, Source};
pub use contracts::*;
pub use query::*;
#[cfg(test)]
mod tests;

pub const API_VERSION: &str = "1.0.0";

pub fn schemas() -> serde_json::Value {
    serde_json::json!({
        "catalog": schemars::schema_for!(CatalogSnapshot),
        "invocation": schemars::schema_for!(Invocation),
        "scan-request": schemars::schema_for!(ScanRequest),
        "source-batch": schemars::schema_for!(SourceBatch),
        "checkpoint": schemars::schema_for!(Checkpoint),
        "conversation-request": schemars::schema_for!(ConversationRequest),
        "conversation-page": schemars::schema_for!(ConversationPage),
        "failure": schemars::schema_for!(Failure),
        "unified-session-query": schemars::schema_for!(UnifiedSessionQuery),
        "unified-session-page": schemars::schema_for!(UnifiedSessionPage),
        "unified-conversation-request": schemars::schema_for!(UnifiedConversationRequest),
        "unified-conversation": schemars::schema_for!(UnifiedConversation),
        "source-consent": schemars::schema_for!(SourceConsent),
        "source-consent-query": schemars::schema_for!(SourceConsentQuery),
        "source-consent-review": schemars::schema_for!(SourceConsentReview),
        "unified-catalog": schemars::schema_for!(UnifiedCatalog)
    })
}
