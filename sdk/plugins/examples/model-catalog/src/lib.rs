wit_bindgen::generate!({path:"../../wit",world:"plugin"});
use serde_json::{Value, json};
struct Catalog;
impl Guest for Catalog {
    fn invoke(method: String, payload: String) -> Result<String, String> {
        if method == "plugin.initialize" {
            return Ok(payload);
        }
        if method.starts_with("plugin.") {
            return Ok(json!({"ready":true}).to_string());
        }
        let input: Value = serde_json::from_str(&payload).map_err(|_| "invalid_input")?;
        match method.as_str() {
            "provider.catalog.request" => Ok(json!({"path":"/catalog"}).to_string()),
            "provider.catalog.parse" => {
                let rows = input["body"]["entries"].as_array().ok_or("invalid_input")?;
                if rows.len() > 256 {
                    return Err("resource_limit".into());
                }
                let models=rows.iter().map(|row|json!({
                    "id":row["key"],"name":row.get("label").unwrap_or(&row["key"]),
                    "contextWindow":row.get("window"),"maxInputTokens":null,"maxOutputTokens":row.get("outputLimit"),
                    "capabilities":{"tools":row.get("tools"),"vision":null,"reasoning":null,"structuredOutput":null,"jsonOutput":null}
                })).collect::<Vec<_>>();
                Ok(json!({"snapshot":input["snapshot"],"models":models,"continuation":{"hasMore":input["body"]["more"]==true,"afterId":input["body"].get("cursor")}}).to_string())
            }
            _ => Err("unsupported".into()),
        }
    }
}
export!(Catalog);
