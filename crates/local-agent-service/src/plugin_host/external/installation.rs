//! One installation budget across capability hosts. No worker or read path uses this gate.
use super::{Checked, Result, fail};
use rusqlite::Connection;
mod cache;
pub(super) use cache::{check_archives, check_staging};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum HostKind {
    General,
    History,
    Routing,
}
pub(super) struct Entry {
    pub host_kind: HostKind,
    pub id: String,
    pub version: String,
    pub digest: String,
    pub publisher: String,
    pub key: Option<String>,
    pub bytes: u64,
}

pub(super) fn check(db: &Connection, full: &[Entry], candidate: &Entry) -> Result<()> {
    let mut statement = db.prepare(
        "SELECT digest,plugin_id,version,manifest,public_key,bytes FROM packages WHERE removed=0 LIMIT 65",
    ).checked("unavailable")?;
    let legacy = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, i64>(5)?,
            ))
        })
        .checked("unavailable")?
        .map(|row| {
            let (digest, id, version, manifest, key, bytes) = row.checked("invalid_record")?;
            let manifest: local_agent_core::plugin::external::Manifest =
                serde_json::from_str(&manifest).checked("invalid_record")?;
            if manifest.id != id || manifest.version != version || bytes < 0 {
                return Err(fail("invalid_record"));
            }
            Ok(Entry {
                host_kind: HostKind::General,
                id,
                version,
                digest,
                publisher: manifest.publisher,
                key,
                bytes: bytes as u64,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    check_entries(&legacy, full, candidate)
}

fn check_entries(legacy: &[Entry], full: &[Entry], candidate: &Entry) -> Result<()> {
    let entries = || legacy.iter().chain(full);
    let existing = entries().any(|p| p.digest == candidate.digest);
    let mut bytes = if existing { 0 } else { candidate.bytes };
    let mut versions = 0;
    for p in entries() {
        bytes = bytes.checked_add(p.bytes).ok_or(fail("resource_limit"))?;
        if p.id != candidate.id {
            continue;
        }
        if p.host_kind != candidate.host_kind
            && (p.host_kind == HostKind::Routing || candidate.host_kind == HostKind::Routing)
        {
            return Err(fail("capability_conflict"));
        }
        // The legacy format keeps its existing key-rotation behavior. A source identity
        // cannot be taken over through the older installer or a parent namespace.
        if (p.host_kind != HostKind::General || candidate.host_kind != HostKind::General)
            && (p.publisher != candidate.publisher || p.key != candidate.key)
        {
            return Err(fail("publisher_changed"));
        }
        if p.version == candidate.version && p.digest != candidate.digest {
            return Err(fail("capability_conflict"));
        }
        if p.digest != candidate.digest {
            versions += 1;
        }
    }
    if legacy.len() + full.len() + usize::from(!existing) > 64
        || bytes > 256 * 1_048_576
        || versions >= 3
    {
        return Err(fail("resource_limit"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(index: usize, host_kind: HostKind) -> Entry {
        Entry {
            host_kind,
            id: format!("org.example.reader{index}"),
            version: "1.0.0".into(),
            digest: format!("{index:064x}"),
            publisher: "org.example".into(),
            key: Some("fixture-key".into()),
            bytes: 1,
        }
    }
    #[test]
    fn shared_count_bytes_and_versions_include_both_formats_and_allow_exact_reinstall() {
        let mut legacy = (0..32)
            .map(|n| entry(n, HostKind::General))
            .collect::<Vec<_>>();
        let full = (32..64)
            .map(|n| entry(n, HostKind::History))
            .collect::<Vec<_>>();
        assert_eq!(
            check_entries(&legacy, &full, &entry(64, HostKind::History))
                .unwrap_err()
                .code,
            "resource_limit"
        );
        assert_eq!(
            check_entries(&legacy, &full, &entry(64, HostKind::General))
                .unwrap_err()
                .code,
            "resource_limit"
        );
        check_entries(&legacy, &full, &entry(0, HostKind::General)).unwrap();
        check_entries(&legacy, &full, &entry(63, HostKind::History)).unwrap();
        legacy.truncate(1);
        legacy[0].bytes = 256 * 1_048_576 - 1;
        check_entries(&legacy, &[], &entry(1, HostKind::History)).unwrap();
        assert_eq!(
            check_entries(&legacy, &full[..1], &entry(1, HostKind::History))
                .unwrap_err()
                .code,
            "resource_limit"
        );
        legacy[0].bytes = u64::MAX;
        assert_eq!(
            check_entries(&legacy, &[], &entry(1, HostKind::History))
                .unwrap_err()
                .code,
            "resource_limit"
        );
        let mut versions = (0..3)
            .map(|n| {
                let mut p = entry(n, HostKind::General);
                p.id = "org.example.reader".into();
                p.version = format!("1.0.{n}");
                p
            })
            .collect::<Vec<_>>();
        let mut candidate = entry(4, HostKind::History);
        candidate.id = "org.example.reader".into();
        candidate.version = "2.0.0".into();
        assert_eq!(
            check_entries(&versions, &[], &candidate).unwrap_err().code,
            "resource_limit"
        );
        versions.pop();
        check_entries(&versions, &[], &candidate).unwrap();
    }
    #[test]
    fn ownership_and_same_version_conflicts_are_symmetric_across_formats() {
        for (old_format, new_format) in [
            (HostKind::General, HostKind::History),
            (HostKind::History, HostKind::General),
        ] {
            let old = entry(0, old_format);
            let mut candidate = entry(1, new_format);
            candidate.id = old.id.clone();
            assert_eq!(
                check_entries(&[], std::slice::from_ref(&old), &candidate)
                    .unwrap_err()
                    .code,
                "capability_conflict"
            );
            candidate.version = "2.0.0".into();
            check_entries(&[], std::slice::from_ref(&old), &candidate).unwrap();
            candidate.publisher = "org".into();
            assert_eq!(
                check_entries(&[], std::slice::from_ref(&old), &candidate)
                    .unwrap_err()
                    .code,
                "publisher_changed"
            );
            candidate.publisher = old.publisher.clone();
            candidate.key = Some("other-key".into());
            assert_eq!(
                check_entries(&[], std::slice::from_ref(&old), &candidate)
                    .unwrap_err()
                    .code,
                "publisher_changed"
            );
        }
    }
}
