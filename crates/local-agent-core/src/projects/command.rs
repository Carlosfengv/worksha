use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

// Fixed binaries/arguments only. Never execute a shell, repository hooks or inherited Git routing.
pub(super) fn run(binary: &str, args: &[&str], cwd: Option<&Path>) -> Result<Vec<u8>, String> {
    let mut command = Command::new(binary);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            command.env_remove(key);
        }
    }
    command
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C");
    let mut child = command.spawn().map_err(|_| "本机采集命令不可用")?;
    let stdout = child.stdout.take().ok_or("无法读取采集结果")?;
    let stderr = child.stderr.take().ok_or("无法读取采集状态")?;
    let errors = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.take(16385).read_to_end(&mut bytes).map(|_| bytes)
    });
    let reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(2 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                let _ = errors.join();
                return Err("采集进程状态不可读".into());
            }
        }
        if start.elapsed() > Duration::from_millis(1200) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            let _ = errors.join();
            return Err("采集超时，保留未知状态，请稍后刷新".into());
        }
        thread::sleep(Duration::from_millis(10));
    };
    let bytes = reader
        .join()
        .map_err(|_| "采集读取中断")?
        .map_err(|_| "采集读取失败")?;
    let errors = errors
        .join()
        .map_err(|_| "采集状态读取中断")?
        .map_err(|_| "采集状态读取失败")?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("采集结果超过 2 MiB，未视为完整快照".into());
    }
    if !errors.is_empty()
        || !(status.success()
            || binary.ends_with("lsof") && status.code() == Some(1) && bytes.is_empty())
    {
        return Err("当前目录或进程不可读取，采集未完成".into());
    }
    Ok(bytes)
}
