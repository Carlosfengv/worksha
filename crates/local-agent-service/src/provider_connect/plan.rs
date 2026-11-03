use crate::{gateway::capabilities, provider_models::ModelsRequest};
use local_agent_core::providers::{ConfigError, catalog, validate_endpoint};
use std::collections::BTreeSet;

pub(super) fn candidates(endpoint: &str) -> Result<Vec<ModelsRequest>, ConfigError> {
    if endpoint.len() > 2048 {
        return Err(ConfigError::new("invalid_provider", "服务地址过长"));
    }
    let url = validate_endpoint(endpoint.trim())?;
    let path = url.path().trim_end_matches('/');
    let vendor = catalog::builtin()
        .presets()
        .iter()
        .find(|preset| {
            preset
                .endpoints
                .values()
                .any(|base| url::Url::parse(base).is_ok_and(|base| base.origin() == url.origin()))
        })
        .map_or("custom", |preset| preset.id.as_str());
    let mut plans = vec![];
    let mut seen = BTreeSet::new();
    let mut push = |base: String, protocol: &str| {
        let wire = capabilities::builtin()
            .resolve(protocol)
            .expect("builtin protocol");
        if plans.len() < 8 && seen.insert((wire.endpoint(&base), protocol.to_string())) {
            plans.push(ModelsRequest {
                endpoint: base,
                protocol: protocol.into(),
                vendor: vendor.into(),
                credential_ref: None,
            });
        }
    };
    let mut base = url.clone();
    let suffix = [
        ("/chat/completions", "chat-completions"),
        ("/responses", "responses"),
        ("/messages", "messages"),
    ]
    .into_iter()
    .find(|(suffix, _)| path.ends_with(suffix));
    if let Some((suffix, protocol)) = suffix {
        base.set_path(path.strip_suffix(suffix).unwrap());
        // A complete /messages URL without /v1 must retain the exact supplied path.
        push(
            if protocol == "messages" && !base.path().ends_with("/v1") {
                url.to_string().trim_end_matches('/').into()
            } else {
                base.to_string().trim_end_matches('/').into()
            },
            protocol,
        );
    } else if let Some(path) = path.strip_suffix("/models") {
        base.set_path(path);
    }
    if path.is_empty() {
        for preset in catalog::builtin().presets() {
            let preferred = preset.endpoints.get(&preset.protocol);
            for (protocol, endpoint) in preferred
                .map(|endpoint| (&preset.protocol, endpoint))
                .into_iter()
                .chain(preset.endpoints.iter())
            {
                if url::Url::parse(endpoint).is_ok_and(|base| base.origin() == url.origin()) {
                    push(endpoint.clone(), protocol);
                }
            }
        }
    }
    let base = base.to_string().trim_end_matches('/').to_string();
    for protocol in ["chat-completions", "responses", "messages"] {
        push(base.clone(), protocol);
    }
    if !base.ends_with("/v1") {
        for protocol in ["chat-completions", "responses", "messages"] {
            push(format!("{base}/v1"), protocol);
        }
    }
    Ok(plans)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plans_preserve_origin_prefix_and_complete_urls_without_guessing_hosts() {
        for endpoint in [
            "https://fixture.invalid/proxy/v1/chat/completions",
            "https://fixture.invalid/proxy/v1/responses",
            "https://fixture.invalid/proxy/v1/messages",
            "https://fixture.invalid/proxy/messages",
            "http://127.0.0.1:1234/v1/models",
        ] {
            let plans = candidates(endpoint).unwrap();
            let original = url::Url::parse(endpoint).unwrap();
            assert!(plans.len() <= 8);
            for p in &plans {
                let wire = capabilities::builtin().resolve(&p.protocol).unwrap();
                let target = url::Url::parse(&wire.endpoint(&p.endpoint)).unwrap();
                assert_eq!(target.origin(), original.origin());
                if original.path().starts_with("/proxy/") {
                    assert!(target.path().starts_with("/proxy/"));
                }
            }
            if !endpoint.ends_with("/models") {
                let first = &plans[0];
                assert_eq!(
                    capabilities::builtin()
                        .resolve(&first.protocol)
                        .unwrap()
                        .endpoint(&first.endpoint),
                    endpoint
                );
            }
        }
        assert_eq!(
            candidates("https://api.deepseek.com").unwrap()[0].protocol,
            "chat-completions"
        );
        for bad in [
            "https://key@host.invalid",
            "http://remote.invalid",
            "https://host.invalid?key=x",
            "file:///tmp/models",
        ] {
            assert!(candidates(bad).is_err());
        }
    }
}
