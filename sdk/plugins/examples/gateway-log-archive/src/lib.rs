wit_bindgen::generate!({path:"../../wit",world:"plugin"});
#[allow(dead_code)]
#[path = "../../../support.rs"]
mod support;

use serde_json::{Value, json};

fn transform(method: &str, payload: &str) -> Result<String, String> {
    if method != "sink.transform" {
        return Err("unsupported".into());
    }
    let input: Value = serde_json::from_str(payload).map_err(|_| "invalid_input")?;
    let records = input["records"].as_array().ok_or("invalid_input")?;
    let lines = records
        .iter()
        .map(|row| {
            json!({
            "time": row["timestamp"],
            "body": { "stringValue": "gateway.request.completed" },
            "attributes": [
                { "key": "request.hash", "value": { "stringValue": row["requestHash"] } },
                { "key": "gateway.client.scope", "value": { "stringValue": row["clientScope"] } },
                { "key": "gateway.outcome", "value": { "stringValue": row["outcome"] } }
            ]
        }).to_string()
        })
        .collect::<Vec<_>>();
    Ok(json!({"lines":lines}).to_string())
}

define_plugin!(LogArchive, transform);
