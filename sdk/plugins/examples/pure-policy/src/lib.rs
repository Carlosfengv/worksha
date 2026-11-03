//! Deterministic reverse order over identities already admitted by the host.
wit_bindgen::generate!({path:"../../wit",world:"plugin"});
use serde_json::{Value, json};
struct PurePolicy;
impl Guest for PurePolicy {
    fn invoke(method: String, payload: String) -> Result<String, String> {
        match method.as_str() {
            "plugin.initialize" => Ok(payload),
            "plugin.prepare" | "plugin.activate" | "plugin.health" | "plugin.deactivate"
            | "plugin.dispose" => Ok(json!({"ready":true}).to_string()),
            "policy.rank" => {
                let input: Value = serde_json::from_str(&payload).map_err(|_| "invalid_input")?;
                let candidates = input["request"]["candidates"]
                    .as_array()
                    .ok_or("invalid_input")?;
                if candidates.len() > 32 {
                    return Err("resource_limit".into());
                }
                let order: Vec<_> = candidates
                    .iter()
                    .rev()
                    .map(|c| c["modelId"].clone())
                    .collect();
                Ok(json!({"requestIdentity":input["requestIdentity"],"order":order,"reason":"extension-order"}).to_string())
            }
            _ => Err("unsupported".into()),
        }
    }
}
export!(PurePolicy);
