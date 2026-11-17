//! Pure policy execution, authorization and durable publication intents are host-owned.
pub(super) mod api;
mod manage;
pub(super) mod package;
mod recovery;
mod runtime;
mod store;
pub(crate) use store::{Host, Presence};

pub(super) fn committed_archives(
    db: &rusqlite::Connection,
    cache: &std::path::Path,
) -> super::Result<Vec<std::path::PathBuf>> {
    use super::{Checked, fail};
    let mut statement = db
        .prepare("SELECT digest,id,record FROM ext_policy_policy_packages LIMIT 65")
        .checked("unavailable")?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .checked("unavailable")?;
    let mut paths = Vec::new();
    for row in rows {
        let (digest, id, record) = row.checked("invalid_record")?;
        let installed: store::Installed = store::decode(&record)?;
        package::validate(&installed.manifest)?;
        if installed.digest != digest
            || installed.manifest.id != id
            || super::package::unhex::<32>(&digest).is_err()
        {
            return Err(fail("invalid_record"));
        }
        if !installed.removed {
            paths.push(
                cache
                    .join("packages")
                    .join(id)
                    .join(installed.manifest.version)
                    .join(digest)
                    .join("package.laplug"),
            );
        }
    }
    Ok(paths)
}

mod dispatch;
pub(crate) use dispatch::{Choice, ChoiceProof};

#[cfg(test)]
mod tests;
