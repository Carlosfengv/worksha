//! Bounded, read-only logfmt diagnostics, associated only by an explicit native session ID.
use super::{failure, parser, safe_path};
use chrono::DateTime;
use regex::Regex;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, VecDeque},
    fs::OpenOptions,
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::OnceLock,
};

const TAIL: u64 = 2 * 1024 * 1024;
const MAX_LINE: u64 = 64 * 1024;
const MAX_ERRORS: usize = 1000;

#[derive(Clone, Serialize)]
pub(super) struct Diagnostic {
    pub timestamp: String,
    pub model: String,
    pub provider: String,
    pub failure: failure::ModelFailure,
}
#[derive(Default)]
pub(super) struct Logs {
    pub sessions: HashMap<String, Vec<Diagnostic>>,
    pub warnings: Vec<String>,
}
pub(super) fn path(home: &Path) -> PathBuf {
    home.join(".local/share/opencode/log/opencode.log")
}

fn fields(line: &str) -> HashMap<String, String> {
    static FIELDS: OnceLock<Regex> = OnceLock::new();
    let regex = FIELDS.get_or_init(|| {
        Regex::new(r#"(?:^|\s)([A-Za-z_][A-Za-z0-9_.-]*)=("(?:\\.|[^"\\])*"|[^\s]+)"#).unwrap()
    });
    regex
        .captures_iter(line)
        .filter_map(|c| {
            let key = &c[1];
            if ![
                "timestamp",
                "level",
                "message",
                "session.id",
                "sessionID",
                "providerID",
                "modelID",
                "error.error",
            ]
            .contains(&key)
            {
                return None;
            }
            let value = if c[2].starts_with('"') {
                serde_json::from_str::<String>(&c[2]).ok()?
            } else {
                c[2].to_string()
            };
            Some((key.into(), value))
        })
        .collect()
}

pub(super) fn load(home: &Path) -> Logs {
    let mut result = Logs::default();
    let path = path(home);
    if !path.exists() && path.symlink_metadata().is_err() {
        return result;
    }
    let read = || -> std::io::Result<_> {
        if !safe_path(home, &path) || !path.symlink_metadata()?.is_file() {
            return Err(std::io::Error::other("unsafe log"));
        }
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
        }
        let mut file = options.open(&path)?;
        let meta = file.metadata()?;
        if !meta.is_file() {
            return Err(std::io::Error::other("not a regular log"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if meta.nlink() > 1 {
                return Err(std::io::Error::other("hardlinked log"));
            }
        }
        let offset = meta.len().saturating_sub(TAIL);
        file.seek(SeekFrom::Start(offset))?;
        Ok((BufReader::new(file.take(meta.len() - offset)), offset > 0))
    };
    let Ok((mut reader, tail)) = read() else {
        result
            .warnings
            .push("OpenCode 诊断日志不可读取或超出安全来源；不能据此认定没有调用异常。".into());
        return result;
    };
    if tail {
        result
            .warnings
            .push("OpenCode 诊断仅读取当前日志末尾 2 MiB，较早错误可能未覆盖。".into());
    }
    let mut bytes = Vec::new();
    if tail {
        let _ = std::io::Read::by_ref(&mut reader)
            .take(MAX_LINE + 1)
            .read_until(b'\n', &mut bytes);
    }
    let mut count = 0;
    let mut recent = VecDeque::new();
    loop {
        bytes.clear();
        match std::io::Read::by_ref(&mut reader)
            .take(MAX_LINE + 1)
            .read_until(b'\n', &mut bytes)
        {
            Ok(0) => break,
            Ok(_) if bytes.len() as u64 > MAX_LINE => {
                result
                    .warnings
                    .push("OpenCode 日志存在超出 64 KiB 的行，已跳过剩余诊断。".into());
                break;
            }
            Ok(_) if !bytes.ends_with(b"\n") => break,
            Err(_) => {
                result.warnings.push("OpenCode 诊断日志读取中断。".into());
                break;
            }
            _ => {}
        }
        let Ok(line) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let fields = fields(line);
        if fields.get("level").map(String::as_str) != Some("ERROR")
            || fields.get("message").map(String::as_str) != Some("stream error")
        {
            continue;
        }
        let Some(session) = fields.get("session.id").or_else(|| fields.get("sessionID")) else {
            continue;
        };
        if !super::opencode::valid_id(session) {
            continue;
        }
        let (Some(time), Some(model), Some(provider), Some(message)) = (
            fields.get("timestamp"),
            fields.get("modelID"),
            fields.get("providerID"),
            fields.get("error.error"),
        ) else {
            continue;
        };
        let Ok(time) = DateTime::parse_from_rfc3339(time) else {
            continue;
        };
        if message.trim().is_empty() {
            continue;
        }
        count += 1;
        if recent.len() == MAX_ERRORS {
            recent.pop_front();
        }
        let code = message
            .split_once(':')
            .map(|(code, _)| code)
            .filter(|code| {
                !code.is_empty()
                    && code.len() <= 80
                    && code.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
            });
        recent.push_back((
            session.clone(),
            Diagnostic {
                timestamp: time.to_rfc3339(),
                model: parser::safe_text(model, 300).0,
                provider: parser::safe_text(provider, 300).0,
                failure: failure::from_parts(code, Some(message), None, None, "opencode_log"),
            },
        ));
    }
    if count > MAX_ERRORS {
        result
            .warnings
            .push("OpenCode 诊断超过 1000 条，仅保留当前日志范围内最近 1000 条。".into());
    }
    for (session, diagnostic) in recent {
        result.sessions.entry(session).or_default().push(diagnostic);
    }
    result.warnings.sort();
    result.warnings.dedup();
    result
}

pub(super) fn fingerprint(diagnostics: &[Diagnostic]) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(diagnostics).unwrap_or_default())
    )
}
