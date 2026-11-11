use crate::backend::{Backend, ConnectionContext};
use local_agent_core::providers::Transaction;
use serde_json::Value;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
mod processes;

static RESTARTING: AtomicBool = AtomicBool::new(false);
struct RestartGuard;
impl Drop for RestartGuard {
    fn drop(&mut self) {
        RESTARTING.store(false, Ordering::Release);
    }
}

fn desktop(product: &str) -> Result<(&'static str, &'static str), String> {
    match product {
        "claude-code-desktop" => Ok(("claude-desktop", "com.anthropic.claudefordesktop")),
        "codex-desktop" => Ok(("codex", "com.openai.codex")),
        "opencode" | "opencode-desktop" => Ok(("opencode", "ai.opencode.desktop")),
        _ => Err("CLI 请在原终端退出，确认应用后再使用原启动方式启动。".into()),
    }
}
fn validate_scope(transaction: &Transaction, agent: &str, id: &str) -> Result<(), String> {
    if transaction.id != id
        || transaction.agent_id != agent
        || transaction.before.agent_id != agent
        || !["pending", "applied"].contains(&transaction.state.as_str())
    {
        return Err("配置事务与当前 Agent 不一致或已结束，请重新读取。".into());
    }
    Ok(())
}
fn validate_applied(transaction: &Transaction, agent: &str, id: &str) -> Result<(), String> {
    validate_scope(transaction, agent, id)?;
    if transaction.state != "applied" {
        return Err("事务尚未确认应用，未自动启动 Agent；请核查原后台。".into());
    }
    Ok(())
}
fn validate_processes(processes: Option<u64>, total: usize, owned: usize) -> Result<(), String> {
    match processes {
        None => Err("无法确认相关进程，尚未退出或修改 Agent。请稍后重新检查。".into()),
        Some(count) if count != total as u64 => Err("检查期间相关进程已变化，尚未退出或修改 Agent。请重新检查后重试。".into()),
        Some(_) if total != owned => Err("还有其他相关进程（可能包含 CLI）。请先保存任务并退出相关 CLI，再重试；当前配置未修改，桌面端未退出。".into()),
        _ => Ok(()),
    }
}
fn reason(error: &str) -> String {
    serde_json::from_str::<Value>(error)
        .ok()
        .and_then(|value| value["message"].as_str().map(str::to_owned))
        .unwrap_or_else(|| error.to_owned())
}
fn transaction_state<'a>(status: &'a Value, id: &str) -> Option<&'a str> {
    status["transactions"]
        .as_array()?
        .iter()
        .find(|t| t["id"] == id)?["state"]
        .as_str()
}

