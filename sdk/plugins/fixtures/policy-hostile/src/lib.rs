//! Only used with disposable homes, loopback providers and test signatures.
wit_bindgen::generate!({path:"../../wit",world:"plugin"});
use serde_json::{Value, json};
struct Hostile;
impl Guest for Hostile {
    fn invoke(method: String, payload: String) -> Result<String, String> {
        if method == "plugin.initialize" {
            let v: Value = serde_json::from_str(&payload).map_err(|_| "invalid_input")?;
            if v["apiVersion"] != "1.0.0"
                || v["capabilities"] != json!(["domain.routing-policy@1"])
                || v["settings"] != json!({})
            {
                return Err("private settings".into());
            }
            return Ok(payload);
        }
        if method.starts_with("plugin.") {
            return Ok(json!({"ready":true}).to_string());
        }
        if cfg!(feature = "host-call") {
            return local_agent::plugin::host::call("connection.request", "{}");
        }
        if cfg!(feature = "loop") {
            loop {
                std::hint::black_box(1);
            }
        }
        if cfg!(feature = "memory") {
            core::arch::wasm32::memory_grow::<0>(4096);
            return Err("allocation denied".into());
        }
        let v: Value = serde_json::from_str(&payload).map_err(|_| "invalid_input")?;
        let r = &v["request"];
        let allowed = [
            "serviceId",
            "serviceVersionId",
            "preferredModelId",
            "defaultModelId",
            "configuredOrder",
            "candidates",
        ];
        if r.as_object()
            .ok_or("invalid_input")?
            .keys()
            .any(|k| !allowed.contains(&k.as_str()))
        {
            return Err("private context".into());
        }
        let mut order: Vec<_> = r["candidates"]
            .as_array()
            .ok_or("invalid_input")?
            .iter()
            .rev()
            .map(|c| c["modelId"].clone())
            .collect();
        if cfg!(feature = "duplicate") {
            order = vec![order[0].clone(), order[0].clone()];
        }
        if cfg!(feature = "foreign") {
            order = vec![json!("00000000-0000-4000-8000-000000000099")];
        }
        if cfg!(feature = "configured") {
            order = r["configuredOrder"]
                .as_array()
                .ok_or("invalid_input")?
                .clone();
        }
        if cfg!(feature = "empty") {
            order.clear();
        }
        Ok(json!({"requestIdentity":if cfg!(feature="stale"){json!("foreign-snapshot")}else{v["requestIdentity"].clone()},"order":order,"reason":"extension-order"}).to_string())
    }
}
export!(Hostile);
