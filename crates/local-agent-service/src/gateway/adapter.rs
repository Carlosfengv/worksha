//! Native execution of the common protocol lifecycle; host delivery/accounting stays separate.
use super::{
    capabilities::WireProtocol,
    protocol::{self, Answer, Result},
    stream::{Decoder, Encoder},
    tools::Tools,
};
use local_agent_core::{
    Provider,
    domain::protocol::{Finished, Open, Opened, Output, ProtocolAdapter},
};
use serde_json::Value;

pub(crate) struct Native {
    provider: Provider,
    source: String,
    tools: Tools,
    tool_calls_allowed: bool,
    hide_intrinsic_reasoning: bool,
    managed: bool,
    opened: bool,
    finished: bool,
    streamed: bool,
    stream: Option<(Decoder, Encoder)>,
    json: Vec<u8>,
    response_id: Option<String>,
    end_status: &'static str,
}
impl Native {
    pub(crate) fn new(
        provider: &Provider,
        source: &str,
        tools: Tools,
        tool_calls_allowed: bool,
        managed: bool,
    ) -> Self {
        Self {
            provider: provider.clone(),
            source: source.into(),
            tools,
            tool_calls_allowed,
            hide_intrinsic_reasoning: false,
            managed,
            opened: false,
            finished: false,
            streamed: false,
            stream: None,
            json: Vec::new(),
            response_id: None,
            end_status: "success",
        }
    }
    pub(crate) fn hide_intrinsic_reasoning(mut self, hide: bool) -> Self {
        self.hide_intrinsic_reasoning = hide;
        self
    }
    // Called only after the host has accepted status, encoding and content type.
    // Starting client output before this point would change the existing error behavior.
    pub(crate) fn start_stream(
        &mut self,
        upstream: &dyn WireProtocol,
        client: &dyn WireProtocol,
    ) -> Result<Vec<u8>> {
        if !self.opened || !self.streamed || self.stream.is_some() {
            return Err("协议转换尚未就绪");
        }
        let decoder = upstream
            .decoder()
            .with_provider(&self.provider)
            .hide_intrinsic_reasoning(self.hide_intrinsic_reasoning);
        let mut encoder = client
            .encoder(&self.provider.model, std::mem::take(&mut self.tools))
            .with_provider(&self.provider)
            .with_tool_calls(self.tool_calls_allowed);
        self.response_id = Some(encoder.response_id().into());
        let start = encoder.start().into_bytes();
        self.stream = Some((decoder, encoder));
        Ok(start)
    }
    pub(crate) fn response_id(&self) -> Option<&str> {
        self.response_id.as_deref()
    }
    pub(crate) fn json_bytes(&self) -> &[u8] {
        &self.json
    }
    pub(crate) fn end_status(&self) -> &'static str {
        self.stream
            .as_ref()
            .map_or(self.end_status, |(_, e)| e.end_status())
    }
    pub(crate) fn error(&mut self) -> Vec<u8> {
        self.stream
            .as_mut()
            .map_or_else(Vec::new, |(_, e)| e.error().into_bytes())
    }
    pub(crate) fn take_rejection(&mut self) -> Option<Value> {
        self.stream.as_mut()?.0.take_rejection()
    }
    pub(crate) fn feed_stream(&mut self, bytes: &[u8], output: &mut Output) -> Result<()> {
        if self.finished {
            return Err("协议转换尚未就绪");
        }
        let (decoder, encoder) = self.stream.as_mut().ok_or("协议转换尚未就绪")?;
        let mut terminal = None;
        decoder.feed_each(bytes, |delta| {
            let chunk = encoder.push(delta)?.into_bytes();
            if encoder.is_finished() {
                terminal = Some(chunk);
            } else {
                output.chunks.push(chunk);
            }
            Ok(())
        })?;
        // An invalid suffix cannot turn a partial answer into a complete reply.
        if let Some(chunk) = terminal {
            output.chunks.push(chunk);
            output.complete = true;
        }
        Ok(())
    }
    fn encode(&mut self, deltas: Vec<super::stream::Delta>) -> Result<Output> {
        let (_, encoder) = self.stream.as_mut().ok_or("协议转换尚未就绪")?;
        let chunks = deltas
            .into_iter()
            .map(|delta| encoder.push(delta).map(String::into_bytes))
            .collect::<Result<Vec<_>>>()?;
        Ok(Output {
            chunks,
            complete: encoder.is_finished(),
        })
    }
}
impl ProtocolAdapter for Native {
    type Error = &'static str;
    async fn open(&mut self, input: Open) -> Result<Opened> {
        if self.opened {
            return Err("协议转换已开始");
        }
        let mut request = if self.managed {
            protocol::provider_request(&self.source, &self.provider, &input.request)?
        } else {
            input.request
        };
        if self.hide_intrinsic_reasoning {
            protocol::use_intrinsic_reasoning(&self.provider.protocol, &mut request);
        }
        self.streamed = input.stream;
        self.opened = true;
        Ok(Opened::Json(request))
    }
    async fn feed(&mut self, bytes: &[u8]) -> Result<Output> {
        if !self.opened || self.finished {
            return Err("协议转换尚未就绪");
        }
        if self.streamed {
            let mut output = Output::default();
            self.feed_stream(bytes, &mut output)?;
            Ok(output)
        } else {
            if self.json.len().saturating_add(bytes.len()) > 8 * 1024 * 1024 {
                return Err("上游响应超限、中断或超时");
            }
            self.json.extend_from_slice(bytes);
            Ok(Output::default())
        }
    }
    async fn finish(&mut self) -> Result<Finished> {
        if !self.opened || self.finished {
            return Err("协议转换尚未就绪");
        }
        self.finished = true;
        if self.streamed {
            let (decoder, _) = self.stream.as_mut().ok_or("协议转换尚未就绪")?;
            let deltas = decoder.end()?;
            self.encode(deltas).map(Finished::Stream)
        } else {
            let id = format!("resp_{}", uuid::Uuid::new_v4().simple());
            let mut v: Value =
                serde_json::from_slice(&self.json).map_err(|_| "上游响应不是有效 JSON")?;
            if self.hide_intrinsic_reasoning {
                protocol::remove_intrinsic_reasoning(&self.provider.protocol, &mut v);
            }
            let answer = Answer::parse_provider(&self.provider, &v)?;
            if !self.tool_calls_allowed
                && (!answer.tools.is_empty() || answer.finish == "tool_calls")
            {
                return Err("上游返回了此文本入口未发布的工具调用");
            }
            let value = answer
                .render(&self.source, &id, &self.provider.model)
                .and_then(|v| self.tools.restore(&self.source, v))?;
            self.end_status =
                if value["status"] == "incomplete" || value["stop_reason"] == "max_tokens" {
                    "incomplete"
                } else {
                    "success"
                };
            self.response_id = Some(id);
            Ok(Finished::Json(value))
        }
    }
}

#[cfg(test)]
mod tests;
