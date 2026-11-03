//! Protocol transformation only. Sending, credentials, retries and observation belong to the host.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;

pub const API_VERSION: &str = "1.0.0";

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Open {
    pub stream: bool,
    pub request: Value,
}

/// A native adapter can retain a parsed request without an encode/decode round trip.
/// Legacy guests can only produce their existing validated wire-byte response.
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
pub enum Opened {
    Json(Value),
    Bytes(Vec<u8>),
}

#[derive(Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Output {
    /// Keep frame order and boundaries. Empty frames need not be delivered.
    pub chunks: Vec<Vec<u8>>,
    /// Transformation reached its validated terminal state, not proof of billing or success.
    pub complete: bool,
}

/// Native JSON stays parsed until host accounting has completed, as before.
/// Wire adapters retain ordered bytes and their explicit completeness check.
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
pub enum Finished {
    Json(Value),
    Stream(Output),
}

/// One instance per request. No implicit retry or fallback is part of this interface.
/// Native event bounds and guest worker budgets remain independently constrained.
/// Cancellation drops the instance; callers must never synthesize a successful finish.
pub trait ProtocolAdapter {
    type Error;
    fn open(&mut self, input: Open) -> impl Future<Output = Result<Opened, Self::Error>> + Send;
    fn feed(&mut self, bytes: &[u8]) -> impl Future<Output = Result<Output, Self::Error>> + Send;
    fn finish(&mut self) -> impl Future<Output = Result<Finished, Self::Error>> + Send;
}

pub fn schemas() -> Value {
    serde_json::json!({
        "open": schemars::schema_for!(Open),
        "opened": schemars::schema_for!(Opened),
        "output": schemars::schema_for!(Output),
        "finished": schemars::schema_for!(Finished),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protocol_contract_is_separate_and_has_no_sending_authority() {
        assert_eq!(API_VERSION, "1.0.0");
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contracts/domain/protocol");
        let schemas = schemas();
        assert_eq!(schemas.as_object().unwrap().len(), 4);
        for (name, schema) in schemas.as_object().unwrap() {
            let saved: Value = serde_json::from_slice(
                &std::fs::read(root.join(format!("{name}.schema.json"))).unwrap(),
            )
            .unwrap();
            assert_eq!(&saved, schema);
        }
        let mut input = serde_json::json!({"stream":true,"request":{"model":"owned"}});
        for key in ["endpoint", "credentialRef", "headers", "retry", "sent"] {
            input[key] = serde_json::json!("forbidden");
            assert!(serde_json::from_value::<Open>(input.clone()).is_err());
            input.as_object_mut().unwrap().remove(key);
        }
        let output: Output =
            serde_json::from_value(serde_json::json!({"chunks":[[0,255],[10]],"complete":false}))
                .unwrap();
        assert_eq!(output.chunks, vec![vec![0, 255], vec![10]]);
        assert!(!output.complete);
    }
}
