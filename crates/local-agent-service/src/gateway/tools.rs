use super::protocol::Result;
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Default, Clone)]
pub struct Tools {
    identities: HashMap<String, Identity>,
}
#[derive(Clone)]
struct Identity {
    name: String,
    namespace: Option<String>,
    custom: bool,
}
impl Tools {
    fn register(&mut self, name: &str, namespace: Option<&str>, custom: bool) -> Result<String> {
        if name.is_empty() || name.len() > 200 || namespace.is_some_and(|s| s.len() > 200) {
            return Err("工具名称无效");
        }
        let key = format!("{}:{name}", namespace.unwrap_or(""));
        let alias = format!(
            "la_{}",
            &local_agent_core::providers::files::hash(key.as_bytes())[..32]
        );
        if let Some(old) = self.identities.get(&alias)
            && old.custom != custom
        {
            return Err("同名工具类型发生冲突");
        }
        self.identities.insert(
            alias.clone(),
            Identity {
                name: name.into(),
                namespace: namespace.map(str::to_owned),
                custom,
            },
        );
        Ok(alias)
    }
    fn definitions(
        &mut self,
        tools: &[Value],
        namespace: Option<&str>,
        out: &mut Vec<Value>,
    ) -> Result<()> {
        for tool in tools {
            if tool["type"] == "namespace" {
                self.definitions(
                    tool["tools"].as_array().ok_or("工具命名空间缺少工具列表")?,
                    Some(tool["name"].as_str().ok_or("工具命名空间无效")?),
                    out,
                )?;
                continue;
            }
            let custom = tool["type"] == "custom";
            if !custom && tool["type"] != "function" {
                return Err("原生托管工具不能跨协议转换；请使用同协议连接或在 Agent 中关闭该工具");
            }
            let alias = self.register(
                tool["name"].as_str().ok_or("工具名称缺失")?,
                namespace,
                custom,
            )?;
            let mut t = tool.clone();
            t["name"] = json!(alias);
            t["type"] = json!("function");
            if custom {
                let mut description = tool["description"].as_str().unwrap_or("").to_string();
                if let Some(grammar) = tool["format"]["definition"].as_str() {
                    description.push_str("\nReturn the complete tool input in the input string. Follow this grammar:\n");
                    description.push_str(grammar);
                }
                t = json!({"type":"function","name":alias,"description":description,"parameters":{"type":"object","properties":{"input":{"type":"string"}},"required":["input"],"additionalProperties":false},"strict":true});
            }
            out.push(t);
        }
        Ok(())
    }
    pub fn prepare(source: &str, value: &Value) -> Result<(Self, Value)> {
        super::capabilities::builtin().resolve(source)?;
        super::capabilities::builtin()
            .tools()?
            .prepare_names(source, value)
    }
    pub(super) fn prepare_builtin(source: &str, value: &Value) -> Result<(Self, Value)> {
        let mut registry = Self::default();
        let mut v = value.clone();
        if source == "responses" {
            if let Some(definitions) = value["tools"].as_array() {
                let mut tools = vec![];
                registry.definitions(definitions, None, &mut tools)?;
                if tools.is_empty() {
                    let request = v.as_object_mut().unwrap();
                    request.remove("parallel_tool_calls");
                    if matches!(
                        request.get("tool_choice").and_then(Value::as_str),
                        Some("auto" | "none")
                    ) {
                        request.remove("tool_choice");
                    }
                }
                // An empty namespace contributes no tool definitions.
                v["tools"] = json!(tools);
            }
            for item in v["input"].as_array_mut().into_iter().flatten() {
                let custom = item["type"] == "custom_tool_call";
                if item["type"] == "function_call" || custom {
                    let alias = registry.register(
                        item["name"].as_str().ok_or("工具调用名称缺失")?,
                        item["namespace"].as_str(),
                        custom,
                    )?;
                    if custom {
                        item["type"] = json!("function_call");
                        item["arguments"]=json!(json!({"input":item["input"].as_str().ok_or("自定义工具输入需要字符串")?}).to_string());
                        item.as_object_mut().unwrap().remove("input");
                    }
                    item["name"] = json!(alias);
                    item.as_object_mut().unwrap().remove("namespace");
                } else if item["type"] == "custom_tool_call_output" {
                    item["type"] = json!("function_call_output");
                }
            }
            if v["tool_choice"]["type"] == "function" || v["tool_choice"]["type"] == "custom" {
                let custom = v["tool_choice"]["type"] == "custom";
                let alias = registry.register(
                    v["tool_choice"]["name"]
                        .as_str()
                        .ok_or("工具选择名称缺失")?,
                    v["tool_choice"]["namespace"].as_str(),
                    custom,
                )?;
                v["tool_choice"] = json!({"type":"function","name":alias});
            }
        } else {
            for tool in v
                .get_mut("tools")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                tool["name"] = json!(registry.register(
                    tool["name"].as_str().ok_or("工具名称缺失")?,
                    None,
                    false
                )?);
            }
            for msg in v["messages"].as_array_mut().into_iter().flatten() {
                for part in msg["content"].as_array_mut().into_iter().flatten() {
                    if part["type"] == "tool_use" {
                        part["name"] = json!(registry.register(
                            part["name"].as_str().ok_or("工具名称缺失")?,
                            None,
                            false
                        )?);
                    }
                }
            }
            if v["tool_choice"]["type"] == "tool" {
                v["tool_choice"]["name"] = json!(
                    registry.register(
                        v["tool_choice"]["name"]
                            .as_str()
                            .ok_or("工具选择名称缺失")?,
                        None,
                        false
                    )?
                );
            }
        }
        Ok((registry, v))
    }
    pub fn is_custom(&self, alias: &str) -> bool {
        self.identities.get(alias).is_some_and(|i| i.custom)
    }
    pub fn name<'a>(&'a self, alias: &'a str) -> &'a str {
        self.identities
            .get(alias)
            .map(|i| i.name.as_str())
            .unwrap_or(alias)
    }
    pub fn response_item(&self, mut item: Value) -> Result<Value> {
        let Some(identity) = item["name"]
            .as_str()
            .and_then(|name| self.identities.get(name))
        else {
            return Ok(item);
        };
        item["name"] = json!(identity.name);
        if let Some(namespace) = &identity.namespace {
            item["namespace"] = json!(namespace);
        }
        if identity.custom {
            let args = item["arguments"].as_str().unwrap_or("");
            let input = if args.is_empty() {
                String::new()
            } else {
                let v: Value =
                    serde_json::from_str(args).map_err(|_| "自定义工具 input 封装不是有效 JSON")?;
                v["input"]
                    .as_str()
                    .ok_or("自定义工具缺少 input 字符串")?
                    .to_string()
            };
            item["type"] = json!("custom_tool_call");
            item["input"] = json!(input);
            item.as_object_mut().unwrap().remove("arguments");
        }
        Ok(item)
    }
    pub fn restore(&self, protocol: &str, mut response: Value) -> Result<Value> {
        if protocol == "responses" {
            for item in response["output"].as_array_mut().into_iter().flatten() {
                if item["type"] == "function_call" {
                    *item = self.response_item(item.clone())?;
                }
            }
        } else {
            for part in response["content"].as_array_mut().into_iter().flatten() {
                if part["type"] == "tool_use" {
                    part["name"] = json!(self.name(part["name"].as_str().ok_or("工具名称缺失")?));
                }
            }
        }
        Ok(response)
    }
}
