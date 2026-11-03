//! Recover only the optional host's owned cache. Never restore a runtime or source grant.
use super::super::{Checked, Result, fail, package as legacy};
use super::store::{self, Host, Installed, Selection};
use local_agent_core::plugin::full as f;
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

fn entries(path: &Path, budget: &mut usize) -> Result<Vec<std::fs::DirEntry>> {
    let mut result = vec![];
    for entry in std::fs::read_dir(path).checked("unavailable")? {
        *budget = budget.checked_sub(1).ok_or(fail("resource_limit"))?;
        result.push(entry.checked("unavailable")?);
    }
    Ok(result)
}
fn erase_owned_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    let meta = match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        result => result.checked("unavailable")?,
    };
    if !meta.is_file() || meta.file_type().is_symlink() || meta.nlink() != 1 {
        return Err(fail("permission_denied"));
    }
    std::fs::remove_file(path).checked("unavailable")?;
    std::fs::File::open(path.parent().ok_or(fail("permission_denied"))?)
        .checked("unavailable")?
        .sync_all()
        .checked("unavailable")
}
fn temporary(name: &str) -> bool {
    name.strip_prefix('.')
        .and_then(|n| n.strip_suffix(".tmp"))
        .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
}
fn prune_empty(path: &Path) -> Result<()> {
    match std::fs::remove_dir(path) {
        Ok(()) => Ok(()),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
            ) =>
        {
            Ok(())
        }
        Err(_) => Err(fail("unavailable")),
    }
}

pub(super) fn reconcile(host: &Host, db: &Connection) -> Result<()> {
    let mut statement = db.prepare("SELECT plugin_id,digest,CASE WHEN length(record)<=65536 THEN record ELSE NULL END FROM full_packages WHERE removed=0 LIMIT 65").checked("unavailable")?;
    let packages = statement
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })
        .checked("unavailable")?
        .map(|row| {
            let (id, digest, text) = row.checked("invalid_record")?;
            let p: Installed = store::decode(&text.ok_or(fail("invalid_record"))?)?;
            store::validate_installed(
                &f::PackageQuery {
                    plugin_id: id,
                    digest,
                },
                &p,
            )?;
            Ok(p)
        })
        .collect::<Result<Vec<_>>>()?;
    if packages.len() > 64 {
        return Err(fail("resource_limit"));
    }
    let committed = packages
        .iter()
        .map(|p| host.package_path(p))
        .collect::<BTreeSet<_>>();
    let mut ids = packages
        .iter()
        .map(|p| p.manifest.id.clone())
        .collect::<BTreeSet<_>>();
    let mut budget = 4096;
    let marker_entries = entries(&host.root.join("selected"), &mut budget)?;
    for entry in &marker_entries {
        let name = entry.file_name().to_string_lossy().into_owned();
        if legacy::valid_id(&name) {
            ids.insert(name);
        }
    }
    // Validate authoritative records before deleting any cache artifact.
    let mut desired = BTreeMap::new();
    for id in ids {
        let text: Option<String> = db
            .query_row(
                "SELECT record FROM full_selections WHERE plugin_id=?1",
                [&id],
                |r| r.get(0),
            )
            .optional()
            .checked("unavailable")?;
        let selection = text
            .as_deref()
            .map(store::decode::<Selection>)
            .transpose()?;
        if let Some(s) = &selection {
            s.validate()?;
        }
        if selection.as_ref().is_some_and(|s| {
            s.enabled
                && !packages
                    .iter()
                    .any(|p| p.manifest.id == id && p.digest == s.digest)
        }) {
            return Err(fail("invalid_record"));
        }
        desired.insert(id, selection.filter(|s| s.enabled).map(|s| s.digest));
    }
    // References from a previous process cannot authorize an install. Original input
    // packages are outside staging and are never touched by recovery.
    db.execute("DELETE FROM full_reviews", [])
        .checked("unavailable")?;
    for entry in entries(&host.root.join("staging"), &mut budget)? {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name
            .strip_suffix(".laplug")
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
            || temporary(&name)
        {
            erase_owned_file(&entry.path())?;
        }
    }
    for author in entries(&host.root.join("packages"), &mut budget)? {
        let id = author.file_name().to_string_lossy().into_owned();
        if !legacy::valid_id(&id) || !author.file_type().checked("unavailable")?.is_dir() {
            continue;
        }
        for version in entries(&author.path(), &mut budget)? {
            if semver::Version::parse(&version.file_name().to_string_lossy()).is_err()
                || !version.file_type().checked("unavailable")?.is_dir()
            {
                continue;
            }
            for digest in entries(&version.path(), &mut budget)? {
                if legacy::unhex::<32>(&digest.file_name().to_string_lossy()).is_err()
                    || !digest.file_type().checked("unavailable")?.is_dir()
                {
                    continue;
                }
                for file in entries(&digest.path(), &mut budget)? {
                    let name = file.file_name().to_string_lossy().into_owned();
                    if temporary(&name)
                        || (name == "package.laplug" && !committed.contains(&file.path()))
                    {
                        erase_owned_file(&file.path())?;
                    }
                }
                prune_empty(&digest.path())?;
            }
            prune_empty(&version.path())?;
        }
        prune_empty(&author.path())?;
    }
    for entry in marker_entries {
        if temporary(&entry.file_name().to_string_lossy()) {
            erase_owned_file(&entry.path())?;
        }
    }
    for (id, digest) in desired {
        host.selected_marker(&id, digest.as_deref())?;
    }
    Ok(())
}

