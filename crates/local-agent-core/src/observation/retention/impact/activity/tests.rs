use super::*;
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt;

fn row(key: &str, at: &str) -> Value {
    json!({"key":key,"agent":"claude","sessionId":"claude:private-session","startedAt":at,"sourceStamp":null,"input":{"text":"private-excerpt-must-not-escape","at":at}})
}
fn save(root: &Path, rows: Vec<Value>) -> std::path::PathBuf {
    let path = root.join(ActivitySource::Gateway.filename());
    crate::providers::files::atomic_write(
        &path,
        &serde_json::to_vec(&json!({"version":1,"rows":rows})).unwrap(),
        0o600,
    )
    .unwrap();
    path
}
fn now() -> DateTime<Utc> {
    "2026-10-27T12:00:00.500000000Z".parse().unwrap()
}

#[test]
fn excerpt_age_preserves_nanoseconds_and_does_not_claim_deletion_eligibility() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    for days in [7, 30, 90] {
        let cutoff = now() - Duration::days(i64::from(days));
        let path = save(
            &root,
            vec![
                row("old", &(cutoff - Duration::nanoseconds(1)).to_rfc3339()),
                row("equal", &cutoff.to_rfc3339()),
                row("future", &(now() + Duration::nanoseconds(1)).to_rfc3339()),
                row("bad", "invalid"),
            ],
        );
        let original = std::fs::read(&path).unwrap();
        let result = activity(&root, ActivitySource::Gateway, days, now()).unwrap();
        assert_eq!(
            (
                result.records,
                result.older_than_cutoff,
                result.within_window,
                result.unknown_time
            ),
            (4, 1, 1, 2)
        );
        let public = serde_json::to_string(&result).unwrap();
        assert!(!public.contains("private"));
        assert!(!public.contains("expired"));
        assert_eq!(result.file_bytes, original.len() as u64);
        assert_eq!(std::fs::read(path).unwrap(), original);
    }
}
#[test]
fn real_cache_writer_and_native_copy_are_read_without_returning_their_content() {
    use crate::sessions::{activity_input::UserInput, activity_input_cache::InputCache};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let path = root.join(ActivitySource::Gateway.filename());
    let mut cache = InputCache::open(path.clone());
    cache.insert(CachedInput {
        key: "gateway-request".into(),
        agent: "claude".into(),
        session_id: None,
        started_at: None,
        source_stamp: None,
        native: None,
        input: UserInput {
            text: "private-excerpt-must-not-escape".into(),
            at: now().to_rfc3339(),
        },
    });
    cache.commit().unwrap();
    assert_eq!(
        activity(&root, ActivitySource::Gateway, 30, now())
            .unwrap()
            .within_window,
        1
    );
    let native = root.join(ActivitySource::Native.filename());
    std::fs::copy(path, &native).unwrap();
    std::fs::set_permissions(&native, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        activity(&root, ActivitySource::Native, 30, now())
            .unwrap()
            .records,
        1
    );
}
#[test]
fn absent_is_not_empty_and_reading_never_initializes_or_heals_a_file() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    assert!(matches!(
        activity(&root, ActivitySource::Gateway, 30, now()),
        Err(Error::SourceAbsent)
    ));
    assert!(std::fs::read_dir(&root).unwrap().next().is_none());
    let path = save(&root, vec![]);
    assert_eq!(
        activity(&root, ActivitySource::Gateway, 30, now())
            .unwrap()
            .records,
        0
    );
    for raw in [
        b"{broken".as_slice(),
        br#"{"version":1,"version":1,"rows":[]}"#,
        br#"{"version":2,"rows":[]}"#,
        br#"{"version":1,"rows":[],"future":null}"#,
    ] {
        std::fs::write(&path, raw).unwrap();
        assert!(matches!(
            activity(&root, ActivitySource::Gateway, 30, now()),
            Err(Error::UnsupportedSchema)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), raw);
    }
}
#[test]
fn unknown_nested_fields_duplicate_identity_and_wrong_provenance_are_unverified() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let baseline = row("r", &now().to_rfc3339());
    let mut future = baseline.clone();
    future["input"]["futureBody"] = json!("private-content");
    let mut wrong = baseline.clone();
    wrong["native"] = json!({"latest":true,"messageId":null,"eventAt":now().to_rfc3339(),"phase":"processing","model":null,"ttftMs":null});
    for rows in [vec![future], vec![baseline.clone(), baseline], vec![wrong]] {
        let path = save(&root, rows);
        let before = std::fs::read(&path).unwrap();
        assert!(matches!(
            activity(&root, ActivitySource::Gateway, 30, now()),
            Err(Error::UnsupportedSchema)
        ));
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
}
#[test]
fn capacity_limits_fail_without_publishing_partial_counts_or_rewriting_data() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut huge = row("r", &now().to_rfc3339());
    huge["input"]["text"] = json!("x".repeat(3073));
    for rows in [
        vec![huge],
        (0..513)
            .map(|n| row(&n.to_string(), &now().to_rfc3339()))
            .collect(),
    ] {
        let path = save(&root, rows);
        let before = std::fs::read(&path).unwrap();
        assert!(matches!(
            activity(&root, ActivitySource::Gateway, 30, now()),
            Err(Error::Limit)
        ));
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
    let path = save(&root, vec![]);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(FILE_BYTES + 1)
        .unwrap();
    assert!(matches!(
        activity(&root, ActivitySource::Gateway, 30, now()),
        Err(Error::Limit)
    ));
    assert_eq!(std::fs::metadata(path).unwrap().len(), FILE_BYTES + 1);
}
#[test]
fn links_public_permissions_and_nonregular_files_are_rejected() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let path = save(&root, vec![]);
    let target = root.join("private-target");
    std::fs::rename(&path, &target).unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    assert!(matches!(
        activity(&root, ActivitySource::Gateway, 30, now()),
        Err(Error::SourceChanged)
    ));
    std::fs::remove_file(&path).unwrap();
    std::fs::hard_link(&target, &path).unwrap();
    assert!(matches!(
        activity(&root, ActivitySource::Gateway, 30, now()),
        Err(Error::SourceChanged)
    ));
    std::fs::remove_file(&path).unwrap();
    std::fs::rename(&target, &path).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        activity(&root, ActivitySource::Gateway, 30, now()),
        Err(Error::SourceChanged)
    ));
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert!(matches!(
        activity(&root, ActivitySource::Gateway, 30, now()),
        Err(Error::SourceChanged)
    ));
}
#[test]
fn inventory_format_defaults_to_legacy_and_requires_explicit_supported_upgrade() {
    let mut v =
        json!({"storeId":uuid::Uuid::new_v4().to_string(),"expectedSettingsRevision":0,"days":30});
    let request: super::super::InventoryRequest = serde_json::from_value(v.clone()).unwrap();
    assert_eq!(request.inventory_version, 1);
    assert!(request.valid());
    for version in [0, 1, 2, 3] {
        v["inventoryVersion"] = json!(version);
        let request: super::super::InventoryRequest = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(request.valid(), version == 1 || version == 2);
    }
    v["future"] = json!(true);
    assert!(serde_json::from_value::<super::super::InventoryRequest>(v).is_err());
}
