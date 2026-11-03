//! The old finite extension RPC remains the execution mechanism for the common lifecycle.
use super::{Checked, Result, Worker, fail};
use local_agent_core::{
    domain::protocol::{Finished, Open, Opened, Output, ProtocolAdapter},
    plugin::advanced::{MAX_DOCUMENT, ProtocolOpen, WireChunk, WireOutput},
};
use serde_json::json;

pub(super) struct GuestProtocolAdapter {
    worker: Worker,
}
impl GuestProtocolAdapter {
    pub(super) fn new(worker: Worker) -> Self {
        Self { worker }
    }
    pub(super) async fn check(&self) -> Result<()> {
        self.worker.check().await
    }
    pub(super) async fn close(self) -> Result<()> {
        self.worker.close().await
    }
}
fn output(value: serde_json::Value, finish: bool) -> Result<Output> {
    let value: WireOutput = serde_json::from_value(value).checked("invalid_input")?;
    if finish && (!value.complete || value.bytes.len() > MAX_DOCUMENT as usize) {
        return Err(fail("incomplete_protocol"));
    }
    if value.bytes.len() > MAX_DOCUMENT as usize {
        return Err(fail("resource_limit"));
    }
    Ok(Output {
        chunks: if value.bytes.is_empty() {
            vec![]
        } else {
            vec![value.bytes]
        },
        complete: value.complete,
    })
}
impl ProtocolAdapter for GuestProtocolAdapter {
    type Error = super::super::super::Error;
    async fn open(&mut self, input: Open) -> Result<Opened> {
        let value = self
            .worker
            .call(
                "protocol.open",
                serde_json::to_value(ProtocolOpen {
                    stream: input.stream,
                    request: input.request,
                })
                .checked("invalid_input")?,
            )
            .await?;
        let value: WireChunk = serde_json::from_value(value).checked("invalid_input")?;
        if value.bytes.len() > MAX_DOCUMENT as usize
            || serde_json::from_slice::<serde_json::Value>(&value.bytes).is_err()
        {
            return Err(fail("invalid_input"));
        }
        Ok(Opened::Bytes(value.bytes))
    }
    async fn feed(&mut self, bytes: &[u8]) -> Result<Output> {
        output(
            self.worker
                .call("protocol.feed", json!({"bytes":bytes}))
                .await?,
            false,
        )
    }
    async fn finish(&mut self) -> Result<Finished> {
        output(self.worker.call("protocol.finish", json!({})).await?, true).map(Finished::Stream)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legacy_output_keeps_strict_dto_limits_and_finish_semantics() {
        let value = output(json!({"bytes":[0,255,10],"complete":false}), false).unwrap();
        assert_eq!(value.chunks, vec![vec![0, 255, 10]]);
        assert!(!value.complete);
        assert_eq!(
            output(json!({"bytes":[],"complete":false}), true)
                .err()
                .unwrap()
                .code,
            "incomplete_protocol"
        );
        assert_eq!(
            output(json!({"bytes":[],"complete":true,"sent":true}), true)
                .err()
                .unwrap()
                .code,
            "invalid_input"
        );
        assert_eq!(
            output(
                json!({"bytes":vec![0u8;MAX_DOCUMENT as usize+1],"complete":true}),
                false
            )
            .err()
            .unwrap()
            .code,
            "resource_limit"
        );
        assert_eq!(
            output(
                json!({"bytes":vec![0u8;MAX_DOCUMENT as usize+1],"complete":true}),
                true
            )
            .err()
            .unwrap()
            .code,
            "incomplete_protocol"
        );
        assert!(
            output(json!({"bytes":[],"complete":true}), true)
                .unwrap()
                .chunks
                .is_empty()
        );
    }
}
