wit_bindgen::generate!({path:"../../wit",world:"plugin"});
#[path = "../../../support.rs"]
mod support;
use serde_json::{Value, json};
struct Aurora;
static SETTINGS: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
impl Guest for Aurora {
    fn invoke(method: String, payload: String) -> Result<String, String> {
        if method == "plugin.initialize" {
            let value: Value = serde_json::from_str(&payload).map_err(|_| "invalid_input")?;
            let _ = SETTINGS.set(value["settings"].clone());
        }
        if let Some(value) = support::lifecycle(&method, &payload) {
            return value;
        }
        let request: Value = serde_json::from_str(&payload).map_err(|_| "invalid_input")?;
        let params = if method == "session.read" {
            &request["source"]
        } else {
            &request
        };
        let (text, revision) = support::source(params)?;
        // An incomplete tail is a partial scan, never evidence that old sessions vanished.
        if !text.is_empty() && !text.ends_with('\n') {
            return Err("stale_snapshot".into());
        }
        let rows: Vec<Value> = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()
            .map_err(|_| "invalid_input")?;
        match method.as_str() {
            "session.scan" => {
                let start = request["cursor"]
                    .as_str()
                    .unwrap_or("0")
                    .parse::<usize>()
                    .map_err(|_| "invalid_input")?;
                if start > rows.len() {
                    return Err("stale_snapshot".into());
                }
                let end = (start + 256).min(rows.len());
                let items:Vec<_>=rows[start..end].iter().map(|row|{let title=if SETTINGS.get().is_some_and(|s|s["title-style"]=="with-model"){format!("{} · {}",row["title"].as_str().unwrap_or(""),row["model"].as_str().unwrap_or("未知模型"))}else{row["title"].as_str().unwrap_or("").to_owned()};json!({"nativeId":row["nativeId"],"title":title,"createdAt":row["createdAt"],"model":row["model"],"usage":row["usage"]})}).collect();
                Ok(json!({"schemaVersion":1,"sourceRef":request["sourceRef"],"sourceRevision":revision,"batchId":format!("batch-{start}"),"upserts":items,"nextCursor":if end<rows.len(){Some(end.to_string())}else{None},"complete":end==rows.len()}).to_string())
            }
            "session.read" => {
                let row = rows
                    .iter()
                    .find(|row| row["nativeId"] == request["nativeId"])
                    .ok_or("not_found")?;
                Ok(json!({"nativeId":row["nativeId"],"messages":row["messages"]}).to_string())
            }
            _ => Err("unsupported".into()),
        }
    }
}
export!(Aurora);
