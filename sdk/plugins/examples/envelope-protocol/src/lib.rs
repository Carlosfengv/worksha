wit_bindgen::generate!({path:"../../wit",world:"plugin"});
use serde_json::{Value, json};
struct Envelope;
#[derive(serde::Deserialize)]
struct Chunk {
    bytes: Vec<u8>,
}
#[derive(serde::Serialize)]
struct Output {
    bytes: Vec<u8>,
    complete: bool,
}
#[derive(Default)]
struct State {
    stream: bool,
    buffer: Vec<u8>,
}
static STATE: std::sync::Mutex<State> = std::sync::Mutex::new(State {
    stream: false,
    buffer: Vec::new(),
});
impl Guest for Envelope {
    fn invoke(method: String, payload: String) -> Result<String, String> {
        if method == "plugin.initialize" {
            return Ok(payload);
        }
        if method.starts_with("plugin.") {
            return Ok(json!({"ready":true}).to_string());
        }
        let mut state = STATE.lock().map_err(|_| "unavailable")?;
        match method.as_str() {
            "protocol.open" => {
                let p: Value = serde_json::from_str(&payload).map_err(|_| "invalid_input")?;
                state.stream = p["stream"] == true;
                state.buffer.clear();
                Ok(json!({"bytes":serde_json::to_vec(&json!({"payload":p["request"]})).map_err(|_|"invalid_input")?}).to_string())
            }
            "protocol.feed" => {
                let chunk: Chunk = serde_json::from_str(&payload).map_err(|_| "invalid_input")?;
                state.buffer.extend_from_slice(&chunk.bytes);
                if state.buffer.len() > 262144 {
                    return Err("resource_limit".into());
                }
                let mut out = Vec::new();
                if state.stream {
                    while let Some(end) = state.buffer.iter().position(|b| *b == b'\n') {
                        let row: Vec<_> = state.buffer.drain(..=end).collect();
                        let frame: Value =
                            serde_json::from_slice(&row).map_err(|_| "invalid_input")?;
                        out.extend_from_slice(
                            frame["payload"].as_str().ok_or("invalid_input")?.as_bytes(),
                        );
                    }
                }
                serde_json::to_string(&Output {
                    bytes: out,
                    complete: false,
                })
                .map_err(|_| "invalid_input".into())
            }
            "protocol.finish" => {
                let out = if state.stream {
                    if !state.buffer.is_empty() {
                        return Err("incomplete".into());
                    }
                    vec![]
                } else {
                    let v: Value =
                        serde_json::from_slice(&state.buffer).map_err(|_| "invalid_input")?;
                    if v.get("payload").is_none() {
                        return Err("invalid_input".into());
                    }
                    serde_json::to_vec(&v["payload"]).map_err(|_| "invalid_input")?
                };
                serde_json::to_string(&Output {
                    bytes: out,
                    complete: true,
                })
                .map_err(|_| "invalid_input".into())
            }
            _ => Err("unsupported".into()),
        }
    }
}
export!(Envelope);
