//! Product identity is separate from adapter families and shared configuration keys.
mod catalog;
pub mod evidence;
pub use catalog::{AgentDirectory, ProductDescriptor, SourceDescriptor, builtin_directory};
pub use evidence::{claude_client_evidence, codex_request_product, native_client};

// Compatibility view for existing built-in protocol/configuration adapters.
pub static PRODUCTS: std::sync::LazyLock<Vec<(&str, &str, &str, &str)>> =
    std::sync::LazyLock::new(|| {
        builtin_directory()
            .products()
            .iter()
            .map(|p| {
                (
                    p.id.as_str(),
                    p.name.as_str(),
                    p.family.as_str(),
                    p.config_id.as_str(),
                )
            })
            .collect()
    });

pub fn family(id: &str) -> Option<&'static str> {
    builtin_directory().family(id)
}

pub fn valid_filter(id: &str) -> bool {
    builtin_directory().valid_filter(id)
}

pub fn matches(filter: Option<&str>, family: &str, product: Option<&str>) -> bool {
    builtin_directory().matches(filter, family, product)
}
pub fn group(family: &str, product: Option<&str>) -> String {
    builtin_directory().group(family, product)
}
pub fn matches_group(filter: Option<&str>, group: &str) -> bool {
    builtin_directory().matches_group(filter, group)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_sdk_modes_have_a_reason_without_claiming_cli_or_desktop() {
        for marker in ["sdk-cli", "sdk-ts", "sdk-py"] {
            let row = json!({"entrypoint":marker});
            assert_eq!(native_client("claude", &row), Some(None));
            assert_eq!(claude_client_evidence(&row), Some("sdk_client"));
        }
        assert_eq!(
            claude_client_evidence(&json!({"entrypoint":"future-host"})),
            Some("unrecognized_client")
        );
        assert_eq!(
            claude_client_evidence(&json!({"message":{"entrypoint":"cli"}})),
            None
        );
    }
    #[test]
    fn claude_aliases_require_consistent_explicit_identity() {
        let row = json!({"entrypoint":"cli","entryPoint":"claude-cli"});
        assert_eq!(
            native_client("claude", &row),
            Some(Some("claude-code-cli".into()))
        );
        assert_eq!(claude_client_evidence(&row), Some("native_record"));
        for other in [json!("claude-desktop"), json!("sdk-cli"), json!(null)] {
            let row = json!({"entrypoint":"cli","entryPoint":other});
            assert_eq!(native_client("claude", &row), Some(None));
            assert_eq!(claude_client_evidence(&row), Some("conflicting_evidence"));
        }
    }

    #[test]
    fn gateway_client_headers_are_scoped_and_conflicts_stay_unknown() {
        assert_eq!(
            codex_request_product(None, Some("codex_work_desktop"), None).as_deref(),
            Some("codex-desktop")
        );
        assert_eq!(
            codex_request_product(None, None, Some("codex_cli_rs/1.0 (Mac OS) ")).as_deref(),
            Some("codex-cli")
        );
        assert_eq!(codex_request_product(None, None, Some("codex/1.0")), None);
        assert_eq!(
            codex_request_product(None, Some("custom_client"), Some("codex_cli_rs/1.0")),
            None
        );
        assert_eq!(
            codex_request_product(None, Some("codex_work_desktop"), Some("codex_cli_rs/1.0")),
            None
        );
        assert_eq!(
            codex_request_product(Some("codex-cli"), Some("codex_work_desktop"), None),
            None
        );
        assert_eq!(
            codex_request_product(Some("codex-cli"), None, None).as_deref(),
            Some("codex-cli")
        );
    }

    #[test]
    fn products_do_not_inherit_unknown_family_records() {
        assert_eq!(PRODUCTS.len(), 6);
        assert!(!matches(Some("codex-cli"), "codex", None));
        assert!(!matches(Some("codex-desktop"), "codex", Some("codex-cli")));
        assert!(matches(Some("codex"), "codex", Some("codex-desktop")));
        assert!(matches(Some("unknown"), "codex", None));
        assert!(matches(Some("unknown"), "opencode", Some("opencode")));
        assert_eq!(group("opencode", Some("opencode")), "opencode-unknown");
        assert!(!matches(Some("opencode-cli"), "opencode", Some("opencode")));
        assert!(matches(
            Some("opencode"),
            "opencode",
            Some("opencode-desktop")
        ));
    }

    #[test]
    fn verified_tui_alias_is_explicit_per_request_and_never_a_generic_codex_guess() {
        assert_eq!(
            codex_request_product(None, Some("codex-tui"), None).as_deref(),
            Some("codex-cli")
        );
        assert_eq!(
            codex_request_product(None, None, Some("codex-tui/0.163.0-alpha.10.2")).as_deref(),
            Some("codex-cli")
        );
        assert_eq!(
            codex_request_product(None, Some("codex-tui"), Some("codex_work_desktop/1.0")),
            None
        );
        assert_eq!(
            codex_request_product(None, Some("generic-tui"), Some("codex-tui/1.0")),
            None
        );
    }

    #[test]
    fn native_metadata_is_explicit_and_unknown_clears_a_segment() {
        assert_eq!(
            native_client(
                "codex",
                &json!({"type":"session_meta","payload":{"originator":"codex_work_desktop","source":"vscode"}})
            ),
            Some(Some("codex-desktop".into()))
        );
        assert_eq!(
            native_client(
                "codex",
                &json!({"type":"session_meta","payload":{"source":"vscode"}})
            ),
            Some(None)
        );
        assert_eq!(
            native_client("claude", &json!({"entrypoint":"sdk-cli"})),
            Some(None)
        );
        assert_eq!(
            native_client("claude", &json!({"entrypoint":"claude-desktop-3p"})),
            Some(Some("claude-code-desktop".into()))
        );
        assert_eq!(
            native_client("claude", &json!({"message":{"content":"entrypoint cli"}})),
            None
        );
    }
}
