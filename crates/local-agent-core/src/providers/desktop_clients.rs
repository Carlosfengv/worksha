use super::{ConfigError, config::NativeConfig, fail, files, opencode_config};
use crate::{Provider, sessions::sanitize_label};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub fn path(home: &Path, agent: &str) -> Result<PathBuf, ConfigError> {
    if agent == "opencode" {
        let root = home.join(".config/opencode");
        let jsonc = root.join("opencode.jsonc");
        // OpenCode merges this file after opencode.json.
        return match jsonc.symlink_metadata() {
            Ok(_) => Ok(jsonc),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(root.join("opencode.json")),
            Err(_) => fail("unavailable", "无法检查 OpenCode 用户配置"),
        };
    }
    let root = home.join("Library/Application Support/Claude-3p/configLibrary");
    let bytes=files::read(&root.join("_meta.json"))?.bytes.ok_or_else(||ConfigError::new("unsupported","请先在 Claude 桌面的第三方推理设置中创建本机配置；本应用只切换已存在的本机 Profile，不改写组织策略"))?;
    let meta: Value = serde_json::from_slice(&bytes)
        .map_err(|_| ConfigError::new("invalid_config", "Claude 桌面配置目录无效"))?;
    let id = meta["appliedId"]
        .as_str()
        .ok_or_else(|| ConfigError::new("invalid_config", "Claude 桌面没有已应用的本机 Profile"))?;
    uuid::Uuid::parse_str(id)
        .map_err(|_| ConfigError::new("invalid_config", "Claude 桌面 Profile ID 无效"))?;
    if home
        .join("Library/Managed Preferences/com.anthropic.claudefordesktop.plist")
        .exists()
        || Path::new("/Library/Managed Preferences/com.anthropic.claudefordesktop.plist").exists()
        || Path::new("/Library/Managed Preferences")
            .join(home.file_name().unwrap_or_default())
            .join("com.anthropic.claudefordesktop.plist")
            .exists()
    {
        return fail("unsupported", "Claude 桌面存在组织策略，未改写本机推理设置");
    }
    Ok(root.join(format!("{id}.json")))
}
fn parse(before: Option<&[u8]>) -> Result<Value, ConfigError> {
    let d: Value = before
        .map(serde_json::from_slice)
        .transpose()
        .map_err(|_| ConfigError::new("invalid_config", "原客户端配置不是有效 JSON，未覆盖"))?
        .unwrap_or_else(|| json!({}));
    if !d.is_object() {
        return fail("invalid_config", "客户端配置必须是 JSON 对象");
    }
    Ok(d)
}
pub fn inspect(home: &Path, agent: &str) -> NativeConfig {
    let mut result = NativeConfig {
        agent_id: agent.into(),
        path: String::new(),
        state: "ready".into(),
        revision: None,
        mode: "native".into(),
        endpoint: None,
        catalog_endpoints: vec![],
        model: None,
        provider_name: "客户端原生配置".into(),
        credential: "未读取原生凭据".into(),
        error: None,
        warnings: vec![
            "仅修改本机用户级配置；项目、环境或组织配置可能覆盖。应用后需重新启动客户端。".into(),
        ],
    };
    let read = (|| {
        let path = path(home, agent)?;
        result.path = path.to_string_lossy().into();
        let read = files::read(&path)?;
        result.revision = Some(read.stamp.digest.clone().unwrap_or("absent".into()));
        if read.bytes.is_none() {
            result.state = "absent".into();
        }
        let d = if agent == "opencode" {
            opencode_config::parse(read.bytes.as_deref())?
        } else {
            parse(read.bytes.as_deref())?
        };
        if agent == "opencode" {
            if let Some(groups) = d["provider"].as_object() {
                for provider in groups.values() {
                    if let Some(endpoint) = provider["options"]["baseURL"]
                        .as_str()
                        .and_then(|raw| super::config::routing_endpoint(raw).ok())
                    {
                        result.catalog_endpoints.push(endpoint);
                    }
                }
            }
            result.model = d["model"].as_str().map(|s| sanitize_label(s, 250));
            if let Some((group, _)) = d["model"].as_str().and_then(|s| s.split_once('/')) {
                let p = &d["provider"][group];
                result.endpoint = p["options"]["baseURL"]
                    .as_str()
                    .map(super::config::routing_endpoint)
                    .transpose()?;
                result.provider_name = p["name"]
                    .as_str()
                    .map(|s| sanitize_label(s, 160))
                    .unwrap_or_else(|| sanitize_label(group, 160));
                if p["options"]["apiKey"].is_string() {
                    result.credential = "原生配置中存在凭据 · 不展示".into();
                }
            }
        } else {
            result.endpoint = d["inferenceGatewayBaseUrl"]
                .as_str()
                .map(super::config::routing_endpoint)
                .transpose()?;
            result.model = d["inferenceModels"]
                .as_array()
                .and_then(|a| a.first())
                .and_then(|v| v.as_str().or(v["name"].as_str()))
                .map(|s| sanitize_label(s, 200));
            result.provider_name = "Claude 桌面第三方推理".into();
            if d["inferenceCredentialHelper"].is_string() {
                result.credential = "系统钥匙串助手".into();
            } else if d["inferenceGatewayApiKey"].is_string() {
                result.credential = "原配置中存在网关凭据 · 不展示".into();
            }
        }
        if result.endpoint.is_some() {
            result.mode = "provider".into();
        }
        Ok::<_, ConfigError>(())
    })();
    if let Err(e) = read {
        result.state = "error".into();
        result.error = Some(e.message);
    }
    result
}
pub fn render(
    agent: &str,
    before: Option<&[u8]>,
    provider: Option<&Provider>,
    helper: &Path,
) -> Result<Vec<u8>, ConfigError> {
    let p = provider.ok_or_else(|| {
        ConfigError::new(
            "unsupported",
            "此客户端通过恢复备份回到原配置；请选择已配置的供应商",
        )
    })?;
    if agent == "opencode" {
        return opencode_config::edit(before, |d| render_opencode(d, p));
    }
    let mut d = parse(before)?;
    if p.credential_ref.is_none() {
        return fail("invalid_credential", "桌面连接需要已保存的凭据");
    }
    d["inferenceProvider"] = json!("gateway");
    d["inferenceGatewayBaseUrl"] = json!(p.endpoint);
    d.as_object_mut().unwrap().remove("inferenceGatewayApiKey");
    d.as_object_mut().unwrap().remove("inferenceGatewayOidc");
    d.as_object_mut()
        .unwrap()
        .remove("inferenceCredentialHelperArgs");
    d.as_object_mut()
        .unwrap()
        .remove("alwaysStartWithDefaultModel");
    d["inferenceCredentialKind"] = json!("helper-script");
    d["inferenceCredentialHelper"] = json!(helper.to_string_lossy());
    d["inferenceCredentialHelperTimeoutSec"] = json!(6);
    d["inferenceGatewayAuthScheme"] = json!("bearer");
    d["modelDiscoveryEnabled"] = json!(false);
    if !url::Url::parse(&p.endpoint).is_ok_and(|u| super::is_loopback_url(&u)) {
        return fail(
            "incompatible",
            "Claude 桌面通过本机兼容模型映射接入第三方模型，请先准备本机路由",
        );
    }
    d["inferenceModels"] =
        json!([{"name":"claude-sonnet-5","labelOverride":format!("{} · {}",p.name,p.model)}]);
    d["inferenceModelPricingEnabled"] = json!(false);
    let mut bytes = serde_json::to_vec_pretty(&d)
        .map_err(|_| ConfigError::new("invalid_config", "无法生成客户端配置"))?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn render_opencode(d: &mut Value, p: &Provider) -> Result<(), ConfigError> {
    if !url::Url::parse(&p.endpoint).is_ok_and(|u| super::is_loopback_url(&u)) {
        return fail(
            "incompatible",
            "OpenCode 使用本机受保护网关，避免把上游 API Key 写入客户端配置",
        );
    }
    if d.get("provider").is_some_and(|v| !v.is_object()) {
        return fail(
            "invalid_config",
            "OpenCode provider 字段不是对象，保留原配置",
        );
    }
    if d.get("provider").is_none() {
        d["provider"] = json!({});
    }
    let group = format!("local_agent_{}", &files::hash(p.id.as_bytes())[..16]);
    d["provider"][&group] = json!({"npm":"@ai-sdk/anthropic","name":format!("{} · Local Agent",p.name),"options":{"baseURL":format!("{}/v1",p.endpoint.trim_end_matches('/'))},"models":{p.model.clone():{"name":p.model,"options":{"thinking":{"type":"disabled"}}}}});
    d["model"] = json!(format!("{group}/{}", p.model));
    d["small_model"] = d["model"].clone();
    // An existing allowlist must include the new group. Preserve all existing entries.
    if let Some(enabled) = d.get_mut("enabled_providers") {
        let list = enabled
            .as_array_mut()
            .ok_or_else(|| ConfigError::new("invalid_config", "OpenCode enabled_providers 无效"))?;
        if !list.iter().any(|v| v == &group) {
            list.push(json!(group));
        }
    }
    Ok(())
}
pub fn opencode_credential(
    rendered: &mut Vec<u8>,
    provider: &Provider,
    key: &str,
    models: &[String],
) -> Result<(), ConfigError> {
    opencode_config::update(rendered, |d| {
        let group = format!("local_agent_{}", &files::hash(provider.id.as_bytes())[..16]);
        d["provider"][&group]["options"]["apiKey"] = json!(key);
        if !models.is_empty() {
            d["provider"][&group]["models"] = Value::Object(
                models
                    .iter()
                    .map(|model| {
                        (
                            model.clone(),
                            json!({"name":model,"options":{"thinking":{"type":"disabled"}}}),
                        )
                    })
                    .collect(),
            );
        }
        if provider.id.starts_with("dynamic-") && provider.model == crate::routing::ALIAS {
            if d.get("$schema").is_none() {
                d["$schema"] = json!("https://opencode.ai/config.json");
            }
            d["provider"][&group]["models"][crate::routing::ALIAS] = json!({
                "name":"Local Agent · 动态模型", "limit":{"context":32000,"output":8192},
                "options":{"thinking":{"type":"disabled"}}
            });
            d["provider"][&group]["options"]["maxRetries"] = json!(0);
        }
        Ok(())
    })
}

pub fn claude_models(
    rendered: &mut Vec<u8>,
    provider: &Provider,
    models: &[String],
) -> Result<(), ConfigError> {
    if models.len() > 200 {
        return fail(
            "capacity",
            "Claude 桌面最多支持 200 个模型，请先调整此 Provider 的已启用模型",
        );
    }
    let mut d = parse(Some(rendered))?;
    let mut ordered = models.to_vec();
    ordered.sort_by_key(|model| model != &provider.model);
    d["inferenceModels"] = json!(
        ordered
            .iter()
            .map(|model| json!({
                "name":super::claude_desktop_model_id(&provider.id, model),
                "labelOverride":format!("{} · {}",provider.name,model)
            }))
            .collect::<Vec<_>>()
    );
    use zeroize::Zeroize;
    rendered.zeroize();
    *rendered = serde_json::to_vec_pretty(&d)
        .map_err(|_| ConfigError::new("invalid_config", "Claude 桌面模型分组无效"))?;
    rendered.push(b'\n');
    Ok(())
}
