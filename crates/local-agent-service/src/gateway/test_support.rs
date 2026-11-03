//! Raw forwarding fixture; production creation goes through the local-token protocol.
use super::*;
use local_agent_core::providers::ProviderManager;
use zeroize::Zeroizing;
impl Gateway {
    pub fn prepare(
        &self,
        manager: &ProviderManager,
        agent: &str,
        provider: &Provider,
    ) -> Result<(Provider, Routing), ConfigError> {
        let models = Self::route_models(agent, provider)?;
        let mut routes = self
            .routes
            .lock()
            .map_err(|_| ConfigError::new("busy", "本地路由不可用"))?;
        let existing = routes.iter().find(|(_, r)| {
            r.agent == agent
                && r.models == models
                && fingerprint(&r.provider) == fingerprint(provider)
        });
        let (id, route) = if let Some((id, route)) = existing {
            (id.clone(), route.clone())
        } else {
            if routes.len() >= 500 {
                return Err(ConfigError::new(
                    "capacity",
                    "已保留 500 条路由快照，请先核查备份与留存",
                ));
            }
            let id = uuid::Uuid::new_v4().to_string();
            let credential = manager.credential(
                "Local Agent 本机路由",
                Zeroizing::new(uuid::Uuid::new_v4().simple().to_string()),
            )?;
            let route = Route {
                agent: agent.into(),
                provider: provider.clone(),
                local_credential_ref: credential.credential_ref,
                local_token_owner: None,
                models: models.clone(),
            };
            files::atomic_write(
                &self.directory.join(format!("route-{id}.json")),
                &serde_json::to_vec(&route)
                    .map_err(|_| ConfigError::new("write_failed", "路由记录无效"))?,
                0o600,
            )?;
            routes.insert(id.clone(), route.clone());
            (id, route)
        };
        let client = if agent != "codex" {
            "messages"
        } else {
            "responses"
        };
        let endpoint = format!(
            "http://{}/gateway/{id}{}",
            self.host,
            if agent == "codex" { "/v1" } else { "" }
        );
        let rendered = self.client_provider(&id, &route);
        Ok((
            rendered,
            Routing {
                id,
                client_protocol: client.into(),
                upstream_protocol: provider.protocol.clone(),
                local_endpoint: endpoint,
                models,
            },
        ))
    }
}
