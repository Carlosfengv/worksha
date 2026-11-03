//! Independent bounded admission for synchronous management work and data queries.
use local_agent_core::{Store, providers::ConfigError};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::Semaphore;

#[cfg(test)]
mod query_tests;

#[derive(Debug)]
pub(crate) enum Error {
    Busy,
    Unavailable,
}

pub(crate) fn config_error(error: Error) -> ConfigError {
    match error {
        Error::Busy => ConfigError::new("busy", "本机同步任务繁忙，请稍后重试"),
        Error::Unavailable => ConfigError::new("unavailable", "本机同步任务未能完成"),
    }
}

/// Acquire the SQLite mutex on the blocking worker, including lock contention.
/// The closure must finish before a request is sent to an upstream provider.
pub(crate) async fn store<T: Send + 'static>(
    store: Arc<Mutex<Store>>,
    work: impl FnOnce(&Store) -> Result<T, ConfigError> + Send + 'static,
) -> Result<T, ConfigError> {
    run(move || {
        let store = store
            .lock()
            .map_err(|_| ConfigError::new("unavailable", "配置存储暂不可用"))?;
        work(&store)
    })
    .await
    .map_err(config_error)?
}

pub(crate) async fn secret(
    vault: Arc<dyn local_agent_core::providers::vault::Vault>,
    reference: String,
) -> Result<zeroize::Zeroizing<Vec<u8>>, local_agent_core::providers::ConfigError> {
    use local_agent_core::providers::ConfigError;
    uuid::Uuid::parse_str(&reference)
        .map_err(|_| ConfigError::new("invalid_credential", "凭据引用无效"))?;
    run(move || vault.get(&reference))
        .await
        .map_err(|error| match error {
            Error::Busy => ConfigError::new("busy", "凭据读取繁忙，请稍后重试"),
            Error::Unavailable => ConfigError::new("unavailable", "凭据服务暂不可用"),
        })?
}

pub(crate) async fn run<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Error> {
    run_with(capacity(), work).await
}

pub(crate) async fn management(
    work: impl FnOnce() -> crate::ApiResult + Send + 'static,
) -> crate::ApiResult {
    run(work).await.map_err(|error| {
        let (code, message, unknown) = match error {
            Error::Busy => ("busy", "本机管理任务繁忙，请稍后重试", false),
            Error::Unavailable => ("unavailable", "本机管理任务未能完成", true),
        };
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({"code":code,"message":message,"outcomeUnknown":unknown})),
        )
    })?
}

// Historical reads may wait on SQL or native files. They must not exhaust the
// separate 32 management slots used by consent, credentials and operation checks.
pub(crate) fn query_capacity() -> Arc<Semaphore> {
    Arc::new(Semaphore::new(8))
}

pub(crate) async fn query(
    capacity: Arc<Semaphore>,
    work: impl FnOnce() -> crate::ApiResult + Send + 'static,
) -> crate::ApiResult {
    run_with(capacity, work).await.map_err(|error| {
        let (code, message) = match error {
            Error::Busy => ("busy", "数据读取繁忙，请稍后重试"),
            Error::Unavailable => ("unavailable", "数据读取暂不可用，请稍后重试"),
        };
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!({"code":code,"message":message})),
        )
    })?
}

fn capacity() -> Arc<Semaphore> {
    static CAPACITY: OnceLock<Arc<Semaphore>> = OnceLock::new();
    CAPACITY
        .get_or_init(|| Arc::new(Semaphore::new(32)))
        .clone()
}

/// Admit drop-time metadata without an unbounded async task queue.
pub(crate) fn try_spawn<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<tokio::task::JoinHandle<T>, Error> {
    spawn_with(capacity(), work)
}

pub(crate) async fn run_with<T: Send + 'static>(
    capacity: Arc<Semaphore>,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, Error> {
    spawn_with(capacity, work)?
        .await
        .map_err(|_| Error::Unavailable)
}

fn spawn_with<T: Send + 'static>(
    capacity: Arc<Semaphore>,
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<tokio::task::JoinHandle<T>, Error> {
    let runtime = tokio::runtime::Handle::try_current().map_err(|_| Error::Unavailable)?;
    let permit = capacity.try_acquire_owned().map_err(|_| Error::Busy)?;
    Ok(runtime.spawn_blocking(move || {
        // Cancelling the caller cannot free admission while its synchronous
        // operation is still running in the blocking pool.
        let _permit = permit;
        work()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejected_work_releases_captured_state_without_a_runtime() {
        struct Release(Arc<std::sync::atomic::AtomicBool>);
        impl Drop for Release {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::Release);
            }
        }
        let released = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let guard = Release(released.clone());
        let capacity = Arc::new(Semaphore::new(1));
        let result = spawn_with(capacity.clone(), move || drop(guard));
        assert!(matches!(result, Err(Error::Unavailable)));
        assert!(released.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(capacity.available_permits(), 1);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn slow_work_keeps_the_runtime_responsive_and_holds_capacity_after_cancellation() {
        let capacity = Arc::new(Semaphore::new(1));
        let started = Arc::new(tokio::sync::Notify::new());
        let (release, gate) = std::sync::mpsc::channel();
        let worker = tokio::spawn({
            let capacity = capacity.clone();
            let started = started.clone();
            async move {
                run_with(capacity, move || {
                    started.notify_one();
                    gate.recv_timeout(std::time::Duration::from_secs(3))
                        .unwrap();
                })
                .await
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), started.notified())
            .await
            .unwrap();
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            tokio::time::sleep(std::time::Duration::from_millis(5)),
        )
        .await
        .unwrap();
        worker.abort();
        assert!(worker.await.unwrap_err().is_cancelled());
        assert!(matches!(
            run_with(capacity.clone(), || panic!(
                "overloaded work must not start"
            ))
            .await,
            Err(Error::Busy)
        ));
        release.send(()).unwrap();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        while capacity.available_permits() == 0 {
            assert!(tokio::time::Instant::now() < deadline);
            tokio::task::yield_now().await;
        }
        assert_eq!(run_with(capacity, || 42).await.unwrap(), 42);
    }
}
