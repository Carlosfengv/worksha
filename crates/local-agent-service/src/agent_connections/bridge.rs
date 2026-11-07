use super::{ConnectionAccess, Result, fail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt},
    path::Path,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};

const LIMIT: u64 = 65_536;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Selection {
    pub provider: String,
    pub model: String,
    pub reasoning_effort: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Route {
    endpoint: String,
    models: Vec<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Status {
    pub revision: u64,
    pub instance: String,
    pub selected: Option<Selection>,
    pub route: Option<Route>,
}
impl Status {
    pub fn matches(&self, profile: &Profile) -> bool {
        self.route.as_ref().is_some_and(|route| {
            route.endpoint == profile.endpoint
                && route
                    .models
                    .iter()
                    .map(String::as_str)
                    .eq(profile.models.iter().map(|m| m.id.as_str()))
        })
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Model {
    pub id: String,
    pub name: String,
    pub context_window: u32,
    pub max_output_tokens: u32,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Profile {
    pub route: String,
    pub endpoint: String,
    pub name: String,
    pub default_model: String,
    pub models: Vec<Model>,
}
impl Profile {
    pub fn selection(&self) -> Selection {
        Selection {
            provider: self.route.clone(),
            model: self.default_model.clone(),
            reasoning_effort: None,
        }
    }
}

fn key(directory: &Path, uid: u32) -> Result<zeroize::Zeroizing<String>> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(directory.join("agent-connection.key"))
        .map_err(|_| fail("bridge_unavailable"))?;
    let meta = file.metadata().map_err(|_| fail("bridge_unavailable"))?;
    if !meta.is_file() || meta.uid() != uid || meta.nlink() != 1 || meta.mode() & 0o077 != 0 {
        return Err(fail("permission_denied"));
    }
    let mut token = zeroize::Zeroizing::new(String::new());
    file.take(65)
        .read_to_string(&mut token)
        .map_err(|_| fail("bridge_unavailable"))?;
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(fail("invalid_record"));
    }
    Ok(token)
}

async fn call(access: &ConnectionAccess, request: Value, mutating: bool) -> Result<Status> {
    // The socket and credential are siblings of the exact authorized source.
    let uid = unsafe { libc::geteuid() };
    let dir = fs::symlink_metadata(&access.directory).map_err(|_| fail("bridge_unavailable"))?;
    if !dir.is_dir() || dir.uid() != uid || dir.mode() & 0o077 != 0 {
        return Err(fail("permission_denied"));
    }
    let path = access.directory.join("agent-connection.sock");
    let socket = fs::symlink_metadata(&path).map_err(|_| fail("bridge_unavailable"))?;
    if !socket.file_type().is_socket() || socket.uid() != uid || socket.mode() & 0o077 != 0 {
        return Err(fail("permission_denied"));
    }
    let key = key(&access.directory, uid)?;
    let mut input = zeroize::Zeroizing::new(
        serde_json::to_vec(&json!({
            "version": 1, "token": &*key, "request": request
        }))
        .map_err(|_| fail("invalid_input"))?,
    );
    if input.len() as u64 >= LIMIT {
        return Err(fail("resource_limit"));
    }
    input.push(b'\n');
    let exchange = async {
        let mut socket = UnixStream::connect(path)
            .await
            .map_err(|_| fail("bridge_unavailable"))?;
        if socket
            .peer_cred()
            .map_err(|_| fail("permission_denied"))?
            .uid()
            != uid
        {
            return Err(fail("permission_denied"));
        }
        let interrupted = || {
            fail(if mutating {
                "connection_result_unknown"
            } else {
                "bridge_unavailable"
            })
        };
        socket.write_all(&input).await.map_err(|_| interrupted())?;
        let mut reply = Vec::new();
        socket
            .take(LIMIT + 1)
            .read_to_end(&mut reply)
            .await
            .map_err(|_| interrupted())?;
        if reply.len() as u64 > LIMIT {
            return Err(interrupted());
        }
        let result: Value = serde_json::from_slice(&reply).map_err(|_| interrupted())?;
        if result["version"] != 1 {
            return Err(interrupted());
        }
        if let Some(error) = result["error"].as_str() {
            return Err(fail(match error {
                "conflict" => "stale_snapshot",
                "denied" => "permission_denied",
                "invalid" => "invalid_input",
                "busy" => "busy",
                _ if mutating => "connection_result_unknown",
                _ => "bridge_unavailable",
            }));
        }
        serde_json::from_value(result["status"].clone()).map_err(|_| interrupted())
    };
    tokio::time::timeout(Duration::from_secs(12), exchange)
        .await
        .map_err(|_| {
            fail(if mutating {
                "connection_result_unknown"
            } else {
                "bridge_unavailable"
            })
        })?
}

pub(super) async fn read(access: &ConnectionAccess, route: Option<&str>) -> Result<Status> {
    call(access, json!({"kind":"read","route":route}), false).await
}

pub(super) async fn apply(
    access: &ConnectionAccess,
    before: &Status,
    profile: &Profile,
    token: &str,
) -> Result<Status> {
    call(
        access,
        json!({"kind":"apply","expectedRevision":before.revision,"expectedInstance":before.instance,
        "before":before.selected,"profile":profile,"gatewayToken":token}),
        true,
    )
    .await
}

pub(super) async fn restore(
    access: &ConnectionAccess,
    profile: &Profile,
    previous: &Selection,
) -> Result<Status> {
    call(
        access,
        json!({"kind":"restore","profile":profile,"previous":previous}),
        true,
    )
    .await
}
