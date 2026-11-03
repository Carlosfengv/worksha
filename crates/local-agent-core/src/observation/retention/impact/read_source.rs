use super::{Error, unavailable};
use rusqlite::{Connection, OpenFlags};
use std::{
    fs::Metadata,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub(super) struct ReadSource {
    pub db: Connection,
    pub started: Instant,
    path: PathBuf,
    before: Metadata,
    budget: Duration,
}
impl ReadSource {
    // File names come only from the two product-owned source call sites.
    pub fn open(directory: &Path, filename: &'static str, budget: Duration) -> Result<Self, Error> {
        let absent = |e: std::io::Error| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::SourceAbsent
            } else {
                Error::Unavailable
            }
        };
        let path = directory.canonicalize().map_err(absent)?.join(filename);
        let before = std::fs::symlink_metadata(&path).map_err(absent)?;
        if !before.is_file() || before.file_type().is_symlink() {
            return Err(Error::SourceChanged);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if before.nlink() != 1 {
                return Err(Error::SourceChanged);
            }
        }
        let started = Instant::now();
        let db = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(unavailable)?;
        db.busy_timeout(Duration::from_millis(250))
            .map_err(unavailable)?;
        db.execute_batch("PRAGMA query_only=ON; PRAGMA cache_size=-2048;")
            .map_err(unavailable)?;
        db.progress_handler(10_000, Some(move || started.elapsed() >= budget))
            .map_err(unavailable)?;
        Ok(Self {
            db,
            started,
            path,
            before,
            budget,
        })
    }
    pub fn finish(&self) -> Result<(), Error> {
        let after = std::fs::symlink_metadata(&self.path).map_err(unavailable)?;
        if !after.is_file() || after.file_type().is_symlink() {
            return Err(Error::SourceChanged);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if self.before.dev() != after.dev()
                || self.before.ino() != after.ino()
                || after.nlink() != 1
            {
                return Err(Error::SourceChanged);
            }
        }
        if self.started.elapsed() >= self.budget {
            return Err(Error::Limit);
        }
        Ok(())
    }
}

pub(super) fn table(
    db: &Connection,
    name: &str,
    expected: &[(&str, &str, u8, u8)],
) -> Result<bool, Error> {
    let ordinary:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM pragma_table_list WHERE schema='main' AND name=?1 AND type='table' AND wr=0 AND strict=0)",[name],|r|r.get(0)).map_err(unavailable)?;
    let found = db
        .prepare("SELECT name,type,\"notnull\",pk,hidden FROM pragma_table_xinfo(?1) ORDER BY cid")
        .map_err(unavailable)?
        .query_map([name], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, u8>(2)?,
                r.get::<_, u8>(3)?,
                r.get::<_, u8>(4)?,
            ))
        })
        .map_err(unavailable)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(unavailable)?;
    Ok(ordinary
        && found.len() == expected.len()
        && found
            .iter()
            .zip(expected)
            .all(|(f, e)| f.0 == e.0 && f.1 == e.1 && f.2 == e.2 && f.3 == e.3 && f.4 == 0))
}
