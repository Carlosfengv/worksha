//! Pin the opened product database file, not just its reusable path or epoch.
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Error {
    Changed,
    Unavailable,
}

pub(super) struct SourceFile {
    path: PathBuf,
    file: File,
}

impl SourceFile {
    pub fn open(path: PathBuf) -> Result<Self, Error> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(&path).map_err(|_| Error::Unavailable)?;
        let source = Self { path, file };
        source.check()?;
        Ok(source)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn check(&self) -> Result<(), Error> {
        let current = std::fs::symlink_metadata(&self.path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::Changed
            } else {
                Error::Unavailable
            }
        })?;
        let opened = self.file.metadata().map_err(|_| Error::Unavailable)?;
        if !current.is_file() || current.file_type().is_symlink() {
            return Err(Error::Changed);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if current.dev() != opened.dev()
                || current.ino() != opened.ino()
                || current.nlink() != 1
            {
                return Err(Error::Changed);
            }
        }
        #[cfg(not(unix))]
        if current.created().map_err(|_| Error::Unavailable)?
            != opened.created().map_err(|_| Error::Unavailable)?
        {
            return Err(Error::Changed);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_and_removal_are_detected_even_if_bytes_match() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("source.db");
        std::fs::write(&path, b"same bytes").unwrap();
        let source = SourceFile::open(path.clone()).unwrap();
        std::fs::rename(&path, temp.path().join("original.db")).unwrap();
        std::fs::write(&path, b"same bytes").unwrap();
        assert_eq!(source.check(), Err(Error::Changed));
        std::fs::remove_file(path).unwrap();
        assert_eq!(source.check(), Err(Error::Changed));
    }

    #[test]
    #[cfg(unix)]
    fn symlinks_and_hardlinks_never_become_the_pinned_product_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("source.db");
        std::fs::write(&path, b"private bytes").unwrap();
        let source = SourceFile::open(path.clone()).unwrap();
        let alias = temp.path().join("alias.db");
        std::fs::hard_link(&path, &alias).unwrap();
        assert_eq!(source.check(), Err(Error::Changed));
        assert_eq!(SourceFile::open(alias.clone()).err(), Some(Error::Changed));
        std::fs::remove_file(&alias).unwrap();
        source.check().unwrap();
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        assert_eq!(SourceFile::open(alias).err(), Some(Error::Unavailable));
        assert_eq!(std::fs::read(path).unwrap(), b"private bytes");
    }
}
