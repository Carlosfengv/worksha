use std::{
    collections::{HashMap, HashSet},
    path::Path,
    process::Stdio,
    time::Duration,
};
use tokio::io::AsyncReadExt;

const UNKNOWN: &str = "无法确认相关进程归属，尚未退出或修改 Agent。请重新检查。";

// Only PID / parent PID / executable names are read; never arguments or environment.
pub(super) async fn read(agent: &str, roots: &[u32]) -> Result<(usize, usize), String> {
    let mut child = tokio::process::Command::new("/bin/ps")
        .args([
            "-U",
            &local_agent_core::providers::current_uid().to_string(),
            "-o",
            "pid=,ppid=,comm=",
        ])
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| UNKNOWN.to_string())?;
    let stdout = child.stdout.take().ok_or(UNKNOWN)?;
    tokio::time::timeout(Duration::from_secs(2), async {
        let mut bytes = Vec::new();
        stdout
            .take(256 * 1024 + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| UNKNOWN)?;
        if bytes.len() > 256 * 1024 || !child.wait().await.map_err(|_| UNKNOWN)?.success() {
            return Err(UNKNOWN.to_string());
        }
        classify(
            agent,
            std::str::from_utf8(&bytes).map_err(|_| UNKNOWN)?,
            roots,
        )
        .map_err(str::to_string)
    })
    .await
    .map_err(|_| UNKNOWN.to_string())?
}

fn classify(agent: &str, text: &str, roots: &[u32]) -> Result<(usize, usize), &'static str> {
    let mut parents = HashMap::new();
    let mut matching = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        let (pid, rest) = line.trim().split_once(char::is_whitespace).ok_or(UNKNOWN)?;
        let (parent, executable) = rest.trim().split_once(char::is_whitespace).ok_or(UNKNOWN)?;
        let pid = pid.parse::<u32>().map_err(|_| UNKNOWN)?;
        let parent = parent.parse::<u32>().map_err(|_| UNKNOWN)?;
        if pid == 0 || parents.insert(pid, parent).is_some() {
            return Err(UNKNOWN);
        }
        let base = Path::new(executable.trim())
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(UNKNOWN)?;
        let relevant = match agent {
            "codex" => ["codex", "Codex", "ChatGPT"].contains(&base),
            "opencode" => ["opencode", "OpenCode"].contains(&base),
            "claude-desktop" => ["claude-desktop", "Claude"].contains(&base),
            _ => return Err(UNKNOWN),
        };
        if relevant {
            matching.push(pid);
        }
    }
    let owned = matching
        .iter()
        .filter(|pid| {
            let mut current = **pid;
            let mut seen = HashSet::new();
            while current > 1 && seen.insert(current) {
                if roots.contains(&current) {
                    return true;
                }
                let Some(parent) = parents.get(&current) else {
                    return false;
                };
                current = *parent;
            }
            false
        })
        .count();
    Ok((matching.len(), owned))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_helpers_are_owned_but_independent_terminal_clients_are_not() {
        let processes = "10 1 /Applications/ChatGPT.app/Contents/MacOS/ChatGPT\n11 10 /app/codex\n12 10 /app/node_repl\n13 12 /app/codex\n20 1 /bin/zsh\n21 20 /app/codex\n";
        assert_eq!(classify("codex", processes, &[10]).unwrap(), (4, 3));
        assert_eq!(classify("codex", processes, &[]).unwrap(), (4, 0));
        assert_eq!(
            classify("opencode", "10 1 /app/OpenCode\n11 10 /app/opencode", &[10]).unwrap(),
            (2, 2)
        );
        assert_eq!(
            classify(
                "claude-desktop",
                "10 1 /app/Claude\n11 10 /app/claude",
                &[10]
            )
            .unwrap(),
            (1, 1)
        );
    }
    #[test]
    fn malformed_missing_or_cyclic_ancestry_never_proves_desktop_ownership() {
        assert!(classify("codex", "broken", &[10]).is_err());
        assert!(classify("codex", "11 1 codex\n11 10 codex", &[10]).is_err());
        assert_eq!(
            classify("codex", "11 12 codex\n12 11 node", &[10]).unwrap(),
            (1, 0)
        );
        assert_eq!(classify("codex", "11 99 codex", &[10]).unwrap(), (1, 0));
        assert!(classify("invalid", "11 1 codex", &[10]).is_err());
    }
}
