use super::{
    Builder, MAX_FILE, MAX_LINE, NativeError, NativeResult, PAGE, Parsed, ProjectCache, Window,
};
use crate::sessions::safe_path;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

#[cfg(test)]
pub fn parse(
    home: &Path,
    path: &Path,
    agent: &str,
    window: Option<Window<'_>>,
) -> NativeResult<Parsed> {
    parse_window(home, path, agent, window, None)
}

pub fn parse_snapshot(
    home: &Path,
    path: &Path,
    agent: &str,
    window: Option<Window<'_>>,
    snapshot_bytes: u64,
) -> NativeResult<Parsed> {
    parse_window(home, path, agent, window, Some(snapshot_bytes))
}

fn parse_window(
    home: &Path,
    path: &Path,
    agent: &str,
    window: Option<Window<'_>>,
    snapshot_bytes: Option<u64>,
) -> NativeResult<Parsed> {
    if let Some(anchor) = window.as_ref().and_then(|w| w.anchor) {
        let meta = parse_inner(home, path, agent, None, snapshot_bytes)?;
        let index = meta
            .events
            .iter()
            .position(|e| e.id == anchor)
            .ok_or_else(|| NativeError::new("not_found", "此调用在当前会话中不存在或来源已更新"))?;
        let parsed = parse_inner(
            home,
            path,
            agent,
            Some(Window {
                offset: index / PAGE * PAGE,
                anchor: None,
            }),
            Some(meta.snapshot_bytes),
        )?;
        if parsed.fingerprint != meta.fingerprint {
            return Err(NativeError::new(
                "stale_snapshot",
                "会话历史内容已改写，请刷新后重试",
            ));
        }
        return Ok(parsed);
    }
    parse_inner(home, path, agent, window, snapshot_bytes)
}

fn parse_inner(
    home: &Path,
    path: &Path,
    agent: &str,
    window: Option<Window<'_>>,
    snapshot_bytes: Option<u64>,
) -> NativeResult<Parsed> {
    parse_indexed(home, path, agent, window, snapshot_bytes, None, None)
}

