use super::*;
use crate::provider_models::read_models;
use serde_json::{Value, json};

fn documented(endpoint: &str, protocol: &str, rows: Value, at: &str) -> Value {
    let mut models = BTreeMap::new();
    read_models(&json!({"data": rows}), &mut models, None).unwrap();
    supplement(
        &ModelsRequest {
            endpoint: endpoint.into(),
            protocol: protocol.into(),
            vendor: "custom".into(),
            credential_ref: None,
        },
        &mut models,
        DateTime::parse_from_rfc3339(at)
            .unwrap()
            .with_timezone(&Utc),
    );
    serde_json::to_value(models).unwrap()
}
fn known() -> Value {
    json!([
        {"id":"deepseek-flash", "context_window":1048576},
        {"id":"deepseek-v4-pro", "context_window":1048576},
        {"id":"deepseek-v4-pro[1M]"}, {"id":"deepseek-future"}
    ])
}

#[test]
fn outputs_are_protocol_specific_and_never_conflate_json_with_schema() {
    for (endpoint, protocol, schema, json_mode) in [
        (
            "https://api.deepseek.com",
            "chat-completions",
            false,
            json!(true),
        ),
        (
            "https://api.deepseek.com/v1/",
            "responses",
            true,
            json!(true),
        ),
        (
            "https://api.deepseek.com/anthropic/v1",
            "messages",
            false,
            Value::Null,
        ),
    ] {
        let result = documented(endpoint, protocol, known(), CHECKED_AT);
        for id in ["deepseek-flash", "deepseek-v4-pro"] {
            let m = &result[id];
            assert_eq!(m["capabilities"]["tools"], true);
            assert_eq!(m["capabilities"]["structuredOutput"], schema);
            assert_eq!(m["capabilities"]["jsonOutput"], json_mode);
            assert_eq!(m["documentation"][0]["url"], PRICING);
            assert_eq!(m["documentation"][0]["checkedAt"], CHECKED_AT);
            assert_eq!(m["documentation"][0]["expiresAt"], EXPIRES_AT);
            assert_eq!(m["contextWindow"], 1048576);
            assert_eq!(m["contextAlias"].is_object(), protocol == "messages");
            if protocol == "messages" {
                assert_eq!(m["contextAlias"]["contextWindow"], 1_000_000);
                assert_eq!(m["contextAlias"]["upstreamContextWindow"], 1_048_576);
            }
        }
        for id in ["deepseek-v4-pro[1M]", "deepseek-future"] {
            assert!(result[id]["capabilities"]["tools"].is_null());
            assert!(result[id]["documentation"].is_null());
            assert!(result[id]["contextAlias"].is_null());
        }
    }
}

#[test]
fn supplements_require_exact_origin_path_model_and_current_evidence() {
    for (endpoint, protocol) in [
        ("https://api.deepseek.com.attacker.invalid", "responses"),
        ("http://api.deepseek.com", "responses"),
        ("https://api.deepseek.com:444", "responses"),
        ("https://api.deepseek.com/proxy", "responses"),
        ("https://api.deepseek.com/beta", "chat-completions"),
        ("https://api.deepseek.com", "messages"),
        ("https://api.deepseek.com/anthropic", "responses"),
        ("https://secret-fixture@api.deepseek.com", "responses"),
        ("https://api.deepseek.com?key=secret-fixture", "responses"),
        ("https://api.deepseek.com#fragment", "responses"),
    ] {
        let result = documented(endpoint, protocol, known(), CHECKED_AT);
        assert!(
            result["deepseek-flash"]["capabilities"]["tools"].is_null(),
            "{endpoint}"
        );
        assert!(result["deepseek-flash"]["contextAlias"].is_null());
        assert!(!result.to_string().contains("secret-fixture"));
    }
    for at in ["2026-10-25T23:59:59Z", EXPIRES_AT, "2027-01-01T00:00:00Z"] {
        let result = documented(
            "https://api.deepseek.com/anthropic",
            "messages",
            known(),
            at,
        );
        assert!(result["deepseek-flash"]["capabilities"]["tools"].is_null());
        assert!(result["deepseek-flash"]["contextAlias"].is_null());
    }
}

#[test]
fn explicit_catalog_values_and_smaller_context_take_precedence() {
    let result = documented(
        "https://api.deepseek.com/anthropic",
        "messages",
        json!([
            {"id":"deepseek-flash", "context_window":65536,
             "capabilities":{"tool_use":{"supported":false}, "structured_outputs":{"supported":true}, "json_output":{"supported":false}}},
            {"id":"deepseek-v4-pro"}
        ]),
        CHECKED_AT,
    );
    let flash = &result["deepseek-flash"];
    assert_eq!(flash["capabilities"]["tools"], false);
    assert_eq!(flash["capabilities"]["structuredOutput"], true);
    assert_eq!(flash["capabilities"]["jsonOutput"], false);
    assert!(flash["documentation"].is_null());
    assert!(flash["contextAlias"].is_null());
    assert!(result["deepseek-v4-pro"]["contextAlias"].is_null());
}
