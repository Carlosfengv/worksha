//! Preset selection is read-only and never prepares credentials, grants or writes.
use crate::{ApiResult, AppState, json_value};
use axum::{Json, extract::State};
use local_agent_core::{
    domain::provider_presets::{
        self, BuiltinPresets, PresetAction, PresetOrigin, PresetReply, checked_presets,
    },
    providers::ConfigError,
};

pub(crate) async fn action(
    State(state): State<AppState>,
    Json(input): Json<PresetAction>,
) -> ApiResult {
    crate::blocking::run(move || {
        let store_id = match &input {
            PresetAction::List { store_id } | PresetAction::Resolve { store_id, .. } => store_id,
        };
        if !local_agent_core::providers::operations::valid_id(store_id) {
            return Err(super::error(ConfigError::new(
                "invalid_input",
                "供应商预设的原数据库身份无效",
            )));
        }
        let current =
            || -> std::result::Result<(), (axum::http::StatusCode, Json<serde_json::Value>)> {
                let protocol = state
                    .store
                    .lock()
                    .map_err(|_| super::error(unavailable()))?
                    .provider_write_protocol()
                    .map_err(super::error)?;
                if protocol.store_id != *store_id {
                    return Err(super::error(ConfigError::new(
                        "operation_store_changed",
                        "请连接此草稿原来的数据库，未替换预设",
                    )));
                }
                Ok(())
            };
        current()?;
        let builtin = checked_presets(&BuiltinPresets).map_err(super::error)?;
        let result = match &input {
            PresetAction::List { store_id } => {
                let (external, issues) = state.plugins.preset_inventory();
                let mut items = builtin;
                items.extend(external);
                PresetReply::Listed {
                    store_id: store_id.clone(),
                    items,
                    issues,
                }
            }
            PresetAction::Resolve { store_id, origin } => {
                let advice = match origin {
                    PresetOrigin::Builtin { .. } => builtin
                        .into_iter()
                        .find(|p| p.origin == *origin)
                        .ok_or_else(|| {
                            super::error(ConfigError::new(
                                "invalid_input",
                                "未找到此内置供应商预设",
                            ))
                        })?,
                    PresetOrigin::External { .. } => {
                        state.plugins.resolve_preset(origin).map_err(|e| {
                            super::error(ConfigError::new(
                                e.code,
                                crate::plugin_host::external::message(e.code),
                            ))
                        })?
                    }
                };
                PresetReply::Resolved {
                    store_id: store_id.clone(),
                    advice: Box::new(advice),
                }
            }
        };
        current()?;
        let bytes = serde_json::to_vec(&result).map_err(|_| super::error(unavailable()))?;
        if bytes.len() > provider_presets::MAX_RESPONSE_BYTES {
            return Err(super::error(ConfigError::new(
                "capacity",
                "预设回复超过读取容量，草稿保持",
            )));
        }
        json_value(result)
    })
    .await
    .map_err(|e| super::error(crate::blocking::config_error(e)))?
}
fn unavailable() -> ConfigError {
    ConfigError::new("unavailable", "供应商预设暂不可核查，草稿保持")
}