#[tauri::command]
pub(crate) async fn restart_agent_provider(
    app: tauri::AppHandle,
    backend: tauri::State<'_, Backend>,
    transaction_id: String,
    agent_product_id: String,
) -> Result<Value, String> {
    uuid::Uuid::parse_str(&transaction_id).map_err(|_| "配置事务 ID 无效")?;
    let (agent, bundle) = desktop(&agent_product_id)?;
    RESTARTING
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .map_err(|_| "已有 Agent 重启操作，请等待完成。")?;
    let _guard = RestartGuard;
    let context = backend.context()?;
    let path = format!("/api/provider-switches/{transaction_id}");
    let checked = context
        .request(reqwest::Method::POST, &format!("{path}/check"), None)
        .await?;
    let transaction: Transaction =
        serde_json::from_value(checked).map_err(|_| "配置事务内容无效")?;
    validate_scope(&transaction, agent, &transaction_id)?;
    let status = context
        .request(reqwest::Method::GET, "/api/provider-status", None)
        .await?;
    let processes = status["runtimes"]
        .as_array()
        .and_then(|items| items.iter().find(|r| r["agentId"] == agent))
        .and_then(|r| r["processes"].as_u64());
    let roots = native(&app, &context, bundle, Operation::Inspect).await?;
    let coverage = processes::read(agent, &roots).await?;
    validate_processes(processes, coverage.0, coverage.1)?;
    native(&app, &context, bundle, Operation::Quit).await?;
    let exited = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if native(&app, &context, bundle, Operation::Inspect)
                .await?
                .is_empty()
                && processes::read(agent, &[]).await?.0 == 0
            {
                return Ok::<_, String>(());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })
    .await;
    match exited {
        Ok(Ok(())) => (),
        _ => return Err("Agent 尚未确认退出，配置未写入。请完成客户端的保存或退出提示后重试；不会强制结束任务。".into()),
    }
    let applied = context
        .request(reqwest::Method::POST, &format!("{path}/apply"), None)
        .await;
    if let Err(error) = applied {
        // A timed-out IPC request may still commit. Read under the service's manager lock
        // before reopening, so the client cannot race an unconfirmed file write.
        let readback = context
            .origin_request(reqwest::Method::GET, "/api/provider-status", None)
            .await
            .ok();
        match readback.as_ref().and_then(|status| transaction_state(status, &transaction_id)) {
            Some("applied") => (),
            Some("pending" | "conflict" | "cancelled") => {
                let recovery = if !roots.is_empty() { native(&app, &context, bundle, Operation::Launch).await } else { Ok(vec![]) };
                return Err(format!("配置应用未完成：{}。{}", reason(&error), if recovery.is_ok() && !roots.is_empty() {
                    "已请求重新打开原 Agent，请重新读取配置"
                } else { "请重新读取配置，必要时手动打开 Agent" }));
            }
            _ => return Err("无法确认配置写入结果，尚未自动重新打开客户端。请重新读取事务状态后再手动启动；备份仍保留。".into()),
        }
    }
    let checked = context
        .request(reqwest::Method::POST, &format!("{path}/check"), None)
        .await
        .map_err(|e| {
            format!(
                "原后台配置已写入，但重启前核对未完成：{}。请核对后手动打开 Agent。",
                reason(&e)
            )
        })?;
    let checked: Transaction = serde_json::from_value(checked)
        .map_err(|_| "重启前回读内容不可核对，未自动启动 Agent；请核查原后台。")?;
    validate_applied(&checked, agent, &transaction_id)?;
    native(&app, &context, bundle, Operation::Launch)
        .await
        .map_err(|e| {
            format!("配置已写入，但启动请求未完成：{e}。请手动打开 Agent；备份仍保留。")
        })?;
    context
        .origin_request(reqwest::Method::POST, &format!("{path}/launched"), None)
        .await
        .map_err(|e| format!("配置已写入并已请求启动，但状态记录未完成：{e}。请重新读取。"))
}

