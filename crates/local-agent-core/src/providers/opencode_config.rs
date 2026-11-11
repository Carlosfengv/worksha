use super::{ConfigError, fail, files};
use jsonc_parser::{
    JsonValue, ParseOptions,
    cst::{CstInputValue, CstObject, CstRootNode},
};
use serde_json::{Map, Value};
use std::{collections::HashMap, path::Path};
use zeroize::Zeroize;

const JSONC: ParseOptions = ParseOptions {
    allow_comments: true,
    allow_trailing_commas: true,
    allow_loose_object_property_names: false,
    allow_missing_commas: false,
    allow_single_quoted_strings: false,
    allow_hexadecimal_numbers: false,
    allow_unary_plus_numbers: false,
    allow_bare_decimal_point_numbers: false,
    allow_non_finite_numbers: false,
    allow_extended_string_escapes: false,
};
const ENABLED: &str = "enabled_providers";
const DISABLED: &str = "disabled_providers";

fn invalid() -> ConfigError {
    ConfigError::new(
        "invalid_config",
        "OpenCode 配置不是有效的 JSON 或 JSONC 对象，保留原文件",
    )
}

fn json_value(value: JsonValue<'_>) -> Result<Value, ConfigError> {
    Ok(match value {
        JsonValue::String(value) => Value::String(value.into_owned()),
        JsonValue::Number(value) => Value::Number(value.parse().map_err(|_| invalid())?),
        JsonValue::Boolean(value) => Value::Bool(value),
        JsonValue::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| Ok((key.into_owned(), json_value(value)?)))
                .collect::<Result<_, ConfigError>>()?,
        ),
        JsonValue::Array(values) => Value::Array(
            values
                .into_iter()
                .map(json_value)
                .collect::<Result<_, _>>()?,
        ),
        JsonValue::Null => Value::Null,
    })
}

fn parse_jsonc(source: &str) -> Result<Value, ConfigError> {
    let value = jsonc_parser::parse_to_value(source, &JSONC)
        .map_err(|_| invalid())?
        .map(json_value)
        .transpose()?
        .ok_or_else(invalid)?;
    object(value)
}

fn object(value: Value) -> Result<Value, ConfigError> {
    if value.is_object() {
        Ok(value)
    } else {
        Err(invalid())
    }
}

fn source(bytes: &[u8]) -> Result<&str, ConfigError> {
    std::str::from_utf8(bytes).map_err(|_| invalid())
}

pub(super) fn parse(bytes: Option<&[u8]>) -> Result<Value, ConfigError> {
    let Some(bytes) = bytes else {
        return Ok(Value::Object(Map::new()));
    };
    match serde_json::from_slice(bytes) {
        Ok(value) => object(value),
        Err(_) => parse_jsonc(source(bytes)?.trim_start_matches('\u{feff}')),
    }
}

