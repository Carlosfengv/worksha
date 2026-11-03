use super::*;
use crate::domain::facts::{query::FactQuery, *};
use serde_json::json;
use std::{cell::Cell, fs};

fn config(home: &std::path::Path, name: &str) {
    fs::write(home.join(".claude.json"), serde_json::to_vec(&json!({"mcpServers":{
        name:{"command":"never-execute-catalog-command", "env":{"TOKEN":"private-catalog-secret"}}
    }})).unwrap()).unwrap();
}
fn query() -> FactQuery {
    serde_json::from_value(
        json!({"scope":{"kind":"builtin","sourceIds":["claude"]},"kind":"mcp-catalog"}),
    )
    .unwrap()
}
fn facts(native: &NativeSessions, q: &FactQuery) -> (Vec<FactRecord>, FactSummary) {
    native
        .domain_facts_for_source("claude", q, None, &[], d::Coverage::Complete)
        .unwrap()
}
#[test]
fn catalog_publication_is_a_real_consumer_and_plain_queries_do_not_open_configuration() {
    let dir = tempfile::tempdir().unwrap();
    config(dir.path(), "first");
    let native = NativeSessions::new(dir.path().into(), &["claude".into()]);
    let (rows, summary) = facts(&native, &query());
    assert!(rows.is_empty());
    assert!(summary.mcp_catalogs.is_none());
    native.refresh().unwrap();
    let before = native.domain_fact_catalog_revision().unwrap();
    let (rows, summary) = facts(&native, &query());
    assert_eq!(rows.len(), 1);
    assert_eq!(summary.mcp_catalogs.unwrap().value(), 1);
    let FactCandidate::McpCatalog(value) = &rows[0].value else {
        panic!("catalog")
    };
    assert_eq!(value.server_name, "first");
    assert!(value.tool_names.is_none());
    assert_eq!(
        rows[0].identity.provider_key,
        "builtin:domain.mcp-source@1:native"
    );
    let serialized = serde_json::to_string(&rows).unwrap();
    for private in [
        dir.path().to_str().unwrap(),
        "private-catalog-secret",
        "never-execute-catalog-command",
    ] {
        assert!(!serialized.contains(private));
    }
    config(dir.path(), "changed-on-disk");
    assert_eq!(facts(&native, &query()).0, rows);
    assert_eq!(native.domain_fact_catalog_revision().unwrap(), before);
    {
        let mut cache = native.mcp_catalog.lock().unwrap();
        cache.published.as_mut().unwrap().snapshot.entries[0].name = "from-common-result".into();
    }
    let changed = facts(&native, &query()).0;
    let FactCandidate::McpCatalog(value) = &changed[0].value else {
        panic!("catalog")
    };
    assert_eq!(value.server_name, "from-common-result");
    assert_eq!(changed[0].id, rows[0].id);
    native.refresh().unwrap();
    assert_ne!(native.domain_fact_catalog_revision().unwrap(), before);
    let FactCandidate::McpCatalog(value) = &facts(&native, &query()).0[0].value else {
        panic!("catalog")
    };
    assert_eq!(value.server_name, "changed-on-disk");
}
#[test]
fn unchanged_refresh_keeps_frozen_identity_but_expiry_and_failed_config_change_the_vector() {
    let dir = tempfile::tempdir().unwrap();
    config(dir.path(), "stable");
    let native = NativeSessions::new(dir.path().into(), &["claude".into()]);
    native.refresh().unwrap();
    let original = facts(&native, &query()).0;
    let vector = native.domain_fact_catalog_revision().unwrap();
    native.refresh().unwrap();
    assert_eq!(native.domain_fact_catalog_revision().unwrap(), vector);
    assert_eq!(facts(&native, &query()).0, original);
    native
        .mcp_catalog
        .lock()
        .unwrap()
        .published
        .as_mut()
        .unwrap()
        .checked_at -= MAX_AGE;
    assert_ne!(native.domain_fact_catalog_revision().unwrap(), vector);
    assert_eq!(facts(&native, &query()).1.coverage, d::Coverage::Partial);
    fs::write(dir.path().join(".claude.json"), "{invalid").unwrap();
    native.refresh().unwrap();
    assert_eq!(facts(&native, &query()).0[0].id, original[0].id);
    assert_eq!(facts(&native, &query()).0[0].value, original[0].value);
    assert_eq!(facts(&native, &query()).1.coverage, d::Coverage::Partial);
    assert_ne!(native.domain_fact_catalog_revision().unwrap(), vector);
}
#[test]
fn authorization_and_plugin_epoch_changes_never_reuse_catalog_observations() {
    let dir = tempfile::tempdir().unwrap();
    config(dir.path(), "authorized");
    let mut native = NativeSessions::new(dir.path().into(), &["claude".into()]);
    native.refresh().unwrap();
    native.set_enabled("claude", false).unwrap();
    native.set_enabled("claude", true).unwrap();
    assert!(facts(&native, &query()).1.mcp_catalogs.is_none());
    native.refresh().unwrap();
    assert_eq!(facts(&native, &query()).0.len(), 1);
    native.stop_source_plugin("claude").unwrap();
    assert!(
        native
            .domain_facts_for_source("claude", &query(), None, &[], d::Coverage::Complete)
            .is_err()
    );
    native.source_plugins =
        crate::sessions::source_plugins::NativeSourcePlugins::builtin().unwrap();
    assert!(facts(&native, &query()).1.mcp_catalogs.is_none());
    native.refresh().unwrap();
    assert_eq!(facts(&native, &query()).0.len(), 1);
    let mut scoped = query();
    scoped.session_id = Some("unproven-history".into());
    let (rows, summary) = facts(&native, &scoped);
    assert!(rows.is_empty());
    assert_eq!(summary.mcp_catalogs.unwrap().value(), 0);
}
#[test]
fn catalog_reader_rejects_forged_and_cancelled_invocations_before_broker_use() {
    let dir = tempfile::tempdir().unwrap();
    config(dir.path(), "private");
    let native = NativeSessions::new(dir.path().into(), &["claude".into()]);
    let expected: m::McpCatalogRequest =
        serde_json::from_value(json!({"sourceIds":["claude"],"invocation":{
            "invocationId":"fixture","providerKey":"fixture","implementationIdentity":"fixture",
            "epoch":"1","grantRevision":"1","catalogRevision":"1","resourceRef":"fixture",
            "budget":{"batchItems":40,"maxRecords":"4096","maxDocumentBytes":"6291456"}
        }}))
        .unwrap();
    let checks = Cell::new(0);
    let check = || {
        checks.set(checks.get() + 1);
        Err(NativeError::new("cancelled", "fixture"))
    };
    let mut reader = Reader {
        expected: expected.clone(),
        native: &native,
        sources: native.sources().unwrap(),
        files: vec![],
        projects: BTreeMap::new(),
        check: &check,
        deadline: Instant::now() + READ_BUDGET,
    };
    let mut forged = expected.clone();
    forged.invocation.resource_ref = "foreign".into();
    assert_eq!(
        reader.read_catalog(&forged).unwrap_err().code,
        d::ErrorCode::StaleSnapshot
    );
    assert_eq!(checks.get(), 0);
    assert!(reader.read_catalog(&expected).is_err());
    assert_eq!(checks.get(), 1);
}
#[test]
fn configured_servers_are_bounded_without_inventing_tools_or_execution_counts() {
    let dir = tempfile::tempdir().unwrap();
    let servers = (0..256)
        .map(|n| (format!("server-{n}"), json!({"command":"never-run"})))
        .collect::<serde_json::Map<_, _>>();
    let projects = (0..17)
        .map(|n| {
            (
                dir.path()
                    .join(format!("p{n}"))
                    .to_string_lossy()
                    .into_owned(),
                json!({"mcpServers":servers}),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    fs::write(
        dir.path().join(".claude.json"),
        serde_json::to_vec(&json!({"projects":projects})).unwrap(),
    )
    .unwrap();
    let native = NativeSessions::new(dir.path().into(), &["claude".into()]);
    native.refresh().unwrap();
    let cache = native.mcp_catalog.lock().unwrap();
    let published = cache.published.as_ref().unwrap();
    assert_eq!(published.snapshot.entries.len(), 4096);
    assert!(
        published
            .snapshot
            .partial_sources
            .contains(&"claude".into())
    );
    drop(cache);
    let (rows, summary) = facts(&native, &query());
    assert_eq!(rows.len(), 41);
    assert_eq!(summary.coverage, d::Coverage::Partial);
    assert_eq!(summary.mcp_catalogs.unwrap().value(), 4096);
    assert_eq!(summary.mcp_executions.unwrap().value(), 0);
}

#[test]
fn retaining_an_old_partial_source_never_overflows_the_shared_catalog_budget() {
    let dir = tempfile::tempdir().unwrap();
    let native = NativeSessions::new(dir.path().into(), &["claude".into(), "codex".into()]);
    native.refresh().unwrap();
    let mut cache = native.mcp_catalog.lock().unwrap();
    let old = cache.published.as_mut().unwrap();
    let bindings = old.bindings.clone();
    let entries = |source: &str| {
        (0..4096)
            .map(|n| m::McpConfiguration {
                id: n.to_string(),
                source_id: source.into(),
                name: format!("configuration-{n}"),
                applicable_product_ids: vec![],
                project_ref: None,
                transport: None,
                configured_enabled: None,
            })
            .collect()
    };
    old.snapshot.entries = entries("claude");
    let revision = old.revision.clone();
    cache.publish(
        m::McpCatalogSnapshot {
            entries: entries("codex"),
            partial_sources: vec!["claude".into()],
            observed_at: super::super::super::now(),
            configuration_revision: "fixture-config-version".into(),
        },
        bindings,
    );
    assert!(cache.problem);
    assert_eq!(cache.published.as_ref().unwrap().revision, revision);
    assert_eq!(
        cache.published.as_ref().unwrap().snapshot.entries.len(),
        4096
    );
}

#[test]
fn replacing_a_same_name_configuration_with_preserved_mtime_changes_the_generation() {
    let dir = tempfile::tempdir().unwrap();
    config(dir.path(), "same-name");
    let native = NativeSessions::new(dir.path().into(), &["claude".into()]);
    native.refresh().unwrap();
    let before = facts(&native, &query()).0;
    let vector = native.domain_fact_catalog_revision().unwrap();
    let path = dir.path().join(".claude.json");
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let replacement = dir.path().join("replacement-config");
    fs::write(
        &replacement,
        serde_json::to_vec(&json!({"mcpServers":{"same-name":{"command":"different-never-run"}}}))
            .unwrap(),
    )
    .unwrap();
    fs::File::options()
        .write(true)
        .open(&replacement)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(modified))
        .unwrap();
    fs::rename(replacement, &path).unwrap();
    native.refresh().unwrap();
    assert_eq!(facts(&native, &query()).0[0].value, before[0].value);
    assert_eq!(facts(&native, &query()).0[0].id, before[0].id);
    assert_ne!(native.domain_fact_catalog_revision().unwrap(), vector);
    let snapshot = serde_json::to_string(
        &native
            .mcp_catalog
            .lock()
            .unwrap()
            .published
            .as_ref()
            .unwrap()
            .snapshot,
    )
    .unwrap();
    assert!(!snapshot.contains("different-never-run"));
    assert!(!snapshot.contains(dir.path().to_str().unwrap()));
}

#[test]
fn a_failed_catalog_cache_does_not_disable_other_native_fact_consumers() {
    let dir = tempfile::tempdir().unwrap();
    config(dir.path(), "healthy-before-failure");
    let file = dir.path().join(".claude/projects/fixture/history.jsonl");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file,format!("{}\n",json!({"type":"user","sessionId":"preserved","uuid":"u","timestamp":super::super::super::now(),"message":{"content":"managed fixture"}}))).unwrap();
    let native = NativeSessions::new(dir.path().into(), &["claude".into()]);
    native.refresh().unwrap();
    let mut q = query();
    q.kind = FactKind::Session;
    let before = facts(&native, &q).0;
    assert_eq!(before.len(), 1);
    let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = native.mcp_catalog.lock().unwrap();
        panic!("injected catalog cache failure");
    }));
    assert!(caught.is_err());
    assert_eq!(
        native.domain_fact_catalog_revision().unwrap(),
        "unavailable"
    );
    let (after, summary) = facts(&native, &q);
    assert_eq!(after, before);
    assert!(summary.mcp_catalogs.is_none());
    assert_eq!(summary.sessions.records.unwrap().value(), 1);
}

