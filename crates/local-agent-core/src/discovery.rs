use crate::AgentDiscovery;

pub fn discover_agents() -> Vec<AgentDiscovery> {
    discover_with_directory(crate::agents::builtin_directory())
}

pub(crate) fn discover_with_directory(
    directory: &crate::agents::AgentDirectory,
) -> Vec<AgentDiscovery> {
    let home = dirs::home_dir();
    directory
        .products()
        .iter()
        .map(|product| {
            let path = product
                .locations
                .iter()
                .filter_map(|location| {
                    if let Some(relative) = location.strip_prefix("~/") {
                        home.as_ref().map(|home| home.join(relative))
                    } else {
                        Some(std::path::PathBuf::from(location))
                    }
                })
                .find(|path| {
                    if product.id == "opencode-cli" {
                        path.is_file()
                    } else {
                        path.is_dir()
                    }
                });
            // Discovery checks metadata of these known locations only; no session content is read.
            let found = path.is_some();
            AgentDiscovery {
                id: product.id.clone(),
                family: product.family.clone(),
                config_id: product.config_id.clone(),
                name: product.name.clone(),
                found,
                location: path.map(|p| p.to_string_lossy().into_owned()),
                connected: false,
            }
        })
        .collect()
}
