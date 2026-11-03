use local_agent_core::runtime;
use std::{
    fs::File,
    path::Path,
    time::{Duration, Instant},
};

/// Hold the lock until the native event loop has exited, including backend teardown.
pub(crate) fn acquire(data: &Path, identifier: &str) -> Result<Option<File>, String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if reopen_existing(identifier)? {
            return Ok(None);
        }
        match runtime::lock(data, "desktop.lock") {
            Ok(guard) => return Ok(Some(guard)),
            Err(error) if error.code == "busy" && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) if error.code == "busy" => {
                return Err("已有 Local Agent 正在启动，请稍后重新打开".into());
            }
            Err(error) => return Err(error.message),
        }
    }
}

#[cfg(target_os = "macos")]
fn reopen_existing(identifier: &str) -> Result<bool, String> {
    use objc2_app_kit::{NSApplicationActivationOptions, NSRunningApplication};
    use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventSendOptions, NSString};
    let applications = NSRunningApplication::runningApplicationsWithBundleIdentifier(
        &NSString::from_str(identifier),
    );
    // Address the exact existing PID. Reopening a bundle URL through synchronous
    // Launch Services can block a second process before finishLaunching.
    for application in applications.iter() {
        if u32::try_from(application.processIdentifier()).ok() == Some(std::process::id())
            || application.isTerminated()
            || !application.isFinishedLaunching()
        {
            continue;
        }
        let target = NSAppleEventDescriptor::descriptorWithProcessIdentifier(
            application.processIdentifier(),
        );
        let event = NSAppleEventDescriptor::appleEventWithEventClass_eventID_targetDescriptor_returnID_transactionID(
            u32::from_be_bytes(*b"aevt"), u32::from_be_bytes(*b"rapp"), Some(&target), -1, 0,
        );
        event
            .sendEventWithOptions_timeout_error(
                NSAppleEventSendOptions::NoReply | NSAppleEventSendOptions::NeverInteract,
                1.0,
            )
            .map_err(|_| "无法唤起已有 Local Agent 窗口，请从 Dock 打开或退出后重试".to_string())?;
        application.activateWithOptions(NSApplicationActivationOptions::ActivateAllWindows);
        return Ok(true);
    }
    Ok(false)
}

#[cfg(not(target_os = "macos"))]
fn reopen_existing(_identifier: &str) -> Result<bool, String> {
    Ok(false)
}
