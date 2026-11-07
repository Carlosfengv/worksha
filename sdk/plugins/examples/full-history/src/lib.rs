wit_bindgen::generate!({path:"../../wit",world:"plugin"});
#[path = "../../../support.rs"]
mod support;
use serde_json::{Value, json};
struct FullHistory;
static HISTORY: std::sync::OnceLock<(Vec<Value>, String)> = std::sync::OnceLock::new();

fn history(request: &Value) -> Result<&'static (Vec<Value>, String), String> {
    if let Some(history) = HISTORY.get() {
        return Ok(history);
    }
    let invocation = &request["invocation"];
    let (text, revision) = support::source(
        &json!({"contextId":invocation["invocationId"],"sourceRef":invocation["resourceRef"]}),
    )?;
    if !text.is_empty() && !text.ends_with('\n') {
        return Err("stale_snapshot".into());
    }
    let mut rows: Vec<Value> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()
        .map_err(|_| "invalid_input")?;
    // Optional framed snapshots keep a producer's selected inode stable while
    // rejecting a torn write, including a valid JSON prefix with an old tail.
    if rows
        .first()
        .is_some_and(|row| row.get("snapshot").is_some())
    {
        let header = rows.remove(0);
        let footer = rows.pop().ok_or("stale_snapshot")?;
        if header["snapshot"].as_str().is_none()
            || footer["commit"] != header["snapshot"]
            || header["records"].as_u64() != Some(rows.len() as u64)
        {
            return Err("stale_snapshot".into());
        }
    }
    if rows.len() > 4096 {
        return Err("resource_limit".into());
    }
    let _ = HISTORY.set((rows, revision));
    HISTORY.get().ok_or_else(|| "invalid_input".into())
}
impl Guest for FullHistory {
    fn invoke(method: String, payload: String) -> Result<String, String> {
        if let Some(value) = support::lifecycle(&method, &payload) {
            return value;
        }
        let request: Value = serde_json::from_str(&payload).map_err(|_| "invalid_input")?;
        let (rows, revision) = history(&request)?;
        if request["sourceRevision"].as_str() != Some(revision.as_str()) {
            return Err("stale_snapshot".into());
        }
        match method.as_str() {
            "domain.scan" => {
                let start = request["cursor"]
                    .as_str()
                    .unwrap_or("0")
                    .parse::<usize>()
                    .map_err(|_| "invalid_input")?;
                if start > rows.len() {
                    return Err("stale_snapshot".into());
                }
                let end = start.saturating_add(256).min(rows.len());
                let records: Vec<_> = rows[start..end].iter().map(|r| r["fact"].clone()).collect();
                let complete = end == rows.len();
                let batches = rows.len().div_ceil(256);
                Ok(json!({
                    "batch":if records.is_empty(){Value::Null}else{json!({"sourceRevision":revision,"batchId":(start/256).to_string(),"records":records})},
                    "checkpoint":if complete{json!({"sourceRevision":revision,"batches":batches.to_string(),"records":rows.len().to_string(),"complete":true,"coverage":if rows.iter().any(|r| r["fact"]["kind"] == "session" && r["fact"]["value"]["partial"] == true) { "partial" } else { "complete" }})}else{Value::Null},
                    "nextCursor":if complete{None}else{Some(end.to_string())}
                }).to_string())
            }
            "domain.read-conversation" => {
                let row = rows
                    .iter()
                    .find(|r| {
                        r["fact"]["kind"] == "session"
                            && r["fact"]["value"]["nativeId"] == request["nativeId"]
                    })
                    .ok_or("not_found")?;
                let events = row["conversation"].as_array().ok_or("not_found")?;
                let start = if let Some(anchor) = request["anchor"].as_str() {
                    events
                        .iter()
                        .position(|e| e["nativeId"].as_str() == Some(anchor))
                        .ok_or("not_found")?
                } else {
                    request["cursor"]
                        .as_str()
                        .unwrap_or("0")
                        .parse::<usize>()
                        .map_err(|_| "invalid_input")?
                };
                if start > events.len() {
                    return Err("stale_snapshot".into());
                }
                let end = start.saturating_add(40).min(events.len());
                Ok(json!({"sessionId":request["nativeId"],"events":events[start..end],"nextCursor":if end<events.len(){Some(end.to_string())}else{None},"complete":end==events.len()}).to_string())
            }
            _ => Err("unsupported".into()),
        }
    }
}
export!(FullHistory);
