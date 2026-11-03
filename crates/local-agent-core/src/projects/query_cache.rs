//! Bounded numeric project pages; live directory and listener state remain fresh.
use super::*;
use std::collections::VecDeque;
use std::io::Write;

const MAX_ENTRIES: usize = 8;
const MAX_PAGE_BYTES: usize = 1024 * 1024;

struct Fingerprint(Sha256);
impl Write for Fingerprint {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn membership(
    catalog: &ProjectCatalog,
    query: &ProjectQuery,
    now: DateTime<Utc>,
) -> (Vec<bool>, Vec<bool>) {
    (
        catalog
            .sessions
            .iter()
            .map(|s| in_range_at(s.created_at.as_deref(), query.days, now))
            .collect(),
        catalog
            .writes
            .iter()
            .map(|w| in_range_at(w.occurred_at.as_deref(), query.days, now))
            .collect(),
    )
}

#[derive(Default)]
pub(super) struct QueryCache {
    pages: VecDeque<CachedPage>,
}
struct CachedPage {
    snapshot: (u64, crate::sessions::CallSnapshotIdentity),
    key: String,
    page: ProjectPage,
}

impl QueryCache {
    pub(super) fn query(
        &mut self,
        catalog: &ProjectCatalog,
        query: ProjectQuery,
        listeners: &listeners::ListenerSnapshot,
        build: impl FnOnce(
            &ProjectCatalog,
            ProjectQuery,
            &listeners::ListenerSnapshot,
        ) -> NativeResult<ProjectPage>,
    ) -> NativeResult<ProjectPage> {
        validate_with_directory(&query, catalog.calls.directory())?;
        let snapshot = (catalog.revision, catalog.calls.snapshot_identity());
        self.pages.retain(|entry| entry.snapshot.1.is_live());
        // Session/write metadata and filesystem identities are small relative to calls.
        // Membership booleans also catch future timestamps and UTC+8 midnight changes.
        let identities: Vec<_> = catalog
            .sessions
            .iter()
            .filter_map(session_identity)
            .collect();
        let selected_membership = membership(catalog, &query, Utc::now());
        // Stream metadata into the digest: a write-heavy history must not allocate
        // another JSON tree or a second copy of its file paths for every reader.
        let mut fingerprint = Fingerprint(Sha256::new());
        serde_json::to_writer(
            &mut fingerprint,
            &(
                &query,
                &catalog.sessions,
                &catalog.writes,
                &identities,
                &selected_membership,
            ),
        )
        .map_err(|_| NativeError::new("unavailable", "项目统计暂不可用"))?;
        let key = format!("{:x}", fingerprint.0.finalize());
        if let Some(entry) = self
            .pages
            .iter()
            .find(|entry| entry.snapshot == snapshot && entry.key == key)
        {
            let mut page = entry.page.clone();
            page.as_of = catalog.as_of.clone();
            page.coverage = catalog.coverage.clone();
            page.listeners = listeners.clone();
            page.listeners.items.clear();
            for project in &mut page.items {
                let matched = listeners.for_project(&project.identity, &identities);
                project.listener_count = matched.count();
                project.known_listener_count = matched.items.len();
            }
            return Ok(page);
        }
        let page = build(catalog, query.clone(), listeners)?;
        // selected_session_ids are skipped by Serialize but are retained for fee linkage.
        let private_bytes = page
            .selected_session_ids
            .iter()
            .chain(page.items.iter().flat_map(|p| &p.selected_session_ids))
            .map(|s| s.len())
            .sum::<usize>();
        if selected_membership == membership(catalog, &query, Utc::now())
            && serde_json::to_vec(&page)
                .is_ok_and(|bytes| bytes.len().saturating_add(private_bytes) <= MAX_PAGE_BYTES)
        {
            while self.pages.len() >= MAX_ENTRIES {
                self.pages.pop_front();
            }
            self.pages.push_back(CachedPage {
                snapshot,
                key,
                page: page.clone(),
            });
        }
        Ok(page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, to_value};

    fn compare(
        cache: &mut QueryCache,
        catalog: &ProjectCatalog,
        query: ProjectQuery,
        snapshot: &listeners::ListenerSnapshot,
    ) -> Value {
        let actual = cache
            .query(catalog, query.clone(), snapshot, super::super::query)
            .unwrap();
        let expected = super::super::query(catalog, query, snapshot).unwrap();
        assert_eq!(actual.selected_session_ids, expected.selected_session_ids);
        for (a, b) in actual.items.iter().zip(&expected.items) {
            assert_eq!(a.selected_session_ids, b.selected_session_ids);
        }
        assert_eq!(to_value(&actual).unwrap(), to_value(expected).unwrap());
        to_value(actual).unwrap()
    }

    #[test]
    fn cached_pages_preserve_client_filters_fee_ids_and_live_listeners() {
        let mut catalog = crate::associated_cost_tests::catalog();
        let mut cache = QueryCache::default();
        let snapshot = listeners::ListenerSnapshot::default();
        for agent in [
            None,
            Some("claude"),
            Some("claude-code-cli"),
            Some("claude-code-desktop"),
            Some("claude-unknown"),
        ] {
            let query = ProjectQuery {
                agent: agent.map(str::to_owned),
                scope: Some("all".into()),
                ..Default::default()
            };
            compare(&mut cache, &catalog, query.clone(), &snapshot);
            let hit = cache
                .query(&catalog, query, &snapshot, |_, _, _| {
                    panic!("unchanged history was rebuilt")
                })
                .unwrap();
            assert_eq!(hit.applied_filter.scope.as_deref(), Some("all"));
        }
        let query = ProjectQuery::default();
        compare(&mut cache, &catalog, query.clone(), &snapshot);
        catalog.coverage = "partial".into();
        let progress = cache
            .query(&catalog, query.clone(), &snapshot, |_, _, _| {
                panic!("scan progress rebuilt project totals")
            })
            .unwrap();
        assert_eq!(progress.coverage, "partial");
        catalog.coverage = "native_history".into();
        let mut live = listeners::ListenerSnapshot {
            state: "ready".into(),
            as_of: Some(now()),
            ..Default::default()
        };
        let path = session_identity(&catalog.sessions[0]).unwrap().path;
        live.items.push(listeners::Listener {
            id: "port".into(),
            protocol: "tcp".into(),
            address: "127.0.0.1".into(),
            port: 12345,
            pid: 1,
            process_identity: "fixture".into(),
            started_at: now(),
            command_redacted: "fixture".into(),
            cwd: path,
            attribution: "cwd".into(),
            evidence: "fixture".into(),
        });
        catalog.as_of = "fresh-snapshot".into();
        let hit = cache
            .query(&catalog, query.clone(), &live, |_, _, _| {
                panic!("listener update rescanned calls")
            })
            .unwrap();
        assert_eq!(hit.items[0].listener_count, Some(1));
        assert_eq!(hit.as_of, "fresh-snapshot");
        compare(&mut cache, &catalog, query, &live);
    }

    #[test]
    fn cache_invalidates_mutations_visibility_revocation_metadata_and_filesystem() {
        let mut catalog = crate::associated_cost_tests::catalog();
        let mut cache = QueryCache::default();
        let query = ProjectQuery::default();
        let snapshot = Default::default();
        let before = compare(&mut cache, &catalog, query.clone(), &snapshot);
        catalog.calls[0].usage.as_mut().unwrap().total_tokens = Some(777);
        let changed = compare(&mut cache, &catalog, query.clone(), &snapshot);
        assert_ne!(before["summary"]["usage"], changed["summary"]["usage"]);
        catalog.sessions[0].disk_bytes += 123;
        compare(&mut cache, &catalog, query.clone(), &snapshot);
        catalog.writes.push(writes::FileWrite {
            id: "write-1".into(),
            project_id: session_identity(&catalog.sessions[0]).unwrap().id,
            session_id: catalog.sessions[0].id.clone(),
            agent_id: "claude".into(),
            agent_product_id: Some("claude-code-cli".into()),
            turn_id: "turn".into(),
            call_id: "tool".into(),
            tool: "Write".into(),
            occurred_at: Some(now()),
            source_line: 1,
            evidence: "structured".into(),
            files: vec![writes::WriteFile {
                path: "/fixture/file".into(),
                operation: "write".into(),
                added: Some(1),
                removed: Some(0),
            }],
        });
        assert_eq!(
            compare(&mut cache, &catalog, query.clone(), &snapshot)["summary"]["writeCount"],
            1
        );
        catalog.writes[0].files[0].path = "/fixture/changed".into();
        compare(&mut cache, &catalog, query.clone(), &snapshot);
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("new-project");
        for session in &mut catalog.sessions {
            session.project_root = Some(root.to_string_lossy().into());
        }
        let absent = compare(&mut cache, &catalog, query.clone(), &snapshot);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        let present = compare(&mut cache, &catalog, query.clone(), &snapshot);
        assert_ne!(
            absent["items"][0]["locationState"],
            present["items"][0]["locationState"]
        );
        let hidden = session_identity(&catalog.sessions[0]).unwrap().into();
        let invisible = visibility::visible_catalog(catalog.clone(), &[hidden]);
        assert_eq!(
            compare(&mut cache, &invisible, query.clone(), &snapshot)["total"],
            0
        );
        compare(&mut cache, &catalog, query.clone(), &snapshot);
        catalog.calls.clear();
        catalog.sessions.clear();
        catalog.writes.clear();
        catalog.revision += 1;
        assert_eq!(
            compare(&mut cache, &catalog, query, &snapshot)["summary"]["modelCalls"],
            0
        );
    }

    #[test]
    fn alternating_published_snapshots_reuse_their_exact_pages_without_rebuilding_history() {
        let mut first = crate::associated_cost_tests::catalog();
        let mut next = first.clone();
        next.calls[0].usage.as_mut().unwrap().total_tokens = Some(777);
        next.revision += 1;
        let mut cache = QueryCache::default();
        let query = ProjectQuery {
            scope: Some("all".into()),
            ..Default::default()
        };
        let listeners = Default::default();
        let old = compare(&mut cache, &first, query.clone(), &listeners);
        let current = compare(&mut cache, &next, query.clone(), &listeners);
        assert_ne!(old["summary"]["usage"], current["summary"]["usage"]);
        // A queued old catalog must not evict the newly published generation.
        // Neither generation keeps another strong reference to its file batches.
        for _ in 0..8 {
            for (catalog, expected) in [(&first, &old), (&next, &current)] {
                let page = cache
                    .query(catalog, query.clone(), &listeners, |_, _, _| {
                        panic!("a captured snapshot rebuilt history")
                    })
                    .unwrap();
                assert_eq!(to_value(page).unwrap(), *expected);
            }
        }
        // Metadata is still part of the fingerprint even with identical batches.
        first.sessions[0].disk_bytes += 123;
        compare(&mut cache, &first, query.clone(), &listeners);
        // A matching revision alone is insufficient for a different call snapshot.
        next.revision = first.revision;
        compare(&mut cache, &next, query.clone(), &listeners);
        let weak = std::sync::Arc::downgrade(&next.calls.chunks[0]);
        drop(next);
        assert!(weak.upgrade().is_none());
        compare(&mut cache, &first, query, &listeners);
        assert!(cache.pages.iter().all(|entry| entry.snapshot.1.is_live()));
        assert!(cache.pages.len() <= MAX_ENTRIES);
    }

    #[test]
    fn cache_is_bounded_and_does_not_retain_old_call_batches() {
        let mut catalog = crate::associated_cost_tests::catalog();
        let weak = std::sync::Arc::downgrade(&catalog.calls.chunks[0]);
        let mut cache = QueryCache::default();
        for index in 0..12 {
            compare(
                &mut cache,
                &catalog,
                ProjectQuery {
                    query: Some(format!("search-{index}")),
                    ..Default::default()
                },
                &Default::default(),
            );
        }
        assert_eq!(cache.pages.len(), MAX_ENTRIES);
        catalog.calls.clear();
        assert!(weak.upgrade().is_none());
        compare(
            &mut cache,
            &catalog,
            Default::default(),
            &Default::default(),
        );
        assert_eq!(cache.pages.len(), 1);
    }

    #[test]
    fn query_membership_changes_at_future_and_utc_eight_midnight_boundaries() {
        let mut catalog = crate::associated_cost_tests::catalog();
        catalog.sessions[0].created_at = Some("2026-10-24T15:59:59.001Z".into());
        catalog.sessions[1].created_at = Some("2026-10-24T01:00:00Z".into());
        let at = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);
        let query = ProjectQuery {
            days: Some(1),
            ..Default::default()
        };
        assert_eq!(
            membership(&catalog, &query, at("2026-10-24T15:59:59Z")).0,
            [false, true]
        );
        assert_eq!(
            membership(&catalog, &query, at("2026-10-24T15:59:59.001Z")).0,
            [true, true]
        );
        assert_eq!(
            membership(&catalog, &query, at("2026-10-24T16:00:00Z")).0,
            [false, false]
        );
        assert_eq!(
            membership(
                &catalog,
                &ProjectQuery::default(),
                at("2026-10-24T16:00:00Z")
            )
            .0,
            [true, true]
        );
    }
}
