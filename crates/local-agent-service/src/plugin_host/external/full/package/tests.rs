use super::super::super::package as common;
use super::*;
use ed25519_dalek::Signer;
use serde_json::json;
use std::{
    collections::BTreeMap,
    io::{Cursor, Write},
};

fn manifest() -> Manifest {
    serde_json::from_value(json!({
        "manifestVersion":1,"id":"org.example.facts","publisher":"org.example",
        "displayName":"只读事实试点","version":"1.0.0","hostApi":"^1.0.0",
        "capabilities":["domain.agent-directory@1","domain.session-source@1","domain.call-source@1","domain.project-source@1","domain.activity-source@1"],
        "requires":[SOURCE_BROKER,FACT_FEATURE],"permissions":[{"kind":"source.content.read","resource":"history"}],
        "runtime":{"kind":"wasm-component","entry":"plugin.wasm","abi":"local-agent:plugin@1.0.0"},
        "source":{"id":"history","products":[{"id":"cli","displayName":"Example CLI"}]}
    })).unwrap()
}
fn folder() -> tempfile::TempDir {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(
        folder.path().join("manifest.json"),
        serde_json::to_vec(&manifest()).unwrap(),
    )
    .unwrap();
    // Signature/parser tests do not instantiate or execute this payload.
    std::fs::write(folder.path().join("plugin.wasm"), b"\0asm\x0d\0\x01\0").unwrap();
    folder
}
fn zip(files: BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, content) in files {
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(&content).unwrap();
    }
    writer.finish().unwrap().into_inner()
}
#[test]
fn source_packages_use_the_common_manifest_and_signing_domain() {
    let source = folder();
    let key = SigningKey::from_bytes(&[17; 32]);
    let bytes = pack(source.path(), &key).unwrap();
    let checked = inspect(&bytes).unwrap();
    assert_eq!(checked.manifest, manifest());
    assert_eq!(
        checked.signing_key,
        common::hex(&key.verifying_key().to_bytes())
    );
    assert_eq!(common::inspect(&bytes, false).unwrap().manifest, manifest());
    assert_eq!(
        serde_json::from_value::<local_agent_core::plugin::external::Manifest>(
            serde_json::to_value(manifest()).unwrap()
        )
        .unwrap(),
        manifest()
    );
    let index: BTreeMap<String, String> =
        serde_json::from_slice(&checked.files["files.json"]).unwrap();
    let signing = signing_bytes(&checked.manifest, &index).unwrap();
    assert!(signing.starts_with(SIGNING_DOMAIN));
    let mut wrong_domain = b"another-application-plugin\n".to_vec();
    wrong_domain.extend_from_slice(&signing[SIGNING_DOMAIN.len()..]);
    let sig = Signature {
        algorithm: "ed25519".into(),
        public_key: checked.signing_key,
        signature: common::hex(&key.sign(&wrong_domain).to_bytes()),
    };
    let mut files = checked.files;
    files.insert("signature.json".into(), serde_json::to_vec(&sig).unwrap());
    assert_eq!(
        inspect(&zip(files)).err().unwrap().code,
        "signature_invalid"
    );
}
#[test]
fn missing_signature_changed_payload_and_changed_manifest_fail_closed() {
    let source = folder();
    let key = SigningKey::from_bytes(&[18; 32]);
    let checked = inspect(&pack(source.path(), &key).unwrap()).unwrap();
    let mut missing = checked.files.clone();
    missing.remove("signature.json");
    assert_eq!(
        inspect(&zip(missing)).err().unwrap().code,
        "publisher_untrusted"
    );
    let mut changed = checked.files.clone();
    changed.insert("plugin.wasm".into(), b"changed".to_vec());
    assert_eq!(
        inspect(&zip(changed)).err().unwrap().code,
        "signature_invalid"
    );
    let mut changed = checked.files;
    let mut m = manifest();
    m.display_name = "different".into();
    changed.insert("manifest.json".into(), serde_json::to_vec(&m).unwrap());
    assert_eq!(
        inspect(&zip(changed)).err().unwrap().code,
        "signature_invalid"
    );
}
#[test]
fn only_declared_bounded_history_capabilities_and_namespaces_are_admitted() {
    let base = manifest();
    validate(&base).unwrap();
    assert_eq!(
        base.source_id().as_deref(),
        Some("org.example.facts:history")
    );
    assert_eq!(base.product_ids(), ["org.example.facts:cli"]);
    assert_eq!(base.fact_kinds().len(), 5);
    for change in [
        "publisher",
        "feature",
        "permission",
        "duplicate",
        "reserved",
        "product",
    ] {
        let mut m = base.clone();
        match change {
            "publisher" => m.publisher = "org.other".into(),
            "feature" => m.requires[0] = "host.arbitrary-files@1".into(),
            "permission" => m.permissions.clear(),
            "duplicate" => m.capabilities.push(Capability::CallSource.as_str().into()),
            "reserved" => m.id = "builtin.facts".into(),
            _ => {
                let s = m.source.as_mut().unwrap();
                s.products.push(s.products[0].clone());
            }
        }
        assert!(validate(&m).is_err(), "{change}");
    }
    let mut m = base.clone();
    m.host_api = "^99.0.0".into();
    assert_eq!(validate(&m).unwrap_err().code, "incompatible_api");
    for (key, value) in [("script", json!("forbidden")), ("sql", json!("forbidden"))] {
        let mut v = serde_json::to_value(&base).unwrap();
        v[key] = value;
        assert!(serde_json::from_value::<Manifest>(v).is_err());
    }
    let mut v = serde_json::to_value(&base).unwrap();
    v["capabilities"][0] = json!("domain.routing-policy@1");
    assert!(validate(&serde_json::from_value::<Manifest>(v).unwrap()).is_err());
    let mut v = serde_json::to_value(&base).unwrap();
    v["runtime"]["kind"] = json!("wasi");
    assert!(serde_json::from_value::<Manifest>(v).is_err());
}
#[test]
fn unsafe_archive_paths_and_aliases_are_rejected_before_activation() {
    for name in [
        "../file",
        "/absolute",
        "a//b",
        "a/./b",
        "a\\b",
        "file:stream",
    ] {
        assert!(inspect(&zip(BTreeMap::from([(name.to_string(), vec![])]))).is_err());
    }
    assert!(
        inspect(&zip(BTreeMap::from([
            ("manifest.json".into(), vec![]),
            ("Manifest.json".into(), vec![])
        ])))
        .is_err()
    );
}

#[test]
fn strict_source_schemas_match_the_checked_in_contract() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/plugins/sources");
    for (name, expected) in local_agent_core::plugin::full::schemas()
        .as_object()
        .unwrap()
    {
        let actual: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.join(format!("{name}.schema.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(&actual, expected, "source schema drift: {name}");
    }
}
