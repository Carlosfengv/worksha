wit_bindgen::generate!({path:"../../wit",world:"plugin"});
use serde_json::{Value, json};
struct Pulse;
impl Guest for Pulse {
    fn invoke(method: String, payload: String) -> Result<String, String> {
        if method == "plugin.initialize" {
            return Ok(payload);
        }
        if method.starts_with("plugin.") {
            return Ok(json!({"ready":true}).to_string());
        }
        if method != "account.quota" {
            return Err("unsupported".into());
        }
        let request: Value = serde_json::from_str(&payload).map_err(|_| "invalid_input")?;
        let response = local_agent::plugin::host::call(
            "connection.request",
            &json!({"contextId":request["contextId"],"connectionRef":request["connectionRef"]})
                .to_string(),
        )?;
        let response: Value = serde_json::from_str(&response).map_err(|_| "invalid_input")?;
        let body: Value = serde_json::from_str(response["body"].as_str().ok_or("invalid_input")?)
            .map_err(|_| "invalid_input")?;
        Ok(json!({"remaining":body["balance"],"limit":body["limit"],"unit":body["unit"],"observedAt":request["observedAt"],"expiresAt":body["expiresAt"]}).to_string())
    }
}
export!(Pulse);
