//! Compiled-in wire capabilities. Network, admission, credentials and history stay in the host.
use super::{
    protocol::{self, Answer, Result},
    stream::{Decoder, Encoder},
    tools::Tools,
};
use local_agent_core::{
    Provider,
    plugin::{
        BuiltinPlugin, CapabilityKey, PluginDescriptor, PluginError, PluginHost, PluginResources,
    },
};
use serde_json::{Value, json};
use std::sync::LazyLock;

pub trait WireProtocol: BuiltinPlugin {
    fn id(&self) -> &'static str;
    fn endpoint(&self, base: &str) -> String;
    fn headers(&self) -> &'static [(&'static str, &'static str)];
    fn api_key_header(&self) -> bool;
    fn verification_body(&self, model: &str) -> Value;
    fn verification_valid(&self, body: &Value) -> bool;
    fn request(&self, source: &str, v: &Value, model: &str) -> Result<Value>;
    fn answer(&self, v: &Value) -> Result<Answer>;
    fn render(&self, answer: &Answer, id: &str, model: &str) -> Result<Value>;
    fn client_catalog(&self, provider: &Provider, models: &[String]) -> Result<Value>;
    fn metrics(&self, bytes: &[u8]) -> crate::observation::stream::Update;
    fn metric_stream(&self) -> crate::observation::stream::StreamDecoder;
    fn decoder(&self) -> Decoder;
    fn encoder(&self, model: &str, tools: Tools) -> Encoder;
}
struct Wire(&'static str);
fn key(id: &'static str) -> CapabilityKey {
    CapabilityKey {
        name: "gateway.protocol",
        version: 1,
        instance: id,
    }
}
impl BuiltinPlugin for Wire {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            id: self.0,
            version: "1.0.0",
            provides: vec![
                key(self.0),
                CapabilityKey {
                    name: "provider.verify",
                    version: 1,
                    instance: self.0,
                },
                CapabilityKey {
                    name: "provider.catalog",
                    version: 1,
                    instance: self.0,
                },
            ],
            requires: vec![],
        }
    }
    fn prepare(&self) -> std::result::Result<PluginResources, PluginError> {
        Ok(PluginResources::default())
    }
}
impl WireProtocol for Wire {
    fn id(&self) -> &'static str {
        self.0
    }
    fn endpoint(&self, base: &str) -> String {
        let base = base.trim_end_matches('/');
        let suffix = match self.0 {
            "messages" => "/messages",
            "chat-completions" => "/chat/completions",
            _ => "/responses",
        };
        if base.ends_with(suffix) {
            return base.into();
        }
        format!(
            "{base}{}",
            match self.0 {
                "messages" if !base.ends_with("/v1") => "/v1/messages",
                "messages" => "/messages",
                "chat-completions" => "/chat/completions",
                _ => "/responses",
            }
        )
    }
    fn headers(&self) -> &'static [(&'static str, &'static str)] {
        if self.0 == "messages" {
            &[("anthropic-version", "2023-06-01")]
        } else {
            &[]
        }
    }
    fn api_key_header(&self) -> bool {
        self.0 == "messages"
    }
    fn verification_body(&self, model: &str) -> Value {
        match self.0 {
            "messages" => {
                json!({"model":model,"max_tokens":16,"messages":[{"role":"user","content":"Reply OK."}]})
            }
            "chat-completions" => {
                json!({"model":model,"max_tokens":16,"messages":[{"role":"user","content":"Reply OK."}],"stream":false})
            }
            _ => json!({"model":model,"max_output_tokens":16,"input":"Reply OK.","store":false}),
        }
    }
    fn verification_valid(&self, body: &Value) -> bool {
        let common = body["id"].is_string() && body["model"].is_string();
        common
            && match self.0 {
                "messages" => {
                    body["type"] == "message"
                        && body["role"] == "assistant"
                        && body["content"].is_array()
                }
                "chat-completions" => {
                    body["object"] == "chat.completion"
                        && body["choices"].as_array().is_some_and(|v| v.len() == 1)
                        && body["choices"][0]["message"]["role"] == "assistant"
                }
                _ => {
                    body["object"] == "response"
                        && matches!(body["status"].as_str(), Some("completed" | "incomplete"))
                        && body["output"].is_array()
                        && body.get("error").is_none_or(Value::is_null)
                }
            }
    }
    fn request(&self, source: &str, v: &Value, model: &str) -> Result<Value> {
        protocol::request_builtin(source, self.0, v, model)
    }
    fn answer(&self, v: &Value) -> Result<Answer> {
        Answer::parse_builtin(self.0, v)
    }
    fn render(&self, answer: &Answer, id: &str, model: &str) -> Result<Value> {
        answer.render_builtin(self.0, id, model)
    }
    fn client_catalog(&self, provider: &Provider, models: &[String]) -> Result<Value> {
        if self.0 != "responses" {
            return Err("此协议未注册 Codex 模型目录");
        }
        Ok(super::codex_catalog::catalog(provider, models))
    }
    fn metrics(&self, bytes: &[u8]) -> crate::observation::stream::Update {
        crate::observation::stream::decode_builtin(bytes, self.0)
    }
    fn metric_stream(&self) -> crate::observation::stream::StreamDecoder {
        crate::observation::stream::StreamDecoder::new(self.0)
    }
    fn decoder(&self) -> Decoder {
        Decoder::new(self.0)
    }
    fn encoder(&self, model: &str, tools: Tools) -> Encoder {
        Encoder::new(self.0, model).with_tools(tools)
    }
}
static WIRES: [Wire; 3] = [
    Wire("messages"),
    Wire("responses"),
    Wire("chat-completions"),
];

