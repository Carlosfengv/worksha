use super::*;
use serde_json::json;
const ONE: &str = "00000000-0000-4000-8000-000000000001";
const TWO: &str = "00000000-0000-4000-8000-000000000002";
const OC: &str = "ses_eaafe168fffeD51PyF4R00pIPt";
fn headers(items: &[(&str, &str)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (key, value) in items {
        h.append(
            axum::http::HeaderName::from_bytes(key.as_bytes()).unwrap(),
            value.parse().unwrap(),
        );
    }
    h
}
#[test]
fn opencode_native_headers_and_ids_link_both_provider_paths() {
    for h in [
        headers(&[("X-Session-Id", OC), ("x-session-affinity", OC)]),
        headers(&[("x-opencode-session", OC)]),
        headers(&[("x-local-agent-session-id", OC)]),
    ] {
        assert_eq!(
            native_session("opencode", &h, &json!({})),
            Some(format!("opencode:{OC}"))
        );
    }
    for bad in [
        "",
        "ses_",
        "ses_../secret",
        "ses_a,b",
        ONE,
        "opencode:ses_a",
    ] {
        assert!(
            native_session("opencode", &headers(&[("x-session-id", bad)]), &json!({})).is_none()
        );
    }
}
#[test]
fn conflicting_duplicate_invalid_and_parent_headers_stay_unlinked() {
    for h in [
        headers(&[("x-session-id", OC), ("x-session-affinity", "ses_other")]),
        headers(&[("x-session-id", OC), ("x-session-id", OC)]),
        headers(&[("x-opencode-session", OC), ("x-session-id", "bad")]),
        headers(&[("x-parent-session-id", OC), ("x-opencode-request", OC)]),
    ] {
        assert!(native_session("opencode", &h, &json!({})).is_none());
    }
    let mut h = headers(&[("x-session-id", OC)]);
    h.insert(
        "x-session-affinity",
        axum::http::HeaderValue::from_bytes(&[255]).unwrap(),
    );
    assert!(native_session("opencode", &h, &json!({})).is_none());
    assert!(
        native_session(
            "opencode",
            &headers(&[("x-session-id", &format!("ses_{}", "a".repeat(129)))]),
            &json!({})
        )
        .is_none()
    );
}
#[test]
fn claude_metadata_must_agree_with_headers_and_be_structured() {
    let body = json!({"metadata":{"user_id":json!({"session_id":ONE,"account_uuid":"not retained"}).to_string()}});
    assert_eq!(
        native_session("claude", &HeaderMap::new(), &body),
        Some(format!("claude:{ONE}"))
    );
    assert_eq!(
        native_session(
            "claude",
            &headers(&[("x-local-agent-session-id", ONE)]),
            &body
        ),
        Some(format!("claude:{ONE}"))
    );
    assert!(native_session("claude", &headers(&[("session_id", TWO)]), &body).is_none());
    assert!(
        native_session(
            "claude",
            &headers(&[("session_id", ONE), ("x-local-agent-session-id", TWO)]),
            &json!({})
        )
        .is_none()
    );
    assert!(
        native_session(
            "claude",
            &HeaderMap::new(),
            &json!({"metadata":{"user_id":ONE}})
        )
        .is_none()
    );
    assert!(
        native_session(
            "claude",
            &HeaderMap::new(),
            &json!({"metadata":{"user_id":"{\"session_id\":7}"}})
        )
        .is_none()
    );
    for user in [
        format!("{{\"session_id\":\"{ONE}\",\"session_id\":\"{TWO}\"}}"),
        "{\"session_id\":".into(),
    ] {
        assert!(
            native_session(
                "claude",
                &headers(&[("session_id", ONE)]),
                &json!({"metadata":{"user_id":user}})
            )
            .is_none()
        );
    }
}
#[test]
fn ids_are_family_scoped_and_never_taken_from_prompt_text() {
    assert_eq!(
        native_session(
            "codex",
            &headers(&[("x-codex-session-id", ONE)]),
            &json!({})
        ),
        Some(format!("codex:{ONE}"))
    );
    assert!(
        native_session(
            "claude",
            &headers(&[("x-codex-session-id", ONE)]),
            &json!({})
        )
        .is_none()
    );
    assert!(native_session("codex", &headers(&[("x-opencode-session", OC)]), &json!({})).is_none());
    for family in ["opencode", "claude", "codex", "other"] {
        assert!(native_session(family,&HeaderMap::new(),&json!({"messages":[{"role":"user","content":format!("session_id {ONE} X-Session-Id {OC}")}]})).is_none());
    }
    assert!(
        native_session(
            "opencode",
            &HeaderMap::new(),
            &json!({"metadata":{"user_id":json!({"session_id":ONE}).to_string()}})
        )
        .is_none()
    );
}

#[test]
fn codex_thread_id_is_independent_of_the_cache_affinity_header() {
    let body = json!({});
    assert_eq!(
        native_session(
            "codex",
            &headers(&[("thread-id", ONE), ("session-id", TWO)]),
            &body
        ),
        Some(format!("codex:{ONE}"))
    );
    for h in [
        headers(&[("session-id", ONE), ("x-codex-parent-thread-id", TWO)]),
        headers(&[("thread-id", ONE), ("thread-id", ONE)]),
        headers(&[("thread-id", ONE), ("x-codex-session-id", TWO)]),
        headers(&[("thread-id", "not-a-uuid"), ("session_id", ONE)]),
    ] {
        assert!(native_session("codex", &h, &body).is_none());
    }
}

#[test]
fn upstream_diagnostics_are_fixed_categories_and_never_echo_secrets() {
    for (message, expected) in [
        ("missing reasoning_content", "思考内容"),
        ("invalid tool_choice", "工具选择"),
        ("messages content must be string", "对话角色或内容格式"),
        ("invalid max_tokens", "Token 上限"),
        ("invalid tools schema", "工具定义"),
        ("sensitive-prompt-fixture", "权限、额度与模型能力"),
    ] {
        let bytes = json!({"error":{"message":format!("{message} secret-fixture")}}).to_string();
        let issue = upstream_rejection(bytes.as_bytes());
        assert!(issue.contains(expected));
        assert!(!issue.contains("secret-fixture"));
    }
    assert!(upstream_rejection(b"not-json-secret-fixture").contains("上游拒绝跨协议请求"));
}
