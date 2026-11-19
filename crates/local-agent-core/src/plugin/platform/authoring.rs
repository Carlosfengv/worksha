use super::{MAX_DECLARATION_BYTES, RulePackage};
use crate::providers::ConfigError;
use jsonc_parser::{CollectOptions, ParseOptions, ast};
use serde_json::{Map, Value};

type Result<T> = std::result::Result<T, ConfigError>;

fn invalid() -> ConfigError {
    ConfigError::new("invalid_input", "插件规则文件格式无效")
}

pub fn compile_rules(name: &str, source: &[u8]) -> Result<RulePackage> {
    if source.len() > MAX_DECLARATION_BYTES {
        return Err(invalid());
    }
    let text = std::str::from_utf8(source).map_err(|_| invalid())?;
    let text = text.trim_start_matches('\u{feff}');
    match name {
        "plugin.yaml" => {
            let value = crate::plugin::yaml::parse(text).map_err(|_| invalid())?;
            serde_json::from_value(value).map_err(|_| invalid())
        }
        "plugin.jsonc" => {
            let parsed = jsonc_parser::parse_to_ast(
                text,
                &CollectOptions::default(),
                &ParseOptions {
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
                },
            )
            .map_err(|_| invalid())?;
            let value = jsonc_value(parsed.value.ok_or_else(invalid)?)?;
            serde_json::from_value(value).map_err(|_| invalid())
        }
        "plugin.toml" => toml_edit::de::from_str(text).map_err(|_| invalid()),
        _ => Err(invalid()),
    }
}

fn jsonc_value(value: ast::Value<'_>) -> Result<Value> {
    Ok(match value {
        ast::Value::StringLit(node) => Value::String(node.value.into_owned()),
        ast::Value::BooleanLit(node) => Value::Bool(node.value),
        ast::Value::NumberLit(node) => Value::Number(node.value.parse().map_err(|_| invalid())?),
        ast::Value::NullKeyword(_) => Value::Null,
        ast::Value::Array(node) => Value::Array(
            node.elements
                .into_iter()
                .map(jsonc_value)
                .collect::<Result<_>>()?,
        ),
        ast::Value::Object(node) => {
            let mut object = Map::new();
            for field in node.properties {
                if object
                    .insert(field.name.into_string(), jsonc_value(field.value)?)
                    .is_some()
                {
                    return Err(invalid());
                }
            }
            Value::Object(object)
        }
    })
}