pub trait ToolNames: BuiltinPlugin {
    fn prepare_names(&self, source: &str, value: &Value) -> Result<(Tools, Value)>;
}
struct NativeTools;
impl BuiltinPlugin for NativeTools {
    fn descriptor(&self) -> PluginDescriptor {
        PluginDescriptor {
            id: "gateway.tool-names",
            version: "1.0.0",
            provides: vec![CapabilityKey {
                name: "gateway.tool-names",
                version: 1,
                instance: "native",
            }],
            requires: vec![],
        }
    }
    fn prepare(&self) -> std::result::Result<PluginResources, PluginError> {
        Ok(PluginResources::default())
    }
}
impl ToolNames for NativeTools {
    fn prepare_names(&self, source: &str, value: &Value) -> Result<(Tools, Value)> {
        Tools::prepare_builtin(source, value)
    }
}
pub struct Protocols {
    host: PluginHost,
    wires: Vec<&'static dyn WireProtocol>,
    tool_names: &'static dyn ToolNames,
}
impl Protocols {
    pub fn builtin() -> std::result::Result<Self, PluginError> {
        Self::from_contributions(
            WIRES.iter().map(|w| w as &dyn WireProtocol).collect(),
            &NativeTools,
        )
    }
    pub fn from_contributions(
        wires: Vec<&'static dyn WireProtocol>,
        tool_names: &'static dyn ToolNames,
    ) -> std::result::Result<Self, PluginError> {
        if wires
            .iter()
            .any(|wire| !wire.descriptor().provides.contains(&key(wire.id())))
            || !tool_names.descriptor().provides.contains(&CapabilityKey {
                name: "gateway.tool-names",
                version: 1,
                instance: "native",
            })
        {
            return Err(PluginError {
                code: "invalid_input",
                message: "协议 / 工具实现与能力描述不匹配".into(),
            });
        }
        let mut plugins: Vec<&dyn BuiltinPlugin> = wires
            .iter()
            .map(|wire| *wire as &dyn BuiltinPlugin)
            .collect();
        plugins.push(tool_names);
        let host = PluginHost::mount(&plugins)?;
        for wire in &wires {
            host.acquire(&key(wire.id()))?;
        }
        host.acquire(&CapabilityKey {
            name: "gateway.tool-names",
            version: 1,
            instance: "native",
        })?;
        Ok(Self {
            host,
            wires,
            tool_names,
        })
    }
    pub fn resolve(&self, id: &str) -> Result<&'static dyn WireProtocol> {
        let wire = self
            .wires
            .iter()
            .copied()
            .find(|wire| wire.id() == id)
            .ok_or("接口协议能力尚未注册")?;
        self.host
            .acquire(&key(wire.id()))
            .map_err(|_| "接口协议能力已停止")?;
        Ok(wire)
    }
    pub fn tools(&self) -> Result<&dyn ToolNames> {
        self.host
            .acquire(&CapabilityKey {
                name: "gateway.tool-names",
                version: 1,
                instance: "native",
            })
            .map_err(|_| "工具名称映射能力已停止")?;
        Ok(self.tool_names)
    }
    #[cfg(test)]
    fn stop(&self, id: &str) {
        self.host.stop(id).unwrap();
    }
}
pub fn builtin() -> &'static Protocols {
    static REGISTRY: LazyLock<Protocols> =
        LazyLock::new(|| Protocols::builtin().expect("builtin protocols"));
    &REGISTRY
}
pub fn codex_catalog(provider: &Provider, models: &[String]) -> Value {
    builtin()
        .resolve("responses")
        .and_then(|wire| wire.client_catalog(provider, models))
        .expect("builtin Codex catalogue")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn composition_rejects_duplicates_and_does_not_expose_unregistered_protocols() {
        let registry = Protocols::from_contributions(vec![&WIRES[0]], &NativeTools).unwrap();
        assert!(registry.resolve("messages").is_ok());
        assert!(registry.resolve("responses").is_err());
        assert!(Protocols::from_contributions(vec![&WIRES[0], &WIRES[0]], &NativeTools).is_err());
    }
    #[test]
    fn capability_stop_fails_closed_and_leaves_other_wire_available() {
        let registry = Protocols::builtin().unwrap();
        registry.stop("messages");
        assert!(registry.resolve("messages").is_err());
        assert!(registry.resolve("responses").is_ok());
        assert!(registry.resolve("future").is_err());
        registry.stop("gateway.tool-names");
        assert!(registry.tools().is_err());
    }
    #[test]
    fn plans_keep_protocol_paths_limits_and_response_validation() {
        for id in ["messages", "responses", "chat-completions"] {
            let wire = builtin().resolve(id).unwrap();
            assert!(!wire.verification_valid(&json!({"id":"x", "model":"m"})));
            assert_eq!(wire.verification_body("m")["model"], "m");
            assert!(
                wire.endpoint("https://example.invalid/v1/")
                    .starts_with("https://example.invalid/v1/")
            );
        }
    }
}
