//! Hold a canonical terminal event until the finite guest finish and close succeed.
//! Earlier events remain streaming; no complete response is collected here.
use super::{Result, fail};
use crate::observation::stream::StreamDecoder;

const MAX_PENDING: usize = 8 * 1024 * 1024;

pub(super) struct Completion {
    decoder: StreamDecoder,
    pending: Vec<u8>,
    held: Vec<u8>,
    line_start: usize,
    terminal: bool,
}
impl Completion {
    pub(super) fn new(protocol: &str) -> Self {
        Self {
            decoder: StreamDecoder::new(protocol),
            pending: Vec::new(),
            held: Vec::new(),
            line_start: 0,
            terminal: false,
        }
    }
    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>> {
        let mut ready = Vec::new();
        for byte in bytes {
            self.pending.push(*byte);
            if self.pending.len().saturating_add(self.held.len()) > MAX_PENDING {
                return Err(fail("resource_limit"));
            }
            if *byte != b'\n' {
                continue;
            }
            let line = &self.pending[self.line_start..self.pending.len() - 1];
            let empty = line.is_empty() || line == b"\r";
            self.line_start = self.pending.len();
            if !empty {
                continue;
            }
            let event = std::mem::take(&mut self.pending);
            self.line_start = 0;
            self.terminal |= self
                .decoder
                .feed(&event)
                .iter()
                .any(|u| u.terminal.is_some());
            if self.terminal {
                self.held.extend(event);
            } else {
                ready.push(event);
            }
        }
        Ok(ready)
    }
    // Only the caller that validated finish and closed the worker can release this tail.
    pub(super) fn finish(mut self) -> Vec<Vec<u8>> {
        self.held.append(&mut self.pending);
        if self.held.is_empty() {
            vec![]
        } else {
            vec![self.held]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arbitrary_utf8_crlf_and_multiple_events_preserve_bytes_and_hold_only_the_tail() {
        for (protocol,terminal) in [
            ("messages",b"event: message_stop\r\ndata: {\"type\":\"message_stop\"}\r\n\r\n".as_slice()),
            ("responses",b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n".as_slice()),
            ("chat-completions",b"data: [DONE]\n\n".as_slice()),
        ] {
            let first="data: {\"type\":\"response.output_text.delta\",\"delta\":\"你好\"}\r\n\r\n".as_bytes();
            let bytes=[first,terminal,b": after\n\n"].concat();
            for width in [1,2,7,13,bytes.len()] {
                let mut completion=Completion::new(protocol);
                let mut sent=Vec::new();
                for part in bytes.chunks(width) {for event in completion.push(part).unwrap(){sent.extend(event);}}
                assert_eq!(sent,first,"{protocol}, width {width}");
                assert_eq!(completion.finish().concat(),[terminal,b": after\n\n"].concat());
            }
        }
    }
    #[test]
    fn partial_tail_is_bounded_and_terminal_is_never_released_by_drop() {
        let mut completion = Completion::new("responses");
        assert!(completion.push(b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n").unwrap().is_empty());
        assert!(completion.push(b"unfinished").unwrap().is_empty());
        drop(completion);
        let mut completion = Completion::new("responses");
        assert_eq!(
            completion
                .push(&vec![b'x'; MAX_PENDING + 1])
                .err()
                .unwrap()
                .code,
            "resource_limit"
        );
        let mut completion = Completion::new("responses");
        let bytes =
            b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}";
        assert!(completion.push(bytes).unwrap().is_empty());
        assert_eq!(completion.finish().concat(), bytes);
    }
}
