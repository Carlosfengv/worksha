//! Additive source package and fact host. Legacy package and database formats stay separate.
pub(super) mod api;
mod connection;
mod consent;
pub(crate) use connection::ConnectionAccess;
mod view;
pub(crate) use view::SourceView;
mod lifecycle;
mod manage;
pub(super) mod package;
mod recovery;
mod runtime;
pub(in crate::plugin_host::external) mod store;
mod watch;
pub(super) use store::Host;
#[cfg(test)]
mod manage_tests;
#[cfg(test)]
mod recovery_tests;

/// Legacy execution never waits for the optional source SQLite/cache mutex.
pub(super) fn legacy_conflict(legacy_root: &std::path::Path, id: &str) -> super::Result<bool> {
    if !super::package::valid_id(id) {
        return Err(super::fail("invalid_input"));
    }
    let root = legacy_root.with_file_name("plugin-sources");
    match std::fs::symlink_metadata(root.join("selected").join(id)) {
        Ok(_) => Ok(true),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(false)
        }
        Err(_) => Err(super::fail("permission_denied")),
    }
}
