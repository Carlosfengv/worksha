use axum::http::HeaderMap;
use local_agent_core::plugin::{
    BuiltinPlugin, CapabilityKey, PluginDescriptor, PluginError, PluginHost, PluginResources,
    builtin::{CapabilityKind, CapabilityMetadata, ResourceScope},
};
use std::sync::Arc;

struct Client {
    id: &'static str,
    name: &'static str,
    marker: &'static str,
}

const CLIENTS: [Client; 2] = [
    Client {
        id: "opencode-cli",
        name: "OpenCode CLI",
        marker: "cli",
    },
    Client {
        id: "opencode-desktop",
        name: "OpenCode Desktop",
        marker: "desktop",
    },
];

fn capability(id: &'static str) -> CapabilityKey {
    CapabilityKey {
        name: "agent.client-profile",
        version: 1,
        instance: id,
    }
}

impl BuiltinPlugin for Client {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            id: self.id,
            version: "1.0.0",
            provides: vec![capability(self.id)],
            requires: vec![],
        }
    }

    fn display_name(&self) -> String {
        self.name.into()
    }

    fn capability_metadata(&self, _: &CapabilityKey) -> CapabilityMetadata {
        CapabilityMetadata {
            kind: CapabilityKind::Query,
            resource_scope: ResourceScope::Application,
        }
    }

    fn prepare(&self) -> Result<PluginResources, PluginError> {
        Ok(PluginResources::default())
    }
}

pub(super) fn registrations() -> impl Iterator<Item = Arc<dyn BuiltinPlugin>> {
    CLIENTS
        .into_iter()
        .map(|client| Arc::new(client) as Arc<dyn BuiltinPlugin>)
}

pub(crate) fn request_product(host: &PluginHost, headers: &HeaderMap) -> Option<String> {
    let marker = reported_client(headers)?;
    let client = CLIENTS.iter().find(|client| client.marker == marker)?;
    host.acquire(&capability(client.id)).ok()?;
    Some(client.id.into())
}

fn reported_client(headers: &HeaderMap) -> Option<&str> {
    let single = |name: &str| {
        let mut values = headers.get_all(name).iter();
        let value = values.next().map(|value| value.to_str()).transpose().ok()?;
        values.next().is_none().then_some(value)
    };
    let header = single("x-opencode-client")?;
    // OpenCode 1.x may send only opencode/<version>. 2.x sends
    // opencode/<channel>/<version>/<client> for custom providers as well.
    let ua = single("user-agent")?.and_then(|value| {
        let mut parts = value.split('/');
        let (name, channel, version, client) =
            (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
        (name == "opencode"
            && [channel, version, client].iter().all(|value| {
                !value.is_empty()
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b".-_".contains(&b))
            })
            && parts.next().is_none())
        .then_some(client)
    });
    if header.zip(ua).is_some_and(|(a, b)| a != b) {
        return None;
    }
    header.or(ua)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_host::ReadonlyRuntime;

    #[test]
    fn client_plugins_use_explicit_evidence_and_stop_independently() {
        let runtime = ReadonlyRuntime::builtin().unwrap();
        let product = |headers: &HeaderMap| request_product(&runtime.host, headers);
        let mut headers = HeaderMap::new();
        headers.insert("user-agent", "opencode/1.18.13".parse().unwrap());
        assert_eq!(product(&headers), None);

        headers.insert("x-opencode-client", "cli".parse().unwrap());
        assert_eq!(product(&headers).as_deref(), Some("opencode-cli"));

        headers.insert(
            "user-agent",
            "opencode/latest/2.2.6/desktop".parse().unwrap(),
        );
        assert_eq!(product(&headers), None);
        headers.remove("x-opencode-client");
        assert_eq!(product(&headers).as_deref(), Some("opencode-desktop"));

        headers.append("x-opencode-client", "desktop".parse().unwrap());
        headers.append("x-opencode-client", "desktop".parse().unwrap());
        assert_eq!(product(&headers), None);

        runtime.host.stop("opencode-desktop").unwrap();
        headers.remove("x-opencode-client");
        assert_eq!(product(&headers), None);
        headers.insert("user-agent", "opencode/latest/2.2.6/cli".parse().unwrap());
        assert_eq!(product(&headers).as_deref(), Some("opencode-cli"));
    }
}
