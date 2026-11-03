//! Bounded downstream backpressure. Reading a body is not acknowledgement by the Agent.
use super::Observation;
use axum::body::{Body, Bytes};
use futures_util::{Stream, StreamExt};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

const SEND_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const REQUEST_LIMIT: Duration = Duration::from_secs(30 * 60);
pub(super) const RECOVERY_LIMIT: Duration = Duration::from_secs(2);
pub(super) const RECOVERY_BYTES: usize = 256 * 1024;

#[derive(Default)]
pub(super) struct Recovery {
    cancelled: AtomicBool,
    bytes: AtomicUsize,
}
impl Recovery {
    fn begin(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    /// Bound raw upstream bytes as well as emitted downstream chunks.
    pub fn allows(&self, bytes: usize) -> bool {
        !self.cancelled.load(Ordering::Acquire)
            || self
                .bytes
                .fetch_add(bytes, Ordering::Relaxed)
                .saturating_add(bytes)
                <= RECOVERY_BYTES
    }
}

struct Reader {
    observation: Arc<Observation>,
    id: String,
    recovery: Arc<Recovery>,
    done: bool,
}
impl Drop for Reader {
    fn drop(&mut self) {
        if !self.done {
            self.recovery.begin();
            self.observation.hub.downstream(&self.id, "cancelled");
            self.observation.hub.finish(
                &self.id,
                "cancelled",
                Some("客户端结束读取，未收到完整结束事件"),
            );
        }
    }
}
pub fn body(
    input: impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static,
    observation: Arc<Observation>,
    id: String,
    recovery: Arc<Recovery>,
) -> Body {
    body_with_limits(
        input,
        observation,
        id,
        recovery,
        SEND_TIMEOUT,
        REQUEST_LIMIT,
    )
}
fn body_with_limits(
    input: impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static,
    observation: Arc<Observation>,
    id: String,
    recovery: Arc<Recovery>,
    send_timeout: Duration,
    request_limit: Duration,
) -> Body {
    let (sender, mut receiver) = tokio::sync::mpsc::channel(2);
    let producer_observation = observation.clone();
    let producer_id = id.clone();
    let producer_recovery = recovery.clone();
    tokio::spawn(async move {
        let mut input = std::pin::pin!(input);
        let deadline = tokio::time::Instant::now() + request_limit;
        loop {
            let item = tokio::select! {
                _ = sender.closed() => {
                    producer_recovery.begin();
                    producer_observation.hub.downstream(&producer_id, "cancelled");
                    producer_observation.hub.finish(&producer_id, "cancelled", Some("客户端结束读取，未收到完整结束事件"));
                    // Do not resend the request or wait for arbitrary model generation.
                    // Briefly read the existing response for its final accounting only.
                    let recovery_deadline = deadline.min(tokio::time::Instant::now() + RECOVERY_LIMIT);
                    let mut bytes = 0usize;
                    while let Ok(Some(Ok(chunk))) = tokio::time::timeout_at(recovery_deadline, input.next()).await {
                        bytes = bytes.saturating_add(chunk.len());
                        if bytes >= RECOVERY_BYTES { break; }
                    }
                    break;
                },
                item = tokio::time::timeout_at(deadline, input.next()) => item,
            };
            let item = match item {
                Ok(Some(item)) => item,
                Ok(None) => break,
                Err(_) => {
                    producer_observation.hub.finish(
                        &producer_id,
                        "interrupted",
                        Some("请求超过最大持续时间，结果未确认"),
                    );
                    producer_observation.hub.downstream(&producer_id, "timeout");
                    let _ = sender.try_send(Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "请求持续时间超限",
                    )));
                    break;
                }
            };
            match tokio::time::timeout_at(
                (tokio::time::Instant::now() + send_timeout).min(deadline),
                sender.send(item),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(_)) => continue,
                Err(_) => {
                    producer_observation.hub.finish(
                        &producer_id,
                        "interrupted",
                        Some("客户端长时间未读取响应，转发已停止"),
                    );
                    producer_observation.hub.downstream(&producer_id, "timeout");
                    break;
                }
            }
        }
    });
    let reader = Reader {
        observation,
        id,
        recovery,
        done: false,
    };
    Body::from_stream(async_stream::stream! {
        let mut reader = reader;
        while let Some(item) = receiver.recv().await { yield item; }
        reader.observation.hub.downstream(&reader.id, "body_read");
        reader.done = true;
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recovery_counts_raw_bytes_only_after_cancellation() {
        let recovery = Recovery::default();
        assert!(recovery.allows(RECOVERY_BYTES * 2));
        recovery.begin();
        assert!(recovery.allows(RECOVERY_BYTES - 1));
        assert!(recovery.allows(1));
        assert!(!recovery.allows(1));
        assert!(!recovery.allows(1));
    }
    fn observation() -> (tempfile::TempDir, Arc<Observation>) {
        let temp = tempfile::tempdir().unwrap();
        let observation = Arc::new(
            Observation::new(&temp.path().canonicalize().unwrap(), "127.0.0.1:9").unwrap(),
        );
        let row = serde_json::from_value(serde_json::json!({
            "id":"r","agentId":"claude","provider":"fixture","endpoint":"http://localhost",
            "bindingRevision":1,"startedAt":"2026-10-21T00:00:00Z","status":"in_progress",
            "protocol":"messages","revision":0,"usage":{}
        }))
        .unwrap();
        observation.hub.start(row, std::time::Instant::now(), None);
        (temp, observation)
    }
    #[tokio::test]
    async fn downstream_stall_is_bounded_and_releases_the_request_slot() {
        let (_temp, observation) = observation();
        let permit = observation.capacity.clone().acquire_owned().await.unwrap();
        let input = async_stream::stream! {
            let _permit = permit;
            loop { yield Ok(Bytes::from_static(b"chunk")); }
        };
        let body = body_with_limits(
            input,
            observation.clone(),
            "r".into(),
            Arc::default(),
            Duration::from_millis(20),
            Duration::from_secs(2),
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            while observation.capacity.available_permits() != 64 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let rows = observation.hub.rows();
        assert_eq!(rows[0].status, "interrupted");
        assert_eq!(rows[0].downstream_status.as_deref(), Some("timeout"));
        drop(body);
        assert_eq!(
            observation.hub.rows()[0].downstream_status.as_deref(),
            Some("timeout")
        );
    }
    #[tokio::test]
    async fn heartbeats_do_not_extend_the_response_budget_or_overwrite_upstream_completion() {
        for completed in [false, true] {
            let (_temp, observation) = observation();
            if completed {
                observation.hub.finish("r", "success", None);
            }
            let permit = observation.capacity.clone().acquire_owned().await.unwrap();
            let input = async_stream::stream! {
                let _permit = permit;
                loop {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    yield Ok(Bytes::from_static(b": heartbeat\n\n"));
                }
            };
            let body = body_with_limits(
                input,
                observation.clone(),
                "r".into(),
                Arc::default(),
                Duration::from_secs(1),
                Duration::from_millis(40),
            );
            assert!(
                tokio::time::timeout(Duration::from_secs(2), axum::body::to_bytes(body, 1024))
                    .await
                    .unwrap()
                    .is_err()
            );
            assert_eq!(observation.capacity.available_permits(), 64);
            let rows = observation.hub.rows();
            assert_eq!(
                rows[0].status,
                if completed { "success" } else { "interrupted" }
            );
            assert_eq!(rows[0].downstream_status.as_deref(), Some("timeout"));
        }
    }
    #[tokio::test]
    async fn dropping_an_unread_body_bounds_accounting_recovery_and_releases_capacity() {
        let (_temp, observation) = observation();
        let permit = observation.capacity.clone().acquire_owned().await.unwrap();
        let input = async_stream::stream! {
            let _permit = permit;
            std::future::pending::<()>().await;
            yield Ok(Bytes::new());
        };
        drop(body(input, observation.clone(), "r".into(), Arc::default()));
        tokio::time::timeout(RECOVERY_LIMIT + Duration::from_secs(1), async {
            while observation.capacity.available_permits() != 64 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            observation.hub.rows()[0].downstream_status.as_deref(),
            Some("cancelled")
        );
    }

    #[tokio::test]
    async fn cancelled_delivery_recovers_final_usage_without_success_or_late_text_timings() {
        let (temp, observation) = observation();
        let producer_observation = observation.clone();
        let input = async_stream::stream! {
            yield Ok(Bytes::from_static(b"first"));
            tokio::time::sleep(Duration::from_millis(30)).await;
            producer_observation.hub.update("r", super::super::stream::Update {
                generated:true, text:true, estimated_tokens:99.0,
                usage:Some(local_agent_core::sessions::Usage {
                    input_tokens:Some(100),output_tokens:Some(7),cached_input_tokens:Some(80),..Default::default()
                }),..Default::default()
            });
            producer_observation.hub.finish("r", "success", None);
            yield Ok(Bytes::from_static(b"final"));
        };
        let mut body =
            body(input, observation.clone(), "r".into(), Arc::default()).into_data_stream();
        assert_eq!(
            body.next().await.unwrap().unwrap(),
            Bytes::from_static(b"first")
        );
        drop(body);
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if observation.hub.rows()[0].usage.output_tokens == Some(7) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        let row = observation.hub.rows().remove(0);
        assert_eq!(row.status, "cancelled");
        assert_eq!(row.downstream_status.as_deref(), Some("cancelled"));
        assert_eq!(row.usage.total_tokens, Some(107));
        assert!(row.ttft_ms.is_none() && row.first_text_ms.is_none());
        observation.hub.flush(Duration::from_secs(3)).unwrap();
        let restarted =
            Observation::new(&temp.path().canonicalize().unwrap(), "127.0.0.1:9").unwrap();
        let row = restarted.hub.rows().remove(0);
        assert_eq!(row.usage.output_tokens, Some(7));
        assert_eq!(row.status, "cancelled");
    }
}
