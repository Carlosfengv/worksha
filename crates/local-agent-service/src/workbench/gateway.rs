use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Scope {
    Current,
    Other,
    External,
    Unknown,
}

pub(super) fn classify(endpoint: Option<&str>, managed: bool, host: &str) -> Scope {
    let Some(endpoint) = endpoint else {
        return if managed {
            Scope::Unknown
        } else {
            Scope::External
        };
    };
    let Ok(url) = reqwest::Url::parse(endpoint) else {
        return Scope::Unknown;
    };
    let local = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    if !(managed || local && url.path().starts_with("/gateway/")) {
        return Scope::External;
    }
    let path = url.path().trim_end_matches('/');
    let binding = path
        .strip_prefix("/gateway/")
        .map(|path| path.strip_suffix("/v1").unwrap_or(path));
    if url.scheme() != "http"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || binding.is_none_or(|id| uuid::Uuid::parse_str(id).is_err())
    {
        return Scope::Unknown;
    }
    if url.host_str() == Some("127.0.0.1")
        && url
            .port_or_known_default()
            .map(|port| format!("127.0.0.1:{port}"))
            .as_deref()
            == Some(host)
    {
        Scope::Current
    } else {
        Scope::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn claude_and_opencode_share_listener_checks_without_changing_models() {
        for suffix in ["", "/", "/v1", "/v1/"] {
            let url = format!(
                "http://127.0.0.1:4000/gateway/00000000-0000-4000-8000-000000000001{suffix}"
            );
            assert_eq!(classify(Some(&url), true, "127.0.0.1:4000"), Scope::Current);
            assert_eq!(classify(Some(&url), true, "127.0.0.1:4001"), Scope::Other);
        }
        assert_eq!(classify(None, true, "127.0.0.1:4000"), Scope::Unknown);
        assert_eq!(classify(None, false, "127.0.0.1:4000"), Scope::External);
    }
}
