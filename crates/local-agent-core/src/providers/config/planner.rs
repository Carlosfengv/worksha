//! Trusted format execution captures private native context; the shared request is redacted.
use super::NativeConfigAdapter;
use crate::{
    Provider,
    domain::config as d,
    providers::{ConfigError, files, fingerprint},
};
use d::ConfigPlanner;
use std::path::Path;

struct NativePlanner<'a> {
    adapter: &'a dyn NativeConfigAdapter,
    before: Option<&'a [u8]>,
    provider: Option<&'a Provider>,
    helper: &'a Path,
}
impl NativePlanner<'_> {
    fn request(&self) -> d::ConfigRequest {
        d::ConfigRequest {
            target_revision: self
                .before
                .map(files::hash)
                .unwrap_or_else(|| "absent".into()),
            intent: d::ConfigIntent::NativeRoute {
                agent_id: self.adapter.id().into(),
                binding: self.provider.map(|p| d::NativeBinding {
                    identity: fingerprint(p),
                    endpoint: p.endpoint.clone(),
                    model: p.model.clone(),
                }),
            },
        }
    }
}
fn error(error: d::PlanError) -> ConfigError {
    match error {
        d::PlanError::StaleSnapshot => {
            ConfigError::new("conflict", "配置规划范围已改变，请重新预览")
        }
        d::PlanError::Unsupported => ConfigError::new("unsupported", "此配置规划方式尚不支持"),
        _ => ConfigError::new("invalid_config", "配置候选超出宿主规划范围，原文件未修改"),
    }
}
impl d::ConfigPlanner for NativePlanner<'_> {
    type Error = ConfigError;
    fn propose(&self, request: &d::ConfigRequest) -> Result<d::ConfigWork, ConfigError> {
        request.validate().map_err(error)?;
        if request != &self.request() {
            return Err(error(d::PlanError::StaleSnapshot));
        }
        let bytes = self
            .adapter
            .render(self.before, self.provider, self.helper)?;
        Ok(d::ConfigWork::Ready(d::ConfigProposal {
            request_identity: request.identity(),
            output: d::ConfigOutput::NativeDocument(bytes),
        }))
    }
    fn inspect_result(
        &self,
        request: &d::ConfigRequest,
        result: d::ConfigProposal,
    ) -> Result<d::ConfigProposal, ConfigError> {
        if request != &self.request() {
            return Err(error(d::PlanError::StaleSnapshot));
        }
        d::inspect_result(request, result).map_err(error)
    }
}
pub(super) fn render(
    adapter: &dyn NativeConfigAdapter,
    before: Option<&[u8]>,
    provider: Option<&Provider>,
    helper: &Path,
) -> Result<Vec<u8>, ConfigError> {
    let planner = NativePlanner {
        adapter,
        before,
        provider,
        helper,
    };
    let request = planner.request();
    let d::ConfigWork::Ready(result) = planner.propose(&request)? else {
        return Err(error(d::PlanError::Unsupported));
    };
    match planner.inspect_result(&request, result)?.output {
        d::ConfigOutput::NativeDocument(bytes) => Ok(bytes),
        d::ConfigOutput::JsonFields(_) => Err(error(d::PlanError::PermissionDenied)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_native_json_and_toml_proposals_keep_the_existing_format_bytes() {
        let registry = super::super::ConfigAdapters::builtin().unwrap();
        let provider: Provider = serde_json::from_value(serde_json::json!({"id":"owned-provider","name":"Owned","protocol":"responses","endpoint":"http://127.0.0.1:8000/v1","model":"owned-model","configured":true,"credentialRef":"11111111-1111-4111-8111-111111111111"})).unwrap();
        let helper = Path::new("/owned/helper path");
        for (agent, bytes) in [
            ("claude", b"{\"keep\":\"private-native-marker\"}".as_slice()),
            (
                "codex",
                b"# preserve\nextra='private-native-marker'\n".as_slice(),
            ),
            (
                "opencode",
                b"{\"keep\":\"private-native-marker\"}".as_slice(),
            ),
            (
                "claude-desktop",
                b"{\"keep\":\"private-native-marker\"}".as_slice(),
            ),
        ] {
            let expected =
                super::super::formats::render(agent, Some(bytes), Some(&provider), helper);
            let actual = registry.plan_native(agent, Some(bytes), Some(&provider), helper);
            match expected {
                Ok(bytes) => assert_eq!(actual.unwrap().0, bytes),
                Err(error) => {
                    let actual = actual.err().unwrap();
                    assert_eq!(actual.code, error.code);
                    assert_eq!(actual.message, error.message);
                }
            }
            let planner = NativePlanner {
                adapter: registry.resolve(agent).unwrap(),
                before: Some(bytes),
                provider: Some(&provider),
                helper,
            };
            let public = serde_json::to_string(&planner.request()).unwrap();
            assert!(
                !public.contains("private-native-marker")
                    && !public.contains("/owned/helper")
                    && !public.contains("11111111-1111-4111-8111-111111111111")
            );
        }
    }
    #[test]
    fn changed_native_context_is_rejected_before_format_parsing() {
        let registry = super::super::ConfigAdapters::builtin().unwrap();
        let planner = NativePlanner {
            adapter: registry.resolve("claude").unwrap(),
            before: Some(b"not JSON"),
            provider: None,
            helper: Path::new("/owned/helper"),
        };
        let mut request = planner.request();
        request.target_revision = "different".into();
        assert_eq!(planner.propose(&request).err().unwrap().code, "conflict");
        let actual = planner.propose(&planner.request()).err().unwrap();
        assert_eq!(actual.code, "invalid_config");
    }
}
