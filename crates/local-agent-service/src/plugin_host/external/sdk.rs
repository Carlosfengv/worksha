//! Shipped authoring/checking utility; never called by installed guests.
use super::{Checked, Result, fail, package};
use ed25519_dalek::{Signer, SigningKey};
use local_agent_core::plugin::external::*;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::Path,
};

pub(crate) fn run(args: Vec<String>) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("schemas")
        | Some("schemas-builtin")
        | Some("schemas-extensions")
        | Some("schemas-contributions")
        | Some("schemas-object-summary")
        | Some("schemas-request-export")
        | Some("schemas-object-contributions")
        | Some("schemas-domain-facts")
        | Some("schemas-domain-provider")
        | Some("schemas-domain-config")
        | Some("schemas-domain-protocol")
        | Some("schemas-provider-access")
        | Some("schemas-provider-connection")
        | Some("schemas-provider-source")
        | Some("schemas-provider-source-operation")
        | Some("schemas-provider-draft-catalog")
        | Some("schemas-provider-draft-grant")
        | Some("schemas-provider-draft-recovery")
        | Some("schemas-provider-presets")
        | Some("schemas-routing-policy")
        | Some("schemas-routing-policy-recovery")
        | Some("schemas-policy")
        | Some("schemas-sources")
        | Some("schemas-domain") => {
            let root = Path::new(args.get(1).ok_or(fail("invalid_input"))?);
            std::fs::create_dir_all(root).checked("unavailable")?;
            let schemas = if args[0] == "schemas-builtin" {
                local_agent_core::plugin::builtin::schemas()
            } else if args[0] == "schemas-routing-policy-recovery" {
                local_agent_core::domain::routing_policy_recovery::schemas()
            } else if args[0] == "schemas-request-export" {
                local_agent_core::domain::requests::export::schemas()
            } else if args[0] == "schemas-object-summary" {
                local_agent_core::domain::object_summary::schemas()
            } else if args[0] == "schemas-object-contributions" {
                local_agent_core::plugin::object_contributions::schemas()
            } else if args[0] == "schemas-policy" {
                local_agent_core::plugin::policy::schemas()
            } else if args[0] == "schemas-routing-policy" {
                local_agent_core::domain::routing_policy::schemas()
            } else if args[0] == "schemas-provider-presets" {
                local_agent_core::domain::provider_presets::schemas()
            } else if args[0] == "schemas-provider-draft-recovery" {
                local_agent_core::domain::provider_draft_recovery::schemas()
            } else if args[0] == "schemas-provider-draft-grant" {
                local_agent_core::domain::provider_draft_grant::schemas()
            } else if args[0] == "schemas-provider-draft-catalog" {
                local_agent_core::domain::provider_draft_catalog::schemas()
            } else if args[0] == "schemas-provider-source-operation" {
                local_agent_core::domain::provider_source_operation::schemas()
            } else if args[0] == "schemas-provider-source" {
                local_agent_core::domain::provider_source::schemas()
            } else if args[0] == "schemas-provider-connection" {
                local_agent_core::domain::provider_connection::schemas()
            } else if args[0] == "schemas-provider-access" {
                local_agent_core::domain::provider_access::schemas()
            } else if args[0] == "schemas-domain-protocol" {
                local_agent_core::domain::protocol::schemas()
            } else if args[0] == "schemas-domain-config" {
                local_agent_core::domain::config::schemas()
            } else if args[0] == "schemas-domain-provider" {
                local_agent_core::domain::provider::schemas()
            } else if args[0] == "schemas-sources" {
                local_agent_core::plugin::full::schemas()
            } else if args[0] == "schemas-domain-facts" {
                local_agent_core::domain::facts::schemas()
            } else if args[0] == "schemas-domain" {
                local_agent_core::domain::schemas()
            } else if args[0] == "schemas-contributions" {
                local_agent_core::plugin::contributions::schemas()
            } else if args[0] == "schemas-extensions" {
                local_agent_core::plugin::advanced::schemas()
            } else {
                schemas()
            };
            for (name, value) in schemas.as_object().ok_or(fail("invalid_input"))? {
                std::fs::write(
                    root.join(format!("{name}.schema.json")),
                    format!(
                        "{}\n",
                        serde_json::to_string_pretty(value).checked("invalid_input")?
                    ),
                )
                .checked("unavailable")?;
            }
        }
        Some("keygen") => {
            use std::os::unix::fs::OpenOptionsExt;
            let path = Path::new(args.get(1).ok_or(fail("invalid_input"))?);
            let mut seed = zeroize::Zeroizing::new([0u8; 32]);
            std::fs::File::open("/dev/urandom")
                .checked("unavailable")?
                .read_exact(seed.as_mut())
                .checked("unavailable")?;
            let key = SigningKey::from_bytes(&seed);
            let mut f = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(path)
                .checked("unavailable")?;
            f.write_all(package::hex(seed.as_ref()).as_bytes())
                .checked("unavailable")?;
            println!(
                "{}",
                serde_json::json!({"publicKey":package::hex(&key.verifying_key().to_bytes()),"algorithm":"ed25519"})
            );
        }
        Some("pack") | Some("pack-development") => {
            let source = Path::new(args.get(1).ok_or(fail("invalid_input"))?);
            let output = Path::new(args.get(2).ok_or(fail("invalid_input"))?);
            let key = if args[0] != "pack-development" {
                let raw = zeroize::Zeroizing::new(
                    std::fs::read_to_string(args.get(3).ok_or(fail("invalid_input"))?)
                        .checked("invalid_input")?,
                );
                Some(SigningKey::from_bytes(&package::unhex::<32>(raw.trim())?))
            } else {
                None
            };
            let bytes = pack(source, key.as_ref())?;
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .mode(0o600)
                .open(output)
                .checked("unavailable")?;
            f.write_all(&bytes).checked("unavailable")?;
            println!(
                "{}",
                serde_json::json!({"digest":package::hash(&bytes),"bytes":bytes.len()})
            );
        }
        Some("check") => {
            let path = Path::new(args.get(1).ok_or(fail("invalid_input"))?);
            let bytes = package::read_file(path, package::MAX_ARCHIVE)?;
            let p = package::inspect(&bytes, args.get(2).is_some_and(|s| s == "--development"))?;
            println!(
                "{}",
                serde_json::json!({"valid":true,"apiVersion":API_VERSION,"manifest":p.manifest,"digest":p.digest,"signingKey":p.signing_key,"expandedBytes":p.expanded.to_string(),"payloadFiles":p.files.len(),"publisherTrusted":false})
            );
        }
        Some("componentize") => {
            let input = std::fs::read(args.get(1).ok_or(fail("invalid_input"))?)
                .checked("invalid_input")?;
            let component = wit_component::ComponentEncoder::default()
                .module(&input)
                .checked("invalid_input")?
                .validate(true)
                .encode()
                .checked("invalid_input")?;
            std::fs::write(args.get(2).ok_or(fail("invalid_input"))?, component)
                .checked("unavailable")?;
        }
        _ => return Err(fail("invalid_input")),
    }
    Ok(())
}
pub(crate) fn pack_files(root: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    fn visit(root: &Path, folder: &Path, files: &mut BTreeMap<String, Vec<u8>>) -> Result<()> {
        for entry in std::fs::read_dir(folder).checked("invalid_input")? {
            let entry = entry.checked("invalid_input")?;
            let path = entry.path();
            let kind = entry.file_type().checked("invalid_input")?;
            if kind.is_symlink() {
                return Err(fail("invalid_input"));
            }
            if kind.is_dir() {
                visit(root, &path, files)?;
            } else {
                let name = path
                    .strip_prefix(root)
                    .checked("invalid_input")?
                    .to_str()
                    .ok_or(fail("invalid_input"))?
                    .to_owned();
                if ["files.json", "signature.json"].contains(&name.as_str()) {
                    return Err(fail("invalid_input"));
                }
                files.insert(name, package::read_file(&path, package::MAX_ARCHIVE)?);
            }
            if files.len() > 1000
                || files.values().map(Vec::len).sum::<usize>() > package::MAX_ARCHIVE as usize
            {
                return Err(fail("resource_limit"));
            }
        }
        Ok(())
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files)?;
    Ok(files)
}
pub(crate) fn pack(root: &Path, key: Option<&SigningKey>) -> Result<Vec<u8>> {
    let mut files = pack_files(root)?;
    let manifest: Manifest =
        serde_json::from_slice(files.get("manifest.json").ok_or(fail("invalid_input"))?)
            .checked("invalid_input")?;
    package::validate(&manifest)?;
    let index: BTreeMap<_, _> = files
        .iter()
        .filter(|(name, _)| name.as_str() != "manifest.json")
        .map(|(name, content)| (name.clone(), package::hash(content)))
        .collect();
    files.insert(
        "files.json".into(),
        serde_json::to_vec(&index).checked("invalid_input")?,
    );
    if let Some(key) = key {
        let signature = Signature {
            algorithm: "ed25519".into(),
            public_key: package::hex(&key.verifying_key().to_bytes()),
            signature: package::hex(
                &key.sign(&package::signing_bytes(&manifest, &index)?)
                    .to_bytes(),
            ),
        };
        files.insert(
            "signature.json".into(),
            serde_json::to_vec(&signature).checked("invalid_input")?,
        );
    }
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, content) in files {
        writer
            .start_file(
                name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored)
                    .unix_permissions(0o600),
            )
            .checked("invalid_input")?;
        writer.write_all(&content).checked("invalid_input")?;
    }
    let bytes = writer.finish().checked("invalid_input")?.into_inner();
    package::inspect(&bytes, key.is_none())?;
    Ok(bytes)
}
