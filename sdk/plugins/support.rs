use serde_json::{Value, json};
pub fn lifecycle(method: &str, payload: &str) -> Option<Result<String, String>> {
    match method {
        "plugin.initialize" => Some(Ok(payload.into())),
        "plugin.prepare" | "plugin.activate" | "plugin.health" | "plugin.deactivate"
        | "plugin.dispose" => Some(Ok(json!({"ready":true}).to_string())),
        _ => None,
    }
}
pub fn source(request: &Value) -> Result<(String, String), String> {
    let mut offset = "0".to_owned();
    let mut text = String::new();
    let mut revision = None;
    for _ in 0..64 {
        let payload = json!({"contextId":request["contextId"],"sourceRef":request["sourceRef"],"offset":offset,"maxBytes":196608});
        let response = crate::local_agent::plugin::host::call("source.read", &payload.to_string())?;
        let value: Value = serde_json::from_str(&response).map_err(|_| "invalid_input")?;
        let next = value["nextOffset"]
            .as_str()
            .ok_or("invalid_input")?
            .to_owned();
        let version = value["sourceRevision"]
            .as_str()
            .ok_or("invalid_input")?
            .to_owned();
        if revision.as_ref().is_some_and(|r| r != &version) {
            return Err("stale_snapshot".into());
        }
        revision = Some(version);
        text.push_str(value["text"].as_str().ok_or("invalid_input")?);
        if text.len() > 8 * 1048576 {
            return Err("resource_limit".into());
        }
        if value["complete"] == true {
            return Ok((text, revision.ok_or("invalid_input")?));
        }
        if next == offset {
            return Err("invalid_input".into());
        }
        offset = next;
    }
    Err("resource_limit".into())
}
