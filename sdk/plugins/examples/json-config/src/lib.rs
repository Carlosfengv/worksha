wit_bindgen::generate!({path:"../../wit",world:"plugin"});
use serde_json::{Value, json};
struct Config;
impl Guest for Config {
    fn invoke(method: String, payload: String) -> Result<String, String> {
        if method == "plugin.initialize" {
            return Ok(payload);
        }
        if method.starts_with("plugin.") {
            return Ok(json!({"ready":true}).to_string());
        }
        if method != "config.propose" {
            return Err("unsupported".into());
        }
        let p: Value = serde_json::from_str(&payload).map_err(|_| "invalid_input")?;
        Ok(json!({"edits":p["edits"]}).to_string())
    }
}
export!(Config);
