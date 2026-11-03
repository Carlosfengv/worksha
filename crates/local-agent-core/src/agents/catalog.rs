//! Immutable trusted composition metadata. Runtime packages are not loaded here.
use serde::Deserialize;
use std::sync::LazyLock;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourceDescriptor {
    pub id: String,
    pub name: String,
    pub shared: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProductDescriptor {
    pub id: String,
    pub name: String,
    pub family: String,
    pub config_id: String,
    pub locations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentDirectory {
    sources: Vec<SourceDescriptor>,
    products: Vec<ProductDescriptor>,
}

impl AgentDirectory {
    pub fn new(
        sources: Vec<SourceDescriptor>,
        products: Vec<ProductDescriptor>,
    ) -> Result<Self, &'static str> {
        let mut ids = std::collections::HashSet::new();
        let valid = |s: &str| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        };
        for source in &sources {
            if !valid(&source.id)
                || source.id == "unknown"
                || source.id.ends_with("-unknown")
                || source.name.is_empty()
                || !ids.insert(source.id.as_str())
            {
                return Err("来源目录描述无效或重复");
            }
        }
        ids.clear();
        for product in &products {
            if !valid(&product.id)
                || product.id == "unknown"
                || product.id.ends_with("-unknown")
                || product.name.is_empty()
                || !valid(&product.config_id)
                || !sources.iter().any(|s| s.id == product.family)
                || !ids.insert(product.id.as_str())
            {
                return Err("产品目录描述无效或重复");
            }
        }
        if sources
            .iter()
            .any(|s| !products.iter().any(|p| p.family == s.id))
        {
            return Err("来源缺少产品描述");
        }
        Ok(Self { sources, products })
    }

    pub fn sources(&self) -> &[SourceDescriptor] {
        &self.sources
    }
    pub fn products(&self) -> &[ProductDescriptor] {
        &self.products
    }
    pub fn source(&self, id: &str) -> Option<&SourceDescriptor> {
        self.sources.iter().find(|s| s.id == id)
    }
    pub fn family(&self, id: &str) -> Option<&str> {
        self.products
            .iter()
            .find(|p| p.id == id)
            .map(|p| p.family.as_str())
            .or_else(|| {
                self.sources
                    .iter()
                    .find(|s| {
                        s.id == id || s.shared && id.strip_suffix("-unknown") == Some(s.id.as_str())
                    })
                    .map(|s| s.id.as_str())
            })
    }
    pub fn valid_filter(&self, id: &str) -> bool {
        id == "unknown" || self.family(id).is_some()
    }
    pub fn matches(&self, filter: Option<&str>, family: &str, product: Option<&str>) -> bool {
        if filter.is_none() || filter == Some(family) {
            return true;
        }
        let product = product.or_else(|| self.single_product(family));
        match filter {
            None => true,
            Some("unknown") => product.is_none(),
            Some(id) if id.strip_suffix("-unknown") == Some(family) => product.is_none(),
            Some(id) if id == family => true,
            Some(id) => product == Some(id),
        }
    }
    pub fn group(&self, family: &str, product: Option<&str>) -> String {
        product
            .or_else(|| self.single_product(family))
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{family}-unknown"))
    }
    pub fn matches_group(&self, filter: Option<&str>, group: &str) -> bool {
        if filter.is_none() || filter == Some(group) {
            return true;
        }
        let family = self
            .family(group)
            .or_else(|| group.strip_suffix("-unknown"))
            .unwrap_or(group);
        let product = self
            .products
            .iter()
            .find(|p| p.id == group)
            .map(|p| p.id.as_str());
        self.matches(filter, family, product)
    }
    pub fn single_product(&self, family: &str) -> Option<&str> {
        let source = self.source(family)?;
        (!source.shared)
            .then(|| {
                self.products
                    .iter()
                    .find(|p| p.family == family)
                    .map(|p| p.id.as_str())
            })
            .flatten()
    }
}

pub fn builtin_directory() -> &'static AgentDirectory {
    static DIRECTORY: LazyLock<AgentDirectory> = LazyLock::new(|| {
        let data: AgentDirectory =
            serde_json::from_str(include_str!("../../../../resources/agent-catalog.json"))
                .expect("valid compiled agent catalog");
        AgentDirectory::new(data.sources, data.products).expect("valid built-in agent identities")
    });
    &DIRECTORY
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn descriptions_reject_duplicate_products_and_orphaned_families() {
        let directory = builtin_directory();
        let mut products = directory.products().to_vec();
        products.push(products[0].clone());
        assert!(AgentDirectory::new(directory.sources().to_vec(), products).is_err());
        let mut products = directory.products().to_vec();
        products[0].family = "not-registered".into();
        assert!(AgentDirectory::new(directory.sources().to_vec(), products).is_err());
    }
    #[test]
    fn a_registered_shared_family_keeps_unknown_and_product_filters_separate() {
        let directory = AgentDirectory::new(
            vec![SourceDescriptor {
                id: "fixture".into(),
                name: "Fixture".into(),
                shared: true,
            }],
            ["fixture-cli", "fixture-desktop"]
                .into_iter()
                .map(|id| ProductDescriptor {
                    id: id.into(),
                    name: id.into(),
                    family: "fixture".into(),
                    config_id: "fixture".into(),
                    locations: vec![],
                })
                .collect(),
        )
        .unwrap();
        assert_eq!(directory.family("fixture-desktop"), Some("fixture"));
        assert_eq!(directory.family("fixture-unknown"), Some("fixture"));
        assert!(directory.matches_group(Some("fixture"), "fixture-cli"));
        assert!(!directory.matches_group(Some("fixture-cli"), "fixture-unknown"));
        assert!(directory.matches_group(Some("unknown"), "fixture-unknown"));
    }
}