#[derive(Clone)]
struct Continuation {
    builder: Builder,
    metadata: fs::Metadata,
    offset: u64,
    lines: usize,
    digest: Sha256,
    #[cfg(test)]
    decoded: usize,
}
#[derive(Default)]
pub(in crate::sessions) struct IndexCache {
    entries: HashMap<PathBuf, Continuation>,
    #[cfg(test)]
    pub snapshots: usize,
}
impl IndexCache {
    pub fn parse(
        &mut self,
        home: &Path,
        path: &Path,
        agent: &str,
        retain: bool,
        projects: &mut ProjectCache,
    ) -> NativeResult<Parsed> {
        let mut continuation = self.entries.remove(path);
        let parsed = parse_indexed(
            home,
            path,
            agent,
            None,
            None,
            retain.then_some(&mut continuation),
            Some(projects),
        )?;
        #[cfg(test)]
        if continuation.is_some() && retain {
            self.snapshots += 1;
        }
        if let Some(state) = continuation.filter(|s| retain && s.builder.weight() <= 32768) {
            while !self.entries.is_empty()
                && (self.entries.len() >= 8
                    || self
                        .entries
                        .values()
                        .map(|s| s.builder.weight())
                        .sum::<usize>()
                        + state.builder.weight()
                        > 65536)
            {
                let oldest = self
                    .entries
                    .iter()
                    .min_by_key(|(_, s)| s.metadata.modified().ok())
                    .map(|(p, _)| p.clone())
                    .unwrap();
                self.entries.remove(&oldest);
            }
            self.entries.insert(path.to_path_buf(), state);
        }
        Ok(parsed)
    }
    pub fn revoke(&mut self, agent: &str) {
        self.entries.retain(|_, state| state.builder.agent != agent);
    }
    #[cfg(test)]
    pub fn decoded_lines(&self, path: &Path) -> usize {
        self.entries.get(path).map_or(0, |s| s.decoded)
    }
    #[cfg(test)]
    pub fn retained_bodies(&self) -> usize {
        self.entries
            .values()
            .flat_map(|s| &s.builder.events)
            .filter(|e| {
                e.text.is_some()
                    || e.input.is_some()
                    || e.output.is_some()
                    || e.raw_usage.is_some()
                    || e.failure.as_ref().is_some_and(|f| f.message.is_some())
            })
            .count()
    }
}
fn parse_indexed(
    home: &Path,
    path: &Path,
    agent: &str,
    window: Option<Window<'_>>,
    snapshot_bytes: Option<u64>,
    mut continuation: Option<&mut Option<Continuation>>,
    projects: Option<&mut ProjectCache>,
) -> NativeResult<Parsed> {
    if !safe_path(home, path) {
        return Err(NativeError::new(
            "unauthorized",
            "会话路径超出授权来源或包含符号链接",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut file: File = options
        .open(path)
        .map_err(|_| NativeError::new("unavailable", "原生会话文件无法读取，请检查来源权限"))?;
    let metadata = file
        .metadata()
        .map_err(|_| NativeError::new("unavailable", "无法读取会话文件信息"))?;
    if !metadata.is_file() || metadata.len() > MAX_FILE {
        return Err(NativeError::new(
            "unsupported",
            "会话文件超出 512 MiB 读取限制或不是普通文件",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() > 1 {
            return Err(NativeError::new("unsupported", "不读取共享硬链接会话文件"));
        }
    }
    let fallback = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let mut builder = Builder::new(agent, &fallback, window.as_ref().map(|w| w.offset));
    let snapshot_bytes = snapshot_bytes.unwrap_or(metadata.len());
    if snapshot_bytes > metadata.len() {
        return Err(NativeError::new(
            "stale_snapshot",
            "会话文件已缩短，请刷新后重试",
        ));
    }
    let mut fingerprint = Sha256::new();
    let mut line = Vec::new();
    let mut line_number = 0;
    let mut bytes_read = 0;
    if let Some(previous) = continuation.as_ref().and_then(|state| state.as_ref()) {
        #[cfg(unix)]
        let same_file = {
            use std::os::unix::fs::MetadataExt;
            previous.metadata.dev() == metadata.dev() && previous.metadata.ino() == metadata.ino()
        };
        #[cfg(not(unix))]
        let same_file = previous.metadata.created().ok() == metadata.created().ok();
        if same_file && previous.builder.agent == agent && previous.offset <= snapshot_bytes {
            // Verify the entire consumed prefix: a rewrite followed by an append
            // must not reuse stale usage, even when size and header still match.
            let mut prefix = std::io::Read::by_ref(&mut file).take(previous.offset);
            let mut buffer = [0; 32768];
            let mut hash = Sha256::new();
            loop {
                let n = prefix
                    .read(&mut buffer)
                    .map_err(|_| NativeError::new("unavailable", "读取原生索引前缀失败"))?;
                if n == 0 {
                    break;
                }
                hash.update(&buffer[..n]);
            }
            if hash.clone().finalize() == previous.digest.clone().finalize() {
                builder = previous.builder.clone();
                fingerprint = hash;
                line_number = previous.lines;
                bytes_read = previous.offset;
            }
        }
        file.seek(SeekFrom::Start(bytes_read))
            .map_err(|_| NativeError::new("unavailable", "原生索引位置不可读取"))?;
    }
    #[cfg(test)]
    let prior_lines = line_number;
    let mut reader = BufReader::new(file.take(snapshot_bytes - bytes_read));
    let mut malformed = 0;
    let mut oversized = 0;
    let mut incomplete = false;
    loop {
        line.clear();
        let n = std::io::Read::by_ref(&mut reader)
            .take(MAX_LINE + 1)
            .read_until(b'\n', &mut line)
            .map_err(|_| NativeError::new("unavailable", "读取会话内容失败"))?;
        if n == 0 {
            break;
        }
        bytes_read += n as u64;
        fingerprint.update(&line);
        line_number += 1;
        if bytes_read > MAX_FILE {
            oversized += 1;
            break;
        }
        if line.len() as u64 > MAX_LINE {
            oversized += 1;
            while !line.ends_with(b"\n") {
                line.clear();
                if std::io::Read::by_ref(&mut reader)
                    .take(MAX_LINE)
                    .read_until(b'\n', &mut line)
                    .unwrap_or(0)
                    == 0
                {
                    break;
                }
                bytes_read += line.len() as u64;
                fingerprint.update(&line);
                if bytes_read > MAX_FILE {
                    break;
                }
            }
            continue;
        }
        if !line.ends_with(b"\n") {
            incomplete = true;
            break;
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice::<Value>(&line) {
            Ok(v) if v.is_object() => builder.consume(v, line_number),
            _ => malformed += 1,
        }
    }
    let mut warnings = vec![];
    if malformed > 0 {
        warnings.push(format!("有 {malformed} 行无法解析，其余记录仍可阅读。"));
    }
    if oversized > 0 {
        warnings.push(format!(
            "跳过 {oversized} 条超出读取限制的记录，内容并非完整。"
        ));
    }
    if incomplete {
        warnings.push("文件末尾尚未写完，已保留完整记录；刷新后继续读取。".into());
    }
    if builder.foreign_records > 0 {
        warnings.push(format!(
            "跳过 {} 条会话身份不一致的记录，未合并到当前对话。",
            builder.foreign_records
        ));
    }
    if builder.native_id.is_none() {
        return Err(NativeError::new("unsupported", "未识别出有效原生会话身份"));
    }
    #[cfg(unix)]
    let allocated = {
        use std::os::unix::fs::MetadataExt;
        Some(metadata.blocks().saturating_mul(512))
    };
    #[cfg(not(unix))]
    let allocated = None;
    if bytes_read != snapshot_bytes {
        return Err(NativeError::new(
            "stale_snapshot",
            "读取期间会话历史内容已变化，请刷新后重试",
        ));
    }
    if let Some(state) = continuation.as_mut() {
        **state = (!incomplete && malformed == 0 && oversized == 0 && builder.weight() <= 32768)
            .then(|| Continuation {
                builder: builder.clone(),
                metadata: metadata.clone(),
                offset: snapshot_bytes,
                lines: line_number,
                digest: fingerprint.clone(),
                #[cfg(test)]
                decoded: line_number - prior_lines,
            });
    }
    let mut parsed = builder.finish(
        path,
        metadata.len(),
        allocated,
        warnings,
        window.is_some(),
        projects,
    )?;
    parsed.snapshot_bytes = snapshot_bytes;
    parsed.fingerprint = format!("{:x}", fingerprint.finalize());
    Ok(parsed)
}
