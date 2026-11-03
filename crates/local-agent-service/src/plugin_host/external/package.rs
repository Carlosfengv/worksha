use super::{Checked, Result, fail};
use ed25519_dalek::{Signature as EdSignature, VerifyingKey};
use local_agent_core::plugin::external::*;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read},
    path::Path,
};

pub(crate) const MAX_ARCHIVE: u64 = 32 * 1_048_576;
const MAX_EXPANDED: u64 = 128 * 1_048_576;
pub(crate) fn hash(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub(crate) fn unhex<const N: usize>(text: &str) -> Result<[u8; N]> {
    if text.len() != 2 * N || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(fail("invalid_input"));
    }
    let mut bytes = [0; N];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).checked("invalid_input")?;
    }
    Ok(bytes)
}
pub(crate) fn valid_id(id: &str) -> bool {
    id.len() >= 5
        && id.len() <= 100
        && id.contains('.')
        && !id.starts_with("builtin.")
        && !id.starts_with("local-agent.")
        && id.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
}
fn label(text: &str, max: usize) -> bool {
    !text.is_empty() && text.len() <= max && !text.chars().any(char::is_control)
}
pub(crate) fn validate_header(manifest: &Manifest) -> Result<()> {
    if manifest.manifest_version != MANIFEST_VERSION {
        return Err(fail("incompatible_api"));
    }
    if !valid_id(&manifest.id)
        || !valid_id(&manifest.publisher)
        || !manifest.id.starts_with(&format!("{}.", manifest.publisher))
        || !label(&manifest.display_name, 120)
        || semver::Version::parse(&manifest.version).is_err()
    {
        return Err(fail("invalid_input"));
    }
    let requirement = semver::VersionReq::parse(&manifest.host_api).checked("incompatible_api")?;
    if !requirement.matches(&semver::Version::parse(API_VERSION).expect("host version")) {
        return Err(fail("incompatible_api"));
    }
    Ok(())
}
pub(crate) fn validate(manifest: &Manifest) -> Result<()> {
    if manifest.source.is_some() {
        return super::full::package::validate(manifest);
    }
    if manifest.kind.is_some() {
        return super::policy::package::validate(manifest);
    }
    if manifest.is_extension() {
        return super::advanced::validate_manifest(manifest);
    }
    validate_header(manifest)?;
    let allowed = [
        "session.source@1",
        "account.quota-source@1",
        "provider.catalog@1",
        local_agent_core::domain::provider_access::CAPABILITY,
        "ui.contribution@1",
        "report.view@1",
    ];
    let mut caps = BTreeSet::new();
    if manifest.capabilities.is_empty()
        || manifest
            .capabilities
            .iter()
            .any(|c| !allowed.contains(&c.as_str()) || !caps.insert(c))
    {
        return Err(fail("unsupported"));
    }
    let mut required = BTreeSet::new();
    if manifest.requires.len() > 16
        || manifest.requires.iter().any(|r| {
            !required.insert(r)
                || !(r == "host.source-broker@1"
                    || r == "host.connection-broker@1"
                    || r == local_agent_core::domain::provider_access::BROKER
                    || r == local_agent_core::plugin::contributions::OPTIONAL
                    || r == local_agent_core::plugin::contributions::DETAILS
                    || r == local_agent_core::plugin::object_contributions::FEATURE
                    || r.strip_prefix("plugin:").is_some_and(valid_id))
        })
    {
        return Err(fail("unsupported"));
    }
    let objects = manifest
        .requires
        .iter()
        .any(|r| r == local_agent_core::plugin::object_contributions::FEATURE);
    if objects {
        super::object_contributions::validate_manifest(manifest)?;
    }
    let source = manifest
        .capabilities
        .iter()
        .any(|s| s == "session.source@1");
    let quota = manifest
        .capabilities
        .iter()
        .any(|s| s == "account.quota-source@1");
    let catalog = super::catalog::opted_in(manifest);
    if catalog
        != manifest
            .requires
            .iter()
            .any(|r| r == local_agent_core::domain::provider_access::BROKER)
    {
        return Err(fail("permission_denied"));
    }
    let mut content = 0;
    let mut network = 0;
    let mut ui = 0;
    let report = manifest.capabilities.iter().any(|s| s == "report.view@1");
    let mut domain = 0;
    for p in &manifest.permissions {
        match p {
            Permission::SourceContentRead { resource } => {
                content += 1;
                if resource != "history" {
                    return Err(fail("unsupported"));
                }
            }
            Permission::ConnectionRequest { path } => {
                network += 1;
                if !path.starts_with('/')
                    || path.contains("..")
                    || !path
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"/-_".contains(&b))
                    || path.len() > 128
                {
                    return Err(fail("invalid_input"));
                }
            }
            Permission::UiContribute => ui += 1,
            Permission::DomainQuery { query } => {
                domain += 1;
                if query != "gateway.summary"
                    && !(objects
                        && query
                            == local_agent_core::plugin::object_contributions::QUERY_PERMISSION)
                {
                    return Err(fail("unsupported"));
                }
            }
        }
    }
    if content > 1
        || network > 1
        || ui > 1
        || (content > 0 && network > 0)
        || source != (content == 1)
        || (quota || catalog) != (network == 1)
        || (source && quota)
        || (catalog && (source || quota || report))
        || domain > 1
        || report != (domain == 1 && !objects)
        || (report && !matches!(manifest.runtime, Runtime::Declarative))
    {
        return Err(fail("permission_denied"));
    }
    match &manifest.runtime {
        Runtime::WasmComponent { entry, abi }
            if safe_path(entry)
                && entry.ends_with(".wasm")
                && abi == "local-agent:plugin@1.0.0" => {}
        Runtime::Declarative if !source && !quota && !catalog => {}
        _ => return Err(fail("unsupported")),
    }
    if source {
        let agent = manifest.agent.as_ref().ok_or(fail("invalid_input"))?;
        if !agent.product_id.starts_with(&format!("{}:", manifest.id))
            || !label(&agent.product_id, 140)
            || !label(&agent.display_name, 100)
        {
            return Err(fail("capability_conflict"));
        }
    } else if manifest.agent.is_some() {
        return Err(fail("invalid_input"));
    }
    if manifest.providers.len() > 16 || manifest.settings.len() > 16 || manifest.views.len() > 8 {
        return Err(fail("resource_limit"));
    }
    let mut ids = BTreeSet::new();
    for preset in &manifest.providers {
        if !ids.insert(&preset.id)
            || !local_id(&preset.id)
            || !label(&preset.name, 100)
            || !["messages", "responses", "chat-completions"].contains(&preset.protocol.as_str())
            || preset.model.len() > 200
        {
            return Err(fail("invalid_input"));
        }
        local_agent_core::providers::validate_endpoint(&preset.endpoint)
            .checked("invalid_input")?;
    }
    if !manifest.providers.is_empty() && !caps.contains(&"provider.catalog@1".to_string()) {
        return Err(fail("invalid_input"));
    }
    ids.clear();
    for setting in &manifest.settings {
        if !ids.insert(&setting.key)
            || !local_id(&setting.key)
            || !label(&setting.label, 100)
            || setting.options.is_empty()
            || setting.options.len() > 32
            || !setting.options.contains(&setting.default)
            || setting.options.iter().any(|v| !label(v, 100))
        {
            return Err(fail("invalid_input"));
        }
    }
    ids.clear();
    for view in &manifest.views {
        if !ids.insert(&view.id)
            || !local_id(&view.id)
            || !label(&view.title, 100)
            || !label(&view.description, 1000)
            || ui != 1
        {
            return Err(fail("invalid_input"));
        }
        if (view.kind == ViewKind::Sessions && !source)
            || (view.kind == ViewKind::Quota && !quota)
            || (view.kind == ViewKind::Providers && manifest.providers.is_empty())
            || (view.kind == ViewKind::Report && !report && !objects)
        {
            return Err(fail("invalid_input"));
        }
    }
    Ok(())
}
pub(crate) fn local_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_".contains(&b))
}
fn safe_path(path: &str) -> bool {
    path.len() <= 160
        && !path.is_empty()
        && path.is_ascii()
        && !path.contains('\\')
        && !path.contains(':')
        && !path.starts_with('/')
        && path.split('/').all(|s| {
            !s.is_empty()
                && s != "."
                && s != ".."
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
}
pub(crate) fn signing_bytes(
    manifest: &Manifest,
    files: &BTreeMap<String, String>,
) -> Result<Vec<u8>> {
    let mut bytes = SIGNING_DOMAIN.to_vec();
    bytes.extend(
        serde_json::to_vec(&serde_json::to_value(manifest).checked("invalid_input")?)
            .checked("invalid_input")?,
    );
    bytes.push(b'\n');
    bytes.extend(serde_json::to_vec(files).checked("invalid_input")?);
    Ok(bytes)
}
pub(crate) struct Package {
    pub manifest: Manifest,
    pub digest: String,
    pub signing_key: Option<String>,
    pub files: BTreeMap<String, Vec<u8>>,
    pub expanded: u64,
}
pub(crate) struct SignedPackage {
    pub manifest: Manifest,
    pub digest: String,
    pub signing_key: String,
    #[cfg(test)]
    pub files: BTreeMap<String, Vec<u8>>,
    pub expanded: u64,
}
impl Package {
    pub(crate) fn require_signature(self) -> Result<SignedPackage> {
        Ok(SignedPackage {
            manifest: self.manifest,
            digest: self.digest,
            signing_key: self.signing_key.ok_or(fail("publisher_untrusted"))?,
            #[cfg(test)]
            files: self.files,
            expanded: self.expanded,
        })
    }
}
pub(crate) fn inspect(bytes: &[u8], development: bool) -> Result<Package> {
    if bytes.len() as u64 > MAX_ARCHIVE {
        return Err(fail("resource_limit"));
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).checked("invalid_input")?;
    if archive.len() > 1000 {
        return Err(fail("resource_limit"));
    }
    let mut files = BTreeMap::new();
    let mut folded = BTreeSet::new();
    let mut total = 0;
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).checked("invalid_input")?;
        let name = file.name().checked("invalid_input")?.into_owned();
        if !safe_path(&name)
            || file.is_dir()
            || !folded.insert(name.to_ascii_lowercase())
            || file
                .unix_mode()
                .is_some_and(|mode| mode & 0o170000 != 0 && mode & 0o170000 != 0o100000)
        {
            return Err(fail("invalid_input"));
        }
        if file.size() > 32 * 1_048_576 || total + file.size() > MAX_EXPANDED {
            return Err(fail("resource_limit"));
        }
        let mut content = Vec::new();
        file.by_ref()
            .take(32 * 1_048_576 + 1)
            .read_to_end(&mut content)
            .checked("invalid_input")?;
        if content.len() > 32 * 1_048_576 {
            return Err(fail("resource_limit"));
        }
        total += content.len() as u64;
        if total > MAX_EXPANDED {
            return Err(fail("resource_limit"));
        }
        files.insert(name, content);
    }
    let manifest_bytes = files.get("manifest.json").ok_or(fail("invalid_input"))?;
    if manifest_bytes.len() > 65536 {
        return Err(fail("resource_limit"));
    }
    let manifest: Manifest = serde_json::from_slice(manifest_bytes).checked("invalid_input")?;
    validate(&manifest)?;
    super::contributions::parse(&manifest, &files)?;
    super::object_contributions::parse(&manifest, &files)?;
    if manifest.is_extension() {
        super::advanced::validate_policy(&manifest, &files)?;
    }
    if super::catalog::opted_in(&manifest) {
        super::catalog::validate(&manifest, &files)?;
    }
    let index: BTreeMap<String, String> =
        serde_json::from_slice(files.get("files.json").ok_or(fail("invalid_input"))?)
            .checked("invalid_input")?;
    let payload: BTreeMap<_, _> = files
        .iter()
        .filter(|(name, _)| {
            !["manifest.json", "files.json", "signature.json"].contains(&name.as_str())
        })
        .map(|(name, value)| (name.clone(), hash(value)))
        .collect();
    if index != payload {
        return Err(fail("signature_invalid"));
    }
    let signing_key = if let Some(bytes) = files.get("signature.json") {
        let sig: Signature = serde_json::from_slice(bytes).checked("signature_invalid")?;
        if sig.algorithm != "ed25519" {
            return Err(fail("signature_invalid"));
        }
        let key_bytes = unhex::<32>(&sig.public_key)?;
        if hex(&key_bytes) != sig.public_key {
            return Err(fail("signature_invalid"));
        }
        let key = VerifyingKey::from_bytes(&key_bytes).checked("signature_invalid")?;
        key.verify_strict(
            &signing_bytes(&manifest, &index)?,
            &EdSignature::from_bytes(&unhex::<64>(&sig.signature)?),
        )
        .checked("signature_invalid")?;
        Some(sig.public_key)
    } else if development && manifest.source.is_none() && manifest.kind.is_none() {
        None
    } else {
        return Err(fail("publisher_untrusted"));
    };
    if let Runtime::WasmComponent { entry, .. } = &manifest.runtime
        && !files
            .get(entry)
            .is_some_and(|b| b.starts_with(b"\0asm\x0d\0\x01\0"))
    {
        return Err(fail("invalid_input"));
    }
    Ok(Package {
        manifest,
        digest: hash(bytes),
        signing_key,
        files,
        expanded: total,
    })
}
pub(crate) fn read_file(path: &Path, limit: u64) -> Result<Vec<u8>> {
    read_bound_file(path, limit, None)
}
pub(crate) fn read_granted_file(
    path: &Path,
    limit: u64,
    device: u64,
    inode: u64,
) -> Result<Vec<u8>> {
    read_bound_file(path, limit, Some((device, inode)))
}
fn read_bound_file(path: &Path, limit: u64, identity: Option<(u64, u64)>) -> Result<Vec<u8>> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .checked("permission_denied")?;
    let before = file.metadata().checked("permission_denied")?;
    // Bind the grant to the descriptor actually opened, not an earlier path stat.
    if identity.is_some_and(|expected| expected != (before.dev(), before.ino())) {
        return Err(fail("permission_denied"));
    }
    if !before.is_file() || before.nlink() != 1 || before.len() > limit {
        return Err(fail("resource_limit"));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .checked("permission_denied")?;
    let after = file.metadata().checked("permission_denied")?;
    let current = std::fs::symlink_metadata(path).checked("permission_denied")?;
    if bytes.len() as u64 > limit
        || before.dev() != current.dev()
        || before.ino() != current.ino()
        || before.len() != after.len()
        || before.mtime_nsec() != after.mtime_nsec()
        || before.mtime() != after.mtime()
        || before.ctime_nsec() != after.ctime_nsec()
        || before.ctime() != after.ctime()
        || after.len() != current.len()
        || after.mtime_nsec() != current.mtime_nsec()
        || after.mtime() != current.mtime()
        || after.ctime_nsec() != current.ctime_nsec()
        || after.ctime() != current.ctime()
        || after.nlink() != 1
        || current.nlink() != 1
        || current.file_type().is_symlink()
    {
        return Err(fail("stale_snapshot"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod bound_read_tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;
    #[test]
    fn all_capability_hosts_share_signed_packages_without_sharing_permissions() {
        let examples = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sdk/plugins/examples");
        let key = ed25519_dalek::SigningKey::from_bytes(&[42; 32]);
        for name in [
            "provider-presets",
            "json-config",
            "full-history",
            "pure-policy",
        ] {
            let source = examples.join(name).join("package");
            let folder = tempfile::tempdir().unwrap();
            for file in std::fs::read_dir(&source).unwrap() {
                let file = file.unwrap();
                if file
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "json")
                {
                    std::fs::copy(file.path(), folder.path().join(file.file_name())).unwrap();
                }
            }
            let manifest: Manifest = serde_json::from_slice(
                &std::fs::read(folder.path().join("manifest.json")).unwrap(),
            )
            .unwrap();
            if matches!(manifest.runtime, Runtime::WasmComponent { .. }) {
                std::fs::write(folder.path().join("plugin.wasm"), b"\0asm\x0d\0\x01\0").unwrap();
            }
            let bytes = super::super::sdk::pack(folder.path(), Some(&key)).unwrap();
            let checked = inspect(&bytes, false).unwrap();
            assert_eq!(checked.manifest, manifest, "{name}");
            assert_eq!(
                checked.signing_key.as_deref(),
                Some(hex(&key.verifying_key().to_bytes()).as_str())
            );
            let mut incompatible = manifest.clone();
            incompatible.manifest_version = 99;
            assert_eq!(
                validate(&incompatible).unwrap_err().code,
                "incompatible_api"
            );
            if manifest.source.is_some() || manifest.kind.is_some() {
                let mut mixed = manifest;
                mixed.permissions.push(Permission::ConnectionRequest {
                    path: "/private".into(),
                });
                assert!(
                    validate(&mixed).is_err(),
                    "{name} must not acquire network authority"
                );
            }
        }
        assert_eq!(
            schemas()["manifest"],
            local_agent_core::plugin::full::schemas()["manifest"]
        );
        assert_eq!(
            schemas()["manifest"],
            local_agent_core::plugin::policy::schemas()["manifest"]
        );
    }
    #[test]
    fn granted_read_rejects_a_replacement_after_the_earlier_path_check() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("selected.jsonl");
        let other = root.path().join("other.jsonl");
        std::fs::write(&path, b"authorized fixture\n").unwrap();
        let grant = std::fs::symlink_metadata(&path).unwrap();
        assert_eq!(
            read_granted_file(&path, 100, grant.dev(), grant.ino()).unwrap(),
            b"authorized fixture\n"
        );
        // The old broker's preliminary path check has already succeeded.
        let before_open = std::fs::symlink_metadata(&path).unwrap();
        assert_eq!(before_open.ino(), grant.ino());
        std::fs::write(&other, b"different private fixture\n").unwrap();
        std::fs::rename(&other, &path).unwrap();
        assert_eq!(
            read_granted_file(&path, 100, grant.dev(), grant.ino())
                .unwrap_err()
                .code,
            "permission_denied"
        );
        // Without the grant binding this replacement is an ordinary readable file.
        assert_eq!(
            read_file(&path, 100).unwrap(),
            b"different private fixture\n"
        );
    }
}
