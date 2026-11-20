wit_bindgen::generate!({path:"../../wit",world:"plugin"});
#[allow(dead_code)]
#[path = "../support.rs"]
mod support;

use local_agent::plugin::host;
use serde_json::{Value, json};

fn dispatch(method: &str, payload: &str) -> Result<String, String> {
    let input: Value = serde_json::from_str(payload).map_err(|_| "invalid_input")?;
    let reply = match method {
        "platform.render" => {
            let rows = input["records"].as_array().ok_or("invalid_input")?;
            let mut ids = Vec::new();
            for row in rows {
                if let Some(handle) = row["objectHandles"]["session"].as_str() {
                    let request = json!({"handle":handle}).to_string();
                    let summary = host::call("domain.read", &request)?;
                    let summary: Value =
                        serde_json::from_str(&summary).map_err(|_| "invalid_input")?;
                    if summary["authorization"] != "authorized" {
                        return Err("permission_denied".into());
                    }
                }
                ids.push(&row["recordId"]);
            }
            json!({"recordIds":ids})
        }
        "platform.action" => {
            let raw = host::call("storage.kv.get", r#"{"key":"test-count"}"#)?;
            let old: Value = serde_json::from_str(&raw).map_err(|_| "invalid_input")?;
            let value = old["value"].as_u64().unwrap_or(0) + 1;
            host::call(
                "storage.kv.put",
                &json!({"key":"test-count","value":value}).to_string(),
            )?;
            let mut values = input["values"].clone();
            if values["title"] == "拒绝" {
                return Ok(json!({"accept":false}).to_string());
            }
            if let Some(title) = values["title"].as_str() {
                values["title"] = Value::String(format!("已核对 · {title}"));
            }
            json!({"accept":true,"values":values})
        }
        _ => return Err("unsupported".into()),
    };
    Ok(reply.to_string())
}

define_plugin!(PlatformFixture, dispatch);
