wit_bindgen::generate!({path:"../../wit",world:"plugin"});
#[allow(dead_code)]
#[path = "../../../support.rs"]
mod support;

use serde_json::{Value, json};

fn dispatch(method: &str, payload: &str) -> Result<String, String> {
    let input: Value = serde_json::from_str(payload).map_err(|_| "invalid_input")?;
    let output = match method {
        "provider.catalog.request" => json!({"path":"/catalog"}),
        "provider.catalog.parse" => {
            let rows = input["body"]["entries"].as_array().ok_or("invalid_input")?;
            if rows.len() > 256 {
                return Err("resource_limit".into());
            }
            let models = rows.iter().map(|row| json!({
                "id":row["key"], "name":row.get("label").unwrap_or(&row["key"]),
                "contextWindow":row.get("window"), "maxInputTokens":null, "maxOutputTokens":row.get("outputLimit"),
                "capabilities":{"tools":null,"vision":null,"reasoning":null,"structuredOutput":null,"jsonOutput":null}
            })).collect::<Vec<_>>();
            json!({"snapshot":input["snapshot"],"models":models,"continuation":{"hasMore":false,"afterId":null}})
        }
        "account.quota" => {
            let request =
                json!({"contextId":input["contextId"],"connectionRef":input["connectionRef"]});
            let response =
                local_agent::plugin::host::call("connection.request", &request.to_string())?;
            let response: Value = serde_json::from_str(&response).map_err(|_| "invalid_input")?;
            let body: Value =
                serde_json::from_str(response["body"].as_str().ok_or("invalid_input")?)
                    .map_err(|_| "invalid_input")?;
            json!({"remaining":body["balance"],"limit":body["limit"],"unit":body["unit"],"observedAt":input["observedAt"],"expiresAt":body["expiresAt"]})
        }
        "protocol.open" => {
            json!({"bytes":serde_json::to_vec(&input["request"]).map_err(|_| "invalid_input")? })
        }
        "protocol.feed" => json!({"bytes":input["bytes"],"complete":false}),
        "protocol.finish" => json!({"bytes":[],"complete":true}),
        _ => return Err("unsupported".into()),
    };
    Ok(output.to_string())
}

define_plugin!(UnifiedProvider, dispatch);
