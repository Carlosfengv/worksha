use super::{ProjectIdentity, canonical, command, hash};
use crate::sessions::{now, sanitize_label};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Listener {
    pub id: String,
    pub protocol: String,
    pub address: String,
    pub port: u16,
    pub pid: u32,
    pub process_identity: String,
    pub started_at: String,
    pub command_redacted: String,
    pub cwd: String,
    pub attribution: String,
    pub evidence: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListenerSnapshot {
    pub as_of: Option<String>,
    pub state: String,
    pub items: Vec<Listener>,
    pub unresolved_processes: usize,
    pub error: Option<String>,
}
impl Default for ListenerSnapshot {
    fn default() -> Self {
        Self {
            as_of: None,
            state: "not_collected".into(),
            items: vec![],
            unresolved_processes: 0,
            error: None,
        }
    }
}
impl ListenerSnapshot {
    pub fn count(&self) -> Option<usize> {
        (self.state == "ready" && self.unresolved_processes == 0).then_some(self.items.len())
    }
    pub fn for_project(&self, project: &ProjectIdentity, all: &[ProjectIdentity]) -> Self {
        let mut result = self.clone();
        result.items.retain(|listener| {
            all.iter()
                .filter(|p| Path::new(&listener.cwd).starts_with(&p.path))
                .max_by_key(|p| p.path.len())
                .is_some_and(|p| p.id == project.id)
        });
        result
    }
}
#[cfg(target_os = "macos")]
fn process(pid: u32) -> Option<(String, String, String)> {
    // Darwin reports microsecond process start identity and cwd without reading argv/env secrets.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let mut paths: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
    let info_len = std::mem::size_of_val(&info) as i32;
    let path_len = std::mem::size_of_val(&paths) as i32;
    if unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            info_len,
        )
    } != info_len
    {
        return None;
    }
    if info.pbi_uid != unsafe { libc::geteuid() } {
        return None;
    }
    if unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDVNODEPATHINFO,
            0,
            &mut paths as *mut _ as *mut libc::c_void,
            path_len,
        )
    } != path_len
    {
        return None;
    }
    let cwd_bytes: Vec<u8> = paths
        .pvi_cdir
        .vip_path
        .iter()
        .flatten()
        .map(|c| *c as u8)
        .take_while(|b| *b != 0)
        .collect();
    let cwd = canonical(Path::new(std::str::from_utf8(&cwd_bytes).ok()?))?
        .to_string_lossy()
        .into_owned();
    let started = DateTime::<Utc>::from_timestamp(
        info.pbi_start_tvsec as i64,
        (info.pbi_start_tvusec * 1000) as u32,
    )?
    .to_rfc3339();
    let comm = info
        .pbi_name
        .iter()
        .map(|b| *b as u8)
        .take_while(|b| *b != 0)
        .collect::<Vec<_>>();
    Some((
        started,
        cwd,
        sanitize_label(&String::from_utf8_lossy(&comm), 200),
    ))
}
#[cfg(not(target_os = "macos"))]
fn process(_pid: u32) -> Option<(String, String, String)> {
    None
}

pub fn collect() -> ListenerSnapshot {
    let as_of = now();
    #[cfg(target_os = "macos")]
    let result = command::run(
        "/usr/sbin/lsof",
        &[
            "-nP",
            "-a",
            "-u",
            &unsafe { libc::geteuid() }.to_string(),
            "-iTCP",
            "-sTCP:LISTEN",
            "-F0pn",
        ],
        None,
    );
    #[cfg(not(target_os = "macos"))]
    let result: Result<Vec<u8>, String> = Err("当前端口采集仅支持 macOS".into());
    let bytes = match result {
        Ok(bytes) => bytes,
        Err(error) => {
            return ListenerSnapshot {
                as_of: Some(as_of),
                state: "unavailable".into(),
                error: Some(error),
                ..Default::default()
            };
        }
    };
    let mut pid = None;
    let mut raw = Vec::new();
    for field in bytes.split(|b| *b == 0) {
        let field = field.strip_prefix(b"\n").unwrap_or(field);
        match field.first() {
            Some(b'p') => {
                pid = std::str::from_utf8(&field[1..])
                    .ok()
                    .and_then(|s| s.parse::<u32>().ok())
            }
            Some(b'n') => {
                if let Some(pid) = pid
                    && let Ok(name) = std::str::from_utf8(&field[1..])
                {
                    raw.push((pid, name.to_string()));
                }
            }
            _ => {}
        }
    }
    let mut processes = HashMap::new();
    let mut unresolved = HashSet::new();
    let mut items = Vec::new();
    let mut seen = HashSet::new();
    let truncated = raw.len() > 4096;
    for (pid, address) in raw.into_iter().take(4096) {
        let details = processes.entry(pid).or_insert_with(|| process(pid));
        let Some((started, cwd, comm)) = details.as_ref() else {
            unresolved.insert(pid);
            continue;
        };
        // Reject processes created after enumeration and PID reuse during collection.
        if started > &as_of || process(pid).as_ref().map(|p| &p.0) != Some(started) {
            unresolved.insert(pid);
            continue;
        }
        let Some((host, port)) = address.rsplit_once(':') else {
            continue;
        };
        let Ok(port) = port.trim().parse::<u16>() else {
            continue;
        };
        let process_identity = format!("local:{pid}:{started}");
        let id = hash(&format!("{process_identity}:tcp:{host}:{port}"));
        if !seen.insert(id.clone()) {
            continue;
        }
        items.push(Listener {
            id,
            protocol: "TCP".into(),
            address: host.into(),
            port,
            pid,
            process_identity,
            started_at: started.clone(),
            command_redacted: comm.clone(),
            cwd: cwd.clone(),
            attribution: "project_cwd".into(),
            evidence: "进程 cwd 匹配项目；未采集 argv / 环境变量，无 Agent / Session 启动证据"
                .into(),
        });
    }
    items.sort_by(|a, b| {
        a.port
            .cmp(&b.port)
            .then(a.address.cmp(&b.address))
            .then(a.pid.cmp(&b.pid))
    });
    ListenerSnapshot {
        as_of: Some(as_of),
        state: if unresolved.is_empty() && !truncated {
            "ready"
        } else {
            "partial"
        }
        .into(),
        items,
        unresolved_processes: unresolved.len(),
        error: if truncated {
            Some("监听项超过 4096 条，当前快照仅为部分覆盖".into())
        } else {
            (!unresolved.is_empty())
                .then(|| "部分进程 cwd 或身份不可读取，已知监听项仅为部分覆盖".into())
        },
    }
}
