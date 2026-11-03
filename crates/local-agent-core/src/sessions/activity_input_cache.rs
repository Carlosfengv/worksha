//! Private, bounded message summaries. Complete bodies and tool data never enter this cache.
use super::activity_input::UserInput;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Read, path::PathBuf};

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CachedInput {
    pub key: String,
    pub agent: String,
    pub session_id: Option<String>,
    pub started_at: Option<String>,
    #[serde(default)]
    pub source_stamp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<CachedNative>,
    pub input: UserInput,
}
#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CachedNative {
    #[serde(default)]
    pub latest: bool,
    pub message_id: Option<String>,
    pub event_at: String,
    pub phase: String,
    pub model: Option<String>,
    pub ttft_ms: Option<f64>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    version: u8,
    rows: Vec<CachedInput>,
}
#[derive(Default)]
pub struct InputCache {
    rows: BTreeMap<String, CachedInput>,
    path: Option<PathBuf>,
    dirty: bool,
    pub error: Option<String>,
}
impl InputCache {
    pub fn open(path: PathBuf) -> Self {
        let mut cache = Self {
            path: Some(path.clone()),
            ..Default::default()
        };
        let read = || -> Result<Option<Saved>, ()> {
            if std::fs::symlink_metadata(&path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            {
                return Ok(None);
            }
            crate::providers::files::checked_path(&path).map_err(|_| ())?;
            let mut options = std::fs::OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
            }
            let file = match options.open(&path) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(_) => return Err(()),
            };
            let meta = file.metadata().map_err(|_| ())?;
            if !meta.is_file() || meta.len() > 2 * 1024 * 1024 {
                return Err(());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                if meta.nlink() != 1
                    || meta.mode() & 0o077 != 0
                    || meta.uid() != unsafe { libc::geteuid() }
                {
                    return Err(());
                }
            }
            let mut bytes = vec![];
            file.take(2 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)
                .map_err(|_| ())?;
            if bytes.len() > 2 * 1024 * 1024 {
                return Err(());
            }
            let saved: Saved = serde_json::from_slice(&bytes).map_err(|_| ())?;
            if saved.version != 1 || saved.rows.len() > 512 {
                return Err(());
            }
            Ok(Some(saved))
        };
        match read() {
            Ok(Some(saved)) => {
                for row in saved.rows {
                    cache.insert(row);
                }
                cache.dirty = false;
            }
            Ok(None) => {}
            Err(()) => {
                cache.error = Some("活动消息摘要暂不可恢复，正在从已授权来源重新读取".into());
                cache.dirty = true;
            }
        }
        cache
    }
    pub fn get(&self, key: &str) -> Option<&CachedInput> {
        self.rows.get(key)
    }
    pub fn values(&self) -> impl Iterator<Item = &CachedInput> {
        self.rows.values()
    }
    pub fn insert(&mut self, mut row: CachedInput) {
        if row.key.len() > 2048
            || row.agent.len() > 250
            || row.session_id.as_ref().is_some_and(|s| s.len() > 1024)
            || chrono::DateTime::parse_from_rfc3339(&row.input.at).is_err()
            || row
                .started_at
                .as_ref()
                .is_some_and(|at| chrono::DateTime::parse_from_rfc3339(at).is_err())
        {
            return;
        }
        let bounded = row.input.text.chars().take(16_384).collect::<String>();
        let (text, truncated) = super::parser::safe_text(&bounded, 240);
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            return;
        }
        row.input.text = format!("{text}{}", if truncated { "…" } else { "" });
        if row
            .source_stamp
            .as_ref()
            .is_some_and(|stamp| stamp.len() > 256)
        {
            return;
        }
        if let Some(native) = &mut row.native {
            if chrono::DateTime::parse_from_rfc3339(&native.event_at).is_err()
                || native.message_id.as_ref().is_some_and(|id| {
                    id.is_empty()
                        || id.len() > 512
                        || !id
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b":._-+".contains(&b))
                })
            {
                return;
            }
            native.model = native
                .model
                .as_ref()
                .map(|model| super::sanitize_label(model, 250));
            native.ttft_ms = native.ttft_ms.filter(|n| n.is_finite() && *n >= 0.0);
            if !["waiting_input", "cancelled", "error"].contains(&native.phase.as_str()) {
                native.phase = "processing".into();
            }
        }
        if !serde_json::to_vec(&row).is_ok_and(|bytes| bytes.len() <= 3072) {
            return;
        }
        if self.rows.get(&row.key) == Some(&row) {
            return;
        }
        self.rows.insert(row.key.clone(), row);
        while self.rows.len() > 512 {
            let oldest = self
                .rows
                .values()
                .min_by_key(|row| {
                    (
                        chrono::DateTime::parse_from_rfc3339(&row.input.at).ok(),
                        &row.key,
                    )
                })
                .unwrap()
                .key
                .clone();
            self.rows.remove(&oldest);
        }
        self.dirty = true;
    }
    pub fn retain(&mut self, keep: impl Fn(&CachedInput) -> bool) {
        let before = self.rows.len();
        self.rows.retain(|_, row| keep(row));
        self.dirty |= before != self.rows.len();
    }
    pub fn commit(&mut self) -> Result<(), String> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        if !self.dirty {
            return Ok(());
        }
        let bytes = serde_json::to_vec(&Saved {
            version: 1,
            rows: self.rows.values().cloned().collect(),
        })
        .map_err(|_| "活动消息摘要无法保存".to_owned())?;
        crate::providers::files::atomic_write(path, &bytes, 0o600)
            .map_err(|_| "活动消息摘要未保存，请检查本地存储".to_owned())?;
        self.dirty = false;
        self.error = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input(key: &str, text: &str, at: String) -> CachedInput {
        CachedInput {
            key: key.into(),
            agent: "claude".into(),
            session_id: Some("claude:s".into()),
            started_at: Some(at.clone()),
            source_stamp: None,
            native: None,
            input: UserInput {
                text: text.into(),
                at,
            },
        }
    }
    #[test]
    fn summaries_are_private_redacted_bounded_and_recover_after_a_corrupt_cache() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().canonicalize().unwrap().join("inputs.json");
        std::fs::write(&path, b"{broken").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let mut cache = InputCache::open(path.clone());
        assert!(cache.error.is_some());
        cache.insert(input(
            "r",
            &("hello Bearer abcdefghijklmnop ".to_owned() + &"你好🙂".repeat(500)),
            chrono::Utc::now().to_rfc3339(),
        ));
        cache.commit().unwrap();
        assert!(cache.error.is_none());
        let restored = InputCache::open(path.clone());
        let row = restored.get("r").unwrap();
        assert!(row.input.text.contains("[已隐藏凭据]"));
        assert!(row.input.text.chars().count() <= 241);
        assert!(
            !std::fs::read_to_string(&path)
                .unwrap()
                .contains("abcdefghijklmnop")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn cache_capacity_is_bounded_and_evicts_oldest_inputs() {
        let mut cache = InputCache::default();
        let base = chrono::Utc::now() - chrono::Duration::seconds(1000);
        for i in 0..600 {
            cache.insert(input(
                &format!("r-{i}"),
                "same input",
                (base + chrono::Duration::seconds(i)).to_rfc3339(),
            ));
        }
        assert_eq!(cache.values().count(), 512);
        assert!(cache.get("r-0").is_none());
        assert!(cache.get("r-599").is_some());
    }
    #[cfg(unix)]
    #[test]
    fn linked_cache_files_are_rejected_without_exposing_or_overwriting_targets() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let target = root.join("target");
        let path = root.join("inputs.json");
        std::fs::write(&target, b"sensitive-target").unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let mut cache = InputCache::open(path);
        assert!(cache.error.is_some());
        cache.insert(input("r", "safe", chrono::Utc::now().to_rfc3339()));
        assert!(cache.commit().is_err());
        assert_eq!(std::fs::read_to_string(target).unwrap(), "sensitive-target");
    }
}