#[test]
fn late_revocation_and_an_expired_read_budget_never_publish_a_complete_catalog() {
    let dir = tempfile::tempdir().unwrap();
    config(dir.path(), "scoped");
    let native = NativeSessions::new(dir.path().into(), &["claude".into()]);
    let expected: m::McpCatalogRequest =
        serde_json::from_value(json!({"sourceIds":["claude"],"invocation":{
            "invocationId":"fixture","providerKey":"fixture","implementationIdentity":"fixture",
            "epoch":"1","grantRevision":"1","catalogRevision":"1","resourceRef":"fixture",
            "budget":{"batchItems":40,"maxRecords":"4096","maxDocumentBytes":"6291456"}
        }}))
        .unwrap();
    let checks = Cell::new(0);
    let check = || {
        checks.set(checks.get() + 1);
        if checks.get() == 2 {
            native.set_enabled("claude", false)?;
            return Err(NativeError::new("unauthorized", "revoked during read"));
        }
        Ok(())
    };
    let sources = native
        .sources()
        .unwrap()
        .into_iter()
        .filter(|s| s.agent_id == "claude")
        .collect::<Vec<_>>();
    let mut reader = Reader {
        expected: expected.clone(),
        native: &native,
        sources: sources.clone(),
        files: vec![],
        projects: BTreeMap::new(),
        check: &check,
        deadline: Instant::now() + READ_BUDGET,
    };
    assert!(reader.read_catalog(&expected).is_err());
    assert_eq!(checks.get(), 2);
    assert!(native.mcp_catalog.lock().unwrap().published.is_none());
    native.set_enabled("claude", true).unwrap();
    let mut reader = Reader {
        expected: expected.clone(),
        native: &native,
        sources,
        files: vec![],
        projects: BTreeMap::new(),
        check: &|| Ok(()),
        deadline: Instant::now() - Duration::from_secs(1),
    };
    let incomplete = reader.read_catalog(&expected).unwrap();
    assert!(incomplete.entries.is_empty());
    assert_eq!(incomplete.partial_sources, vec!["claude"]);
}