#[derive(Clone, Copy)]
enum Operation {
    Inspect,
    Quit,
    Launch,
}
struct QueuedOperation(Arc<AtomicBool>);
impl Drop for QueuedOperation {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
#[cfg(any(target_os = "macos", test))]
fn native_scope(context: &ConnectionContext, permitted: &AtomicBool) -> Result<(), String> {
    if !permitted.load(Ordering::Acquire) {
        return Err("本次桌面操作已结束，未继续退出或启动 Agent。".into());
    }
    context.ensure_current()
}

#[cfg(target_os = "macos")]
async fn native(
    app: &tauri::AppHandle,
    context: &ConnectionContext,
    bundle: &'static str,
    operation: Operation,
) -> Result<Vec<u32>, String> {
    let (send, receive) = tokio::sync::oneshot::channel();
    let permitted = Arc::new(AtomicBool::new(true));
    let _queued = QueuedOperation(permitted.clone());
    let context = context.clone();
    app.run_on_main_thread(move || {
        if !permitted.load(Ordering::Acquire) {
            return;
        }
        use objc2_app_kit::{NSRunningApplication, NSWorkspace};
        use objc2_foundation::NSString;
        let bundle = NSString::from_str(bundle);
        let workspace = NSWorkspace::sharedWorkspace();
        let result = native_scope(&context, &permitted).and_then(|_| {
            let url = workspace
                .URLForApplicationWithBundleIdentifier(&bundle)
                .ok_or_else(|| "未找到对应桌面 App，请选择稍后手动重启。".to_string())?;
            let running = NSRunningApplication::runningApplicationsWithBundleIdentifier(&bundle);
            let roots = running
                .iter()
                .map(|application| {
                    u32::try_from(application.processIdentifier())
                        .map_err(|_| "无法确认桌面实例进程，未执行操作。".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?;
            match operation {
                Operation::Inspect => (),
                Operation::Quit => {
                    for application in running.iter() {
                        if !application.terminate() {
                            return Err("客户端未接受退出请求，配置尚未写入。".into());
                        }
                    }
                }
                Operation::Launch => {
                    if !workspace.openURL(&url) {
                        return Err("系统未接受打开客户端的请求。".into());
                    }
                }
            }
            Ok(roots)
        });
        let _ = send.send(result);
    })
    .map_err(|_| "无法联系桌面窗口，请重试。")?;
    tokio::time::timeout(Duration::from_secs(5), receive)
        .await
        .map_err(|_| {
            "桌面操作超时，已发出的退出或启动请求可能仍在处理，请重新检查客户端状态。".to_string()
        })?
        .map_err(|_| "桌面操作未返回，请重试。".to_string())?
}

#[cfg(not(target_os = "macos"))]
async fn native(
    _: &tauri::AppHandle,
    _: &ConnectionContext,
    _: &'static str,
    _: Operation,
) -> Result<Vec<u32>, String> {
    Err("当前系统请手动退出并重新启动 Agent。".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_readback_must_match_the_exact_transaction_agent_and_applied_state() {
        let mut receipt:Transaction=serde_json::from_value(serde_json::json!({"id":"target","agentId":"claude-desktop","state":"applied","createdAt":"fixture","before":{"agentId":"claude-desktop","path":"/fixture","state":"configured","mode":"api","providerName":"fixture","credential":"set","warnings":[]},"targetName":"fixture","backupPath":"/fixture-backup"})).unwrap();
        validate_applied(&receipt, "claude-desktop", "target").unwrap();
        receipt.state = "pending".into();
        validate_scope(&receipt, "claude-desktop", "target").unwrap();
        assert!(validate_applied(&receipt, "claude-desktop", "target").is_err());
        receipt.state = "applied".into();
        receipt.id = "other".into();
        assert!(validate_applied(&receipt, "claude-desktop", "target").is_err());
        receipt.id = "target".into();
        receipt.before.agent_id = "codex".into();
        assert!(validate_applied(&receipt, "claude-desktop", "target").is_err());
    }
    #[test]
    fn queued_native_work_cannot_run_after_cancellation_or_connection_change() {
        let backend = Backend::test_connection("http://127.0.0.1:1".into());
        let context = backend.context().unwrap();
        let permitted = AtomicBool::new(true);
        native_scope(&context, &permitted).unwrap();
        permitted.store(false, Ordering::Release);
        assert!(native_scope(&context, &permitted).is_err());
        permitted.store(true, Ordering::Release);
        backend.test_install_connection("http://127.0.0.1:2".into());
        let mut native_calls = 0;
        let result = native_scope(&context, &permitted).map(|_| native_calls += 1);
        assert!(result.is_err());
        assert_eq!(native_calls, 0);
    }
    #[test]
    fn restart_only_targets_known_desktop_products_and_shared_scope() {
        assert_eq!(desktop("claude-code-desktop").unwrap().0, "claude-desktop");
        assert_eq!(desktop("codex-desktop").unwrap().0, "codex");
        assert_eq!(desktop("opencode").unwrap().0, "opencode");
        assert_eq!(
            desktop("opencode-desktop").unwrap(),
            ("opencode", "ai.opencode.desktop")
        );
        for product in [
            "claude-code-cli",
            "codex-cli",
            "opencode-cli",
            "../Applications/Terminal.app",
            "",
        ] {
            assert!(desktop(product).is_err());
        }
    }
    #[test]
    fn unknown_or_extra_processes_never_trigger_shutdown() {
        assert!(validate_processes(None, 1, 1).is_err());
        assert!(validate_processes(Some(2), 2, 1).is_err());
        assert!(validate_processes(Some(1), 2, 2).is_err());
        assert!(validate_processes(Some(3), 3, 3).is_ok());
        assert!(validate_processes(Some(0), 0, 0).is_ok());
    }
    #[test]
    fn abandoned_main_thread_operation_is_cancelled_before_execution() {
        let permitted = Arc::new(AtomicBool::new(true));
        {
            let _operation = QueuedOperation(permitted.clone());
            assert!(permitted.load(Ordering::Acquire));
        }
        assert!(!permitted.load(Ordering::Acquire));
    }
    #[test]
    fn uncertain_write_requires_readback_of_the_exact_transaction() {
        let status = serde_json::json!({"transactions":[{"id":"other","state":"applied"},{"id":"target","state":"pending"}]});
        assert_eq!(transaction_state(&status, "target"), Some("pending"));
        assert_eq!(transaction_state(&status, "missing"), None);
        assert_eq!(transaction_state(&serde_json::json!({}), "target"), None);
    }
}
