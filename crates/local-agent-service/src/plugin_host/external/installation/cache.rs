//! Read-only admission for actual owned archive bytes, including failed cleanup.
//! Logical installed-package limits are separate. Never open archive contents here.
use super::{HostKind, Result, fail};
use crate::plugin_host::external::{Checked, package};
use std::{
    fs::{self, File, OpenOptions},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

const ARCHIVES: u64 = 256 * 1_048_576;
const FULL_STAGING: u64 = 128 * 1_048_576;
const WALK_LIMIT: usize = 4096;

#[derive(Clone, Copy)]
enum Level {
    ArchiveRoot,
    StagingRoot(HostKind),
    Packages,
    Versions,
    Digests,
    Files,
    LegacyStaging,
    FullStaging,
}
struct Scan {
    remaining: usize,
    bytes: u64,
    limit: u64,
    files: usize,
    file_limit: usize,
    target: Option<(u64, u64)>,
    present: bool,
}
fn temporary(name: &str) -> bool {
    name.strip_prefix('.')
        .and_then(|n| n.strip_suffix(".tmp"))
        .is_some_and(|n| uuid::Uuid::parse_str(n).is_ok())
}
fn directory(path: &Path) -> Result<Option<File>> {
    match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => Ok(Some(file)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(fail("permission_denied")),
    }
}
impl Scan {
    fn child(&mut self, entry: &fs::DirEntry, level: Level) -> Result<()> {
        let kind = entry.file_type().checked("unavailable")?;
        if kind.is_symlink() {
            return Err(fail("permission_denied"));
        }
        if kind.is_dir() {
            self.walk(&entry.path(), level)?;
        }
        Ok(())
    }
    fn add(&mut self, bytes: u64) -> Result<()> {
        self.files = self
            .files
            .checked_add(1)
            .filter(|n| *n <= self.file_limit)
            .ok_or(fail("resource_limit"))?;
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .filter(|n| *n <= self.limit)
            .ok_or(fail("resource_limit"))?;
        Ok(())
    }
    fn file(&mut self, path: &Path) -> Result<()> {
        let m = fs::symlink_metadata(path).checked("unavailable")?;
        if !m.is_file() || m.file_type().is_symlink() || m.nlink() != 1 {
            return Err(fail("permission_denied"));
        }
        self.add(m.len())?;
        self.present |= self.target == Some((m.dev(), m.ino()));
        Ok(())
    }
    fn walk(&mut self, path: &Path, level: Level) -> Result<()> {
        let Some(handle) = directory(path)? else {
            return Ok(());
        };
        let before = handle.metadata().checked("unavailable")?;
        for entry in fs::read_dir(path).checked("unavailable")? {
            self.remaining = self
                .remaining
                .checked_sub(1)
                .ok_or(fail("resource_limit"))?;
            let entry = entry.checked("unavailable")?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // macOS can resolve case aliases. Count them, and use file identity
            // rather than folded path text to recognize an existing target.
            let name = name.to_ascii_lowercase();
            let next = entry.path();
            match level {
                Level::ArchiveRoot if name == "packages" => self.walk(&next, Level::Packages)?,
                Level::StagingRoot(format) if name == "staging" => self.walk(
                    &next,
                    if format == HostKind::History {
                        Level::FullStaging
                    } else {
                        Level::LegacyStaging
                    },
                )?,
                Level::Packages if package::valid_id(&name) => {
                    self.child(&entry, Level::Versions)?
                }
                Level::Versions if semver::Version::parse(&name).is_ok() => {
                    self.child(&entry, Level::Digests)?
                }
                Level::Digests if package::unhex::<32>(&name).is_ok() => {
                    self.child(&entry, Level::Files)?
                }
                Level::LegacyStaging if uuid::Uuid::parse_str(&name).is_ok() => {
                    self.child(&entry, Level::Files)?
                }
                Level::FullStaging
                    if name
                        .strip_suffix(".laplug")
                        .is_some_and(|n| uuid::Uuid::parse_str(n).is_ok()) =>
                {
                    self.file(&next)?
                }
                Level::Files if name == "package.laplug" || temporary(&name) => self.file(&next)?,
                Level::LegacyStaging | Level::FullStaging if temporary(&name) => {
                    self.file(&next)?
                }
                _ => {} // Unknown user files are neither consumed nor erased.
            }
        }
        let after = fs::symlink_metadata(path).checked("unavailable")?;
        if after.file_type().is_symlink()
            || before.dev() != after.dev()
            || before.ino() != after.ino()
        {
            return Err(fail("permission_denied"));
        }
        // Retrying admission is safe; accounting cannot accept a changed directory.
        if before.ctime() != after.ctime() || before.ctime_nsec() != after.ctime_nsec() {
            return Err(fail("busy"));
        }
        Ok(())
    }
}

enum Addition<'a> {
    Archive(&'a Path, u64),
    Staging(HostKind, u64),
}
fn check(roots: [&Path; 2], addition: Addition<'_>) -> Result<()> {
    let target = match addition {
        Addition::Archive(path, _) => match fs::symlink_metadata(path) {
            Ok(m) if m.is_file() && !m.file_type().is_symlink() && m.nlink() == 1 => {
                Some((m.dev(), m.ino()))
            }
            Ok(_) => return Err(fail("permission_denied")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err(fail("permission_denied")),
        },
        _ => None,
    };
    let mut archives = Scan {
        remaining: WALK_LIMIT,
        bytes: 0,
        limit: ARCHIVES,
        files: 0,
        file_limit: 64,
        target,
        present: false,
    };
    for root in roots {
        archives.walk(root, Level::ArchiveRoot)?;
    }
    if let Addition::Archive(_, bytes) = addition
        && !archives.present
    {
        archives.add(bytes)?;
    }
    let mut remaining = archives.remaining;
    for (root, format) in roots
        .into_iter()
        .zip([HostKind::General, HostKind::History])
    {
        let mut stage = Scan {
            remaining,
            bytes: 0,
            limit: if format == HostKind::History {
                FULL_STAGING
            } else {
                ARCHIVES
            },
            files: 0,
            file_limit: if format == HostKind::History { 64 } else { 8 },
            target: None,
            present: false,
        };
        stage.walk(root, Level::StagingRoot(format))?;
        if let Addition::Staging(selected, bytes) = addition
            && selected == format
        {
            stage.add(bytes)?;
        }
        remaining = stage.remaining;
    }
    Ok(())
}
/// Roots are the private legacy and source cache roots, in that order.
pub(in crate::plugin_host::external) fn check_archives(
    roots: [&Path; 2],
    target: &Path,
    bytes: u64,
) -> Result<()> {
    check(roots, Addition::Archive(target, bytes))
}
pub(in crate::plugin_host::external) fn check_staging(
    roots: [&Path; 2],
    host_kind: HostKind,
    bytes: u64,
) -> Result<()> {
    check(roots, Addition::Staging(host_kind, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn roots(base: &Path) -> [std::path::PathBuf; 2] {
        let roots = [base.join("legacy"), base.join("full")];
        for root in &roots {
            fs::create_dir_all(root.join("packages")).unwrap();
            fs::create_dir(root.join("staging")).unwrap();
        }
        roots
    }
    fn archive(root: &Path, n: u64, size: u64) -> std::path::PathBuf {
        let path = root
            .join("packages/org.example.cache/1.0.0")
            .join(format!("{n:064x}"))
            .join("package.laplug");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        File::create(&path).unwrap().set_len(size).unwrap();
        path
    }
    fn stage(root: &Path, host_kind: HostKind, size: u64) -> std::path::PathBuf {
        let id = uuid::Uuid::new_v4();
        let path = if host_kind == HostKind::History {
            root.join("staging").join(format!("{id}.laplug"))
        } else {
            let dir = root.join("staging").join(id.to_string());
            fs::create_dir(&dir).unwrap();
            dir.join("package.laplug")
        };
        File::create(&path).unwrap().set_len(size).unwrap();
        path
    }
    #[test]
    fn physical_archives_keep_shared_byte_count_and_exact_reinstall_boundaries() {
        let data = tempfile::tempdir().unwrap();
        let roots = roots(data.path());
        let old = archive(&roots[0], 0, ARCHIVES - 10);
        let new = roots[1]
            .join("packages/org.example.next/1.0.0")
            .join("f".repeat(64))
            .join("package.laplug");
        check_archives([&roots[0], &roots[1]], &new, 10).unwrap();
        assert_eq!(
            check_archives([&roots[0], &roots[1]], &new, 11)
                .unwrap_err()
                .code,
            "resource_limit"
        );
        check_archives([&roots[0], &roots[1]], &old, ARCHIVES - 10).unwrap();
        fs::remove_file(old).unwrap();
        for n in 0..64 {
            archive(&roots[n as usize % 2], n, 1);
        }
        assert_eq!(
            check_archives([&roots[0], &roots[1]], &new, 1)
                .unwrap_err()
                .code,
            "resource_limit"
        );
        let existing = roots[0]
            .join("packages/org.example.cache/1.0.0")
            .join(format!("{:064x}", 0))
            .join("package.laplug");
        check_archives([&roots[0], &roots[1]], &existing, 1).unwrap();
    }
    #[test]
    fn staging_residue_and_package_temporaries_consume_existing_allowances() {
        for format in [HostKind::General, HostKind::History] {
            let data = tempfile::tempdir().unwrap();
            let roots = roots(data.path());
            let index = usize::from(format == HostKind::History);
            let limit = if format == HostKind::History {
                FULL_STAGING
            } else {
                ARCHIVES
            };
            let path = stage(&roots[index], format, limit - 10);
            check_staging([&roots[0], &roots[1]], format, 10).unwrap();
            assert_eq!(
                check_staging([&roots[0], &roots[1]], format, 11)
                    .unwrap_err()
                    .code,
                "resource_limit"
            );
            File::create(&path).unwrap().set_len(limit + 1).unwrap();
            let target = roots[0].join("packages/new-target");
            assert_eq!(
                check_archives([&roots[0], &roots[1]], &target, 1)
                    .unwrap_err()
                    .code,
                "resource_limit"
            );
            fs::remove_file(path).unwrap();
            let count = if format == HostKind::History { 64 } else { 8 };
            for _ in 0..count {
                stage(&roots[index], format, 1);
            }
            assert_eq!(
                check_staging([&roots[0], &roots[1]], format, 1)
                    .unwrap_err()
                    .code,
                "resource_limit"
            );
        }
        let data = tempfile::tempdir().unwrap();
        let roots = roots(data.path());
        let original = archive(&roots[0], 0, 0);
        fs::remove_file(&original).unwrap();
        let temporary = original
            .parent()
            .unwrap()
            .join(format!(".{}.tmp", uuid::Uuid::new_v4()));
        File::create(&temporary).unwrap().set_len(ARCHIVES).unwrap();
        assert_eq!(
            check_archives([&roots[0], &roots[1]], &original, 1)
                .unwrap_err()
                .code,
            "resource_limit"
        );
        // Package bytes are exactly at their cap, so staging may grow within its
        // separate allowance. Only an additional package must be rejected.
        check_staging([&roots[0], &roots[1]], HostKind::History, 1).unwrap();
    }
    #[test]
    fn archive_walk_rejects_aliases_without_consuming_or_erasing_source_contents() {
        let data = tempfile::tempdir().unwrap();
        let roots = roots(data.path());
        let source = data.path().join("source.jsonl");
        fs::write(&source, b"private source sentinel").unwrap();
        let path = archive(&roots[0], 0, 1);
        fs::remove_file(&path).unwrap();
        symlink(&source, &path).unwrap();
        assert_eq!(
            check_archives([&roots[0], &roots[1]], &path, 1)
                .unwrap_err()
                .code,
            "permission_denied"
        );
        fs::remove_file(&path).unwrap();
        fs::hard_link(&source, &path).unwrap();
        assert_eq!(
            check_staging([&roots[0], &roots[1]], HostKind::History, 1)
                .unwrap_err()
                .code,
            "permission_denied"
        );
        fs::remove_file(&path).unwrap();
        let alias = data.path().join("alias");
        symlink(&roots[0], &alias).unwrap();
        assert_eq!(
            check_archives([&alias, &roots[1]], &path, 1)
                .unwrap_err()
                .code,
            "permission_denied"
        );
        let note = path.parent().unwrap().join("keep-user-note.txt");
        fs::write(&note, "preserve").unwrap();
        let root_note = roots[0].join("packages/org.example.notes");
        fs::write(&root_note, "preserve root note").unwrap();
        check_archives([&roots[0], &roots[1]], &path, 1).unwrap();
        assert_eq!(
            fs::read_to_string(source).unwrap(),
            "private source sentinel"
        );
        assert_eq!(fs::read_to_string(note).unwrap(), "preserve");
        assert_eq!(fs::read_to_string(root_note).unwrap(), "preserve root note");
    }
    #[test]
    fn unreadable_cache_rejects_new_writes_and_can_be_retried_after_permissions_recover() {
        let data = tempfile::tempdir().unwrap();
        let roots = roots(data.path());
        // Unix root can bypass discretionary directory permissions; this product
        // check is exercised by the normal macOS account, not a privileged runner.
        if fs::metadata(data.path()).unwrap().uid() == 0 {
            return;
        }
        let dir = roots[1].join("staging");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o0)).unwrap();
        let result = check_staging([&roots[0], &roots[1]], HostKind::General, 1);
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(result.unwrap_err().code, "permission_denied");
        check_staging([&roots[0], &roots[1]], HostKind::General, 1).unwrap();
    }
    #[test]
    fn unknown_files_stay_intact_and_directory_work_is_bounded() {
        let data = tempfile::tempdir().unwrap();
        let roots = roots(data.path());
        for n in 0..WALK_LIMIT {
            fs::write(roots[0].join(format!("note-{n}.txt")), b"keep").unwrap();
        }
        let target = roots[0].join("new-target");
        assert_eq!(
            check_archives([&roots[0], &roots[1]], &target, 1)
                .unwrap_err()
                .code,
            "resource_limit"
        );
        assert_eq!(
            fs::read_to_string(roots[0].join("note-0.txt")).unwrap(),
            "keep"
        );
        assert_eq!(fs::read_dir(&roots[0]).unwrap().count(), WALK_LIMIT + 2);
    }
    #[test]
    fn case_aliases_cannot_hide_archive_bytes_or_double_charge_existing_targets() {
        let data = tempfile::tempdir().unwrap();
        let roots = roots(data.path());
        let canonical = archive(&roots[0], 0, ARCHIVES - 10);
        let name = canonical.with_file_name("PACKAGE.LAPLUG");
        fs::rename(&canonical, &name).unwrap();
        fs::rename(
            roots[0].join("packages/org.example.cache"),
            roots[0].join("packages/ORG.EXAMPLE.CACHE"),
        )
        .unwrap();
        fs::rename(roots[0].join("packages"), roots[0].join("PACKAGES")).unwrap();
        let alias_resolves = canonical.exists();
        let result = check_archives([&roots[0], &roots[1]], &canonical, ARCHIVES - 10);
        if alias_resolves {
            result.unwrap();
        } else {
            assert_eq!(result.unwrap_err().code, "resource_limit");
        }
        let new = roots[1].join("packages/new-target");
        check_archives([&roots[0], &roots[1]], &new, 10).unwrap();
        assert_eq!(
            check_archives([&roots[0], &roots[1]], &new, 11)
                .unwrap_err()
                .code,
            "resource_limit"
        );
    }
}