pub(super) fn edit(
    before: Option<&[u8]>,
    update: impl FnOnce(&mut Value) -> Result<(), ConfigError>,
) -> Result<Vec<u8>, ConfigError> {
    let mut next = match before.map(serde_json::from_slice::<Value>).transpose() {
        Ok(value) => object(value.unwrap_or_else(|| Value::Object(Map::new())))?,
        Err(_) => {
            let source = source(before.unwrap())?;
            let text = source.trim_start_matches('\u{feff}');
            let bom = &source[..source.len() - text.len()];
            let old = parse_jsonc(text)?;
            let mut next = old.clone();
            update(&mut next)?;
            let root = CstRootNode::parse(text, &JSONC).map_err(|_| invalid())?;
            let object = root.object_value().ok_or_else(invalid)?;
            patch(&object, old.as_object().unwrap(), next.as_object().unwrap());
            let rendered = root.to_string();
            if parse_jsonc(&rendered)? != next {
                return fail("invalid_config", "无法安全更新 OpenCode 配置，保留原文件");
            }
            return Ok(format!("{bom}{rendered}").into_bytes());
        }
    };
    update(&mut next)?;
    let mut bytes = serde_json::to_vec_pretty(&next).map_err(|_| invalid())?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub(super) fn update(
    bytes: &mut Vec<u8>,
    update: impl FnOnce(&mut Value) -> Result<(), ConfigError>,
) -> Result<(), ConfigError> {
    let next = edit(Some(bytes), update)?;
    bytes.zeroize();
    *bytes = next;
    Ok(())
}

fn provider_list(document: &Value, field: &str) -> Result<Option<Vec<String>>, ConfigError> {
    document
        .get(field)
        .map(|value| {
            serde_json::from_value(value.clone()).map_err(|_| {
                ConfigError::new(
                    "invalid_config",
                    &format!("OpenCode {field} 必须是字符串列表"),
                )
            })
        })
        .transpose()
}

fn route_allowlist(
    path: &Path,
    before: Option<&[u8]>,
    next: &Value,
) -> Result<Option<Value>, ConfigError> {
    let target = parse(before)?;
    let mut enabled = provider_list(&target, ENABLED)?;
    let mut disabled = provider_list(&target, DISABLED)?;
    // OpenCode merges user configs in ascending priority. These lists replace
    // lower-priority values, so look up each field independently.
    let lower_files: &[&str] = if path.extension().is_some_and(|ext| ext == "jsonc") {
        &["opencode.json", "config.json"]
    } else {
        &["config.json"]
    };
    for file in lower_files {
        if enabled.is_some() && disabled.is_some() {
            break;
        }
        let snapshot = files::read(&path.with_file_name(file))?;
        let lower = parse(snapshot.bytes.as_deref())?;
        if enabled.is_none() {
            enabled = provider_list(&lower, ENABLED)?;
        }
        if disabled.is_none() {
            disabled = provider_list(&lower, DISABLED)?;
        }
    }
    let (group, _) = next["model"]
        .as_str()
        .and_then(|model| model.split_once('/'))
        .ok_or_else(invalid)?;
    if disabled.is_some_and(|list| list.iter().any(|id| id == group)) {
        return fail(
            "conflict",
            "OpenCode 的禁用列表包含本机模型分组，未写入配置",
        );
    }
    Ok(enabled.map(|mut list| {
        if !list.iter().any(|id| id == group) {
            list.push(group.to_owned());
        }
        serde_json::json!(list)
    }))
}

pub(super) fn preserve_route_allowlist(
    path: &Path,
    before: Option<&[u8]>,
    after: &mut Vec<u8>,
) -> Result<(), ConfigError> {
    let next = parse(Some(after))?;
    let allowlist = route_allowlist(path, before, &next)?;
    if let Some(list) = allowlist.filter(|list| next.get(ENABLED) != Some(list)) {
        update(after, |doc| {
            doc[ENABLED] = list;
            Ok(())
        })?;
    }
    Ok(())
}

pub(super) fn check_route_allowlist(
    path: &Path,
    before: Option<&[u8]>,
    after: &[u8],
) -> Result<(), ConfigError> {
    let next = parse(Some(after))?;
    if next.get(ENABLED) != route_allowlist(path, before, &next)?.as_ref() {
        return fail(
            "conflict",
            "OpenCode 的生效白名单已改变，未写入配置；请重新预览",
        );
    }
    Ok(())
}

fn patch(object: &CstObject, old: &Map<String, Value>, next: &Map<String, Value>) {
    let mut properties = HashMap::new();
    for prop in object.properties() {
        let Some(name) = prop.name().and_then(|name| name.decoded_value().ok()) else {
            continue;
        };
        if next.contains_key(&name) {
            // OpenCode uses the last value when a key is repeated.
            properties.insert(name, prop);
        } else {
            prop.remove();
        }
    }
    for (key, value) in next {
        let previous = old.get(key);
        if previous == Some(value) {
            continue;
        }
        let Some(prop) = properties.remove(key) else {
            object.append(key, input(value));
            continue;
        };
        if let (Some(child), Some(old), Some(next)) = (
            prop.object_value(),
            previous.and_then(Value::as_object),
            value.as_object(),
        ) {
            patch(&child, old, next);
        } else if let (Some(child), Some(old), Some(next)) = (
            prop.array_value(),
            previous.and_then(Value::as_array),
            value.as_array(),
        ) && let Some(added) = next.strip_prefix(old.as_slice())
        {
            for value in added {
                child.append(input(value));
            }
        } else {
            prop.set_value(input(value));
        }
    }
}

fn input(value: &Value) -> CstInputValue {
    match value {
        Value::Null => CstInputValue::Null,
        Value::Bool(value) => CstInputValue::Bool(*value),
        Value::Number(value) => CstInputValue::Number(value.to_string()),
        Value::String(value) => CstInputValue::String(value.clone()),
        Value::Array(values) => CstInputValue::Array(values.iter().map(input).collect()),
        Value::Object(values) => CstInputValue::Object(
            values
                .iter()
                .map(|(key, value)| (key.clone(), input(value)))
                .collect(),
        ),
    }
}