impl Host {
    pub(super) fn publish_package(&self, p: &Installed, bytes: &[u8]) -> Result<()> {
        store::validate_installed(
            &f::PackageQuery {
                plugin_id: p.manifest.id.clone(),
                digest: p.digest.clone(),
            },
            p,
        )?;
        if p.compressed_bytes != bytes.len() as u64 || legacy::hash(bytes) != p.digest {
            return Err(fail("signature_invalid"));
        }
        let db = self.db()?;
        let target = self.package_path(p);
        let folder = target.parent().ok_or(fail("invalid_input"))?;
        for path in [
            folder.parent().and_then(Path::parent),
            folder.parent(),
            Some(folder),
        ] {
            store::directory(path.ok_or(fail("invalid_input"))?)?;
        }
        let written = !target.exists();
        if written {
            if let Err(error) = self.write_private(&target, bytes) {
                self.remove_archive(p)?;
                return Err(error);
            }
        } else if legacy::hash(&legacy::read_file(&target, legacy::MAX_ARCHIVE)?) != p.digest {
            return Err(fail("signature_invalid"));
        }
        let result = (|| {
            let tx = db.unchecked_transaction().checked("unavailable")?;
            tx.execute("INSERT INTO full_packages VALUES(?1,?2,?3,0) ON CONFLICT(digest) DO UPDATE SET removed=0",params![p.digest,p.manifest.id,store::encode(p)?]).checked("unavailable")?;
            tx.execute(
                "INSERT OR IGNORE INTO full_selections VALUES(?1,?2)",
                params![
                    p.manifest.id,
                    store::encode(&Selection::installed(&p.digest))?
                ],
            )
            .checked("unavailable")?;
            tx.commit().checked("unavailable")
        })();
        if result.is_err() && written {
            self.remove_archive(p)?;
        }
        result
    }
    pub(super) fn remove_archive(&self, p: &Installed) -> Result<()> {
        let target = self.package_path(p);
        erase_owned_file(&target)?;
        let mut folder = target.parent().ok_or(fail("invalid_input"))?;
        for _ in 0..3 {
            prune_empty(folder)?;
            folder = folder.parent().ok_or(fail("invalid_input"))?;
        }
        Ok(())
    }
}
