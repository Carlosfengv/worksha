use super::*;
use std::sync::{Arc, Mutex};
pub(super) struct Fixture {
    pub(super) state: AppState,
    _root: tempfile::TempDir,
}
impl Fixture {
    pub(super) async fn new() -> Self {
        use local_agent_core::{
            Store,
            providers::{ProviderManager, vault::MemoryVault},
            sessions::NativeSessions,
        };
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let home = root.path().join("home");
        let data = root.path().join("data");
        let files = home.join(".claude/projects/fixture");
        std::fs::create_dir_all(&files).unwrap();
        let at = chrono::Utc::now().to_rfc3339();
        let rows = [
            serde_json::json!({"type":"user","sessionId":"activity-fixture","uuid":"u1","promptId":"turn-1","entrypoint":"cli","cwd":home,"timestamp":at,"message":{"role":"user","content":"owned activity fixture"}}),
            serde_json::json!({"type":"assistant","sessionId":"activity-fixture","uuid":"a1","timestamp":at,"message":{"id":"r1","model":"fixture","content":[{"type":"text","text":"owned reply"}],"usage":{"input_tokens":10,"output_tokens":3}}}),
        ];
        std::fs::write(
            files.join("fixture.jsonl"),
            rows.iter()
                .map(|r| r.to_string() + "\n")
                .collect::<String>(),
        )
        .unwrap();
        let store = Store::open(&data).unwrap();
        let vault = Arc::new(MemoryVault::default());
        let helper = root.path().join("helper");
        std::fs::write(&helper, "offline").unwrap();
        let providers = ProviderManager::new(home.clone(), &data, helper, vault.clone()).unwrap();
        let (gateway, _listener) = crate::gateway::Gateway::bind(&data).await.unwrap();
        let state = AppState {
            iam: Default::default(),
            workbench: None,
            provider_adapters: crate::provider_models::adapters::builtin(),
            provider_client: crate::state::provider_client().unwrap(),
            credentials: vault.clone(),
            plugins: Arc::new(crate::plugin_host::external::Manager::open(&data, vault).unwrap()),
            readonly: Arc::new(crate::plugin_host::ReadonlyRuntime::builtin().unwrap()),
            store: Arc::new(Mutex::new(store)),
            token: Arc::new("owned-activity-test".into()),
            host: Arc::new("127.0.0.1:9".into()),
            sessions: Arc::new(NativeSessions::new(home.clone(), &["claude".into()])),
            session_work: Arc::new(crate::sessions::work::SessionWork::default()),
            observation: Arc::new(
                crate::observation::Observation::new(&data, "127.0.0.1:9").unwrap(),
            ),
            listeners: Arc::new(Mutex::new(Default::default())),
            providers: Arc::new(Mutex::new(providers)),
            verification_capacity: Arc::new(tokio::sync::Semaphore::new(2)),
            query_capacity: crate::blocking::query_capacity(),
            billing: Arc::new(Mutex::new(crate::billing::Billing::new(home, &data))),
            gateway: Arc::new(gateway),
        };
        state.sessions.refresh().unwrap();
        state
            .observation
            .poll_native_guarded(&state.sessions, &|| true);
        Self { state, _root: root }
    }
}
pub(super) fn input() -> q::UnifiedRequestQuery {
    q::UnifiedRequestQuery {
        scope: d::QueryScope {
            kind: d::ScopeKind::Builtin,
            source_ids: vec!["claude".into()],
        },
        unit: d::requests::RequestUnit::ModelResponse,
        filter: Default::default(),
        descending: true,
        sort: None,
        include_evidence: None,
        snapshot_token: None,
        cursor: None,
    }
}

#[tokio::test]
async fn object_slot_catalog_remains_available_when_the_base_query_pool_is_busy() {
    let fixture = Fixture::new().await;
    let state = fixture.state.clone();
    let _busy = state
        .query_capacity
        .clone()
        .acquire_many_owned(2)
        .await
        .unwrap();
    let app = crate::plugin_host::external::api::routes().with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let response = reqwest::Client::new()
        .post(format!("http://{address}/api/plugins/object-contributions"))
        .json(&serde_json::json!({"kind":"catalog"}))
        .timeout(std::time::Duration::from_secs(3))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body: serde_json::Value = response.json().await.unwrap();
    server.abort();
    assert_eq!(status, reqwest::StatusCode::OK);
    assert_eq!(body["kind"], "catalog");
    assert_eq!(body["catalog"]["schemaVersion"], 1);
}

#[tokio::test]
async fn object_signed_sdk_package_grants_and_reads_all_six_real_metadata_contexts() {
    use d::object_summary as o;
    use local_agent_core::plugin::{external as p, object_contributions as c};
    let fixture = Fixture::new().await;
    let state = &fixture.state;
    let package = fixture._root.path().join("object-package");
    std::fs::create_dir(&package).unwrap();
    for (name, bytes) in [
        (
            "manifest.json",
            include_bytes!(
                "../../../../../sdk/plugins/examples/object-summary/package/manifest.json"
            )
            .as_slice(),
        ),
        (
            "contributions.json",
            include_bytes!(
                "../../../../../sdk/plugins/examples/object-summary/package/contributions.json"
            )
            .as_slice(),
        ),
        (
            "object-contributions.json",
            include_bytes!(
                "../../../../../sdk/plugins/examples/object-summary/package/object-contributions.json"
            )
            .as_slice(),
        ),
    ] {
        std::fs::write(package.join(name), bytes).unwrap();
    }
    let archive = fixture._root.path().join("objects.laplug");
    std::fs::write(
        &archive,
        crate::plugin_host::external::sdk::pack(
            &package,
            Some(&ed25519_dalek::SigningKey::from_bytes(&[19; 32])),
        )
        .unwrap(),
    )
    .unwrap();
    let review = state.plugins.inspect(&archive).unwrap();
    let installed = state
        .plugins
        .install(&p::InstallRequest {
            package_ref: review.package_ref,
            digest: review.digest,
            approve_publisher: true,
        })
        .unwrap();
    let active = state
        .plugins
        .change(p::StateRequest {
            plugin_id: installed.manifest.id.clone(),
            digest: installed.digest.clone(),
            action: p::StateAction::Enable,
        })
        .await
        .unwrap();
    assert_eq!(active.runtime_status, "active");
    assert_eq!(
        state
            .plugins
            .query_view(&p::ViewQuery {
                plugin_id: active.manifest.id.clone(),
                view_id: "session-summary".into(),
                cursor: None
            })
            .unwrap_err()
            .code,
        "object_context_required"
    );
    let native = state.sessions.project_catalog().unwrap().sessions.remove(0);
    let provider = state.store.lock().unwrap().providers().unwrap().remove(0);
    let mut request_query = input();
    request_query.unit = d::requests::RequestUnit::ModelResponse;
    let requests = super::query(state, request_query).unwrap();
    let request = &requests.items[0].record;
    let selectors = [
        (
            "agent-card",
            o::ObjectSelector::Agent {
                id: "claude".into(),
            },
        ),
        (
            "provider-card",
            o::ObjectSelector::Provider { id: provider.id },
        ),
        (
            "account-card",
            o::ObjectSelector::Account {
                id: "native-codex".into(),
            },
        ),
        (
            "project-card",
            o::ObjectSelector::Project {
                id: native.project_id.unwrap(),
                origin: o::ObjectOrigin::Builtin {},
            },
        ),
        (
            "session-card",
            o::ObjectSelector::Session {
                id: native.id,
                origin: o::ObjectOrigin::Builtin {},
            },
        ),
        (
            "request-card",
            o::ObjectSelector::Request {
                id: request.id.clone(),
                unit: request.unit,
                origin: o::ObjectOrigin::Unified {
                    scope: input().scope,
                    source_id: request.source_id.clone(),
                    snapshot_token: requests.snapshot_token,
                },
            },
        ),
        (
            "request-tab",
            o::ObjectSelector::Request {
                id: request.id.clone(),
                unit: request.unit,
                origin: o::ObjectOrigin::Builtin {},
            },
        ),
    ];
    let before = state.sessions.domain_revision().unwrap();
    let app = crate::plugin_host::external::api::routes().with_state(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/api/plugins/object-contributions",
        listener.local_addr().unwrap()
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    assert!(
        !fixture
            ._root
            .path()
            .join("data/object-contributions.sqlite3")
            .exists()
    );
    async fn call(
        client: &reqwest::Client,
        url: &str,
        action: c::ObjectContributionAction,
    ) -> c::ObjectContributionReply {
        let response = client
            .post(url)
            .json(&action)
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body: serde_json::Value = response.json().await.unwrap();
        assert!(status.is_success(), "{body}");
        serde_json::from_value(body).unwrap()
    }
    for (slot_id, object) in selectors {
        let c::ObjectContributionReply::Catalog { catalog } =
            call(&client, &url, c::ObjectContributionAction::Catalog {}).await
        else {
            panic!("catalog");
        };
        let parent = catalog
            .items
            .iter()
            .find(|p| p.plugin_id == active.manifest.id)
            .unwrap();
        let status = parent
            .slots
            .iter()
            .find(|s| s.declaration.id == slot_id)
            .unwrap();
        assert!(!status.authorized);
        let slot = c::ObjectSlotQuery {
            plugin_id: parent.plugin_id.clone(),
            digest: parent.digest.clone(),
            slot_id: slot_id.into(),
        };
        assert_eq!(
            state
                .plugins
                .issue_object_reference(
                    c::ObjectRefRequest {
                        slot: slot.clone(),
                        expected_context_identity: status.context_identity.clone(),
                        object: object.clone(),
                    },
                    &crate::domain::object_summary::Source(state),
                )
                .unwrap_err()
                .code,
            "permission_denied"
        );
        let c::ObjectContributionReply::Status { status: granted } = call(
            &client,
            &url,
            c::ObjectContributionAction::Grant {
                request: c::ObjectGrantRequest {
                    slot: slot.clone(),
                    expected_activation_epoch: parent.activation_epoch.clone(),
                    expected_grant_revision: parent.grant_revision.clone(),
                    expected_descriptor_identity: status.descriptor_identity.clone(),
                    expected_context_identity: status.context_identity.clone(),
                },
            },
        )
        .await
        else {
            panic!("grant");
        };
        let status = granted
            .slots
            .iter()
            .find(|s| s.declaration.id == slot_id)
            .unwrap();
        let c::ObjectContributionReply::Reference { reference } = call(
            &client,
            &url,
            c::ObjectContributionAction::Issue {
                request: c::ObjectRefRequest {
                    slot: slot.clone(),
                    expected_context_identity: status.context_identity.clone(),
                    object: object.clone(),
                },
            },
        )
        .await
        else {
            panic!("reference");
        };
        if slot_id == "session-card" {
            state.sessions.refresh().unwrap();
        }
        let c::ObjectContributionReply::Summary {
            summary,
            object_ref,
        } = call(
            &client,
            &url,
            c::ObjectContributionAction::Read {
                query: c::ObjectRefQuery {
                    slot: slot.clone(),
                    object_ref: reference.object_ref.clone(),
                },
            },
        )
        .await
        else {
            panic!("summary");
        };
        assert_eq!(object_ref, reference.object_ref);
        assert_eq!(summary.target, object.target());
        assert_eq!(
            summary.metrics.iter().map(|m| m.field).collect::<Vec<_>>(),
            status.declaration.fields
        );
        summary.validate().unwrap();
        call(
            &client,
            &url,
            c::ObjectContributionAction::Release {
                query: c::ObjectRefQuery { slot, object_ref },
            },
        )
        .await;
    }
    let reader = crate::domain::object_summary::Source(state);
    let session = o::ObjectSelector::Session {
        id: state.sessions.project_catalog().unwrap().sessions[0]
            .id
            .clone(),
        origin: o::ObjectOrigin::Builtin {},
    };
    let context = |id: &str| {
        let parent = state
            .plugins
            .object_contribution_catalog()
            .unwrap()
            .items
            .remove(0);
        let status = parent
            .slots
            .iter()
            .find(|slot| slot.declaration.id == id)
            .unwrap()
            .clone();
        let request = c::ObjectGrantRequest {
            slot: c::ObjectSlotQuery {
                plugin_id: parent.plugin_id,
                digest: parent.digest,
                slot_id: id.into(),
            },
            expected_activation_epoch: parent.activation_epoch,
            expected_grant_revision: parent.grant_revision,
            expected_descriptor_identity: status.descriptor_identity,
            expected_context_identity: status.context_identity,
        };
        (request, status.error_code)
    };
    let issue = |id: &str| {
        let (request, _) = context(id);
        let reference = state.plugins.issue_object_reference(
            c::ObjectRefRequest {
                slot: request.slot.clone(),
                expected_context_identity: request.expected_context_identity,
                object: session.clone(),
            },
            &reader,
        )?;
        Ok::<_, crate::plugin_host::external::Error>(c::ObjectRefQuery {
            slot: request.slot,
            object_ref: reference.object_ref,
        })
    };
    let base = issue("session-card").unwrap();
    let (grant, error) = context("session-enhanced");
    assert_eq!(error.as_deref(), Some("dependency_missing"));
    state
        .plugins
        .object_contribution_grant(grant, true)
        .unwrap();
    assert_eq!(
        issue("session-enhanced").unwrap_err().code,
        "dependency_missing"
    );
    let (stale_confirmation, _) = context("session-enhanced");
    let helper = fixture._root.path().join("helper-package");
    std::fs::create_dir(&helper).unwrap();
    std::fs::write(
        helper.join("manifest.json"),
        include_bytes!("../../../../../sdk/plugins/examples/detail-helper/package/manifest.json"),
    )
    .unwrap();
    let archive = fixture._root.path().join("helper.laplug");
    std::fs::write(
        &archive,
        crate::plugin_host::external::sdk::pack(
            &helper,
            Some(&ed25519_dalek::SigningKey::from_bytes(&[19; 32])),
        )
        .unwrap(),
    )
    .unwrap();
    let review = state.plugins.inspect(&archive).unwrap();
    let helper = state
        .plugins
        .install(&p::InstallRequest {
            package_ref: review.package_ref,
            digest: review.digest,
            approve_publisher: false,
        })
        .unwrap();
    let helper_state = |action| p::StateRequest {
        plugin_id: helper.manifest.id.clone(),
        digest: helper.digest.clone(),
        action,
    };
    state
        .plugins
        .change(helper_state(p::StateAction::Enable))
        .await
        .unwrap();
    assert_eq!(
        state
            .plugins
            .object_contribution_grant(stale_confirmation, true)
            .unwrap_err()
            .code,
        "stale_snapshot"
    );
    let enhanced = issue("session-enhanced").unwrap();
    state
        .plugins
        .read_object_reference(enhanced.clone(), &reader)
        .unwrap();
    state
        .plugins
        .change(helper_state(p::StateAction::Disable))
        .await
        .unwrap();
    state
        .plugins
        .read_object_reference(base.clone(), &reader)
        .unwrap();
    assert_eq!(
        state
            .plugins
            .read_object_reference(enhanced.clone(), &reader)
            .unwrap_err()
            .code,
        "dependency_missing"
    );
    state
        .plugins
        .change(helper_state(p::StateAction::Enable))
        .await
        .unwrap();
    state
        .plugins
        .read_object_reference(base.clone(), &reader)
        .unwrap();
    assert!(
        state
            .plugins
            .read_object_reference(enhanced, &reader)
            .is_err()
    );
    let enhanced = issue("session-enhanced").unwrap();
    let (revoke, _) = context("session-enhanced");
    state
        .plugins
        .object_contribution_grant(revoke, false)
        .unwrap();
    let (grant, _) = context("session-enhanced");
    state
        .plugins
        .object_contribution_grant(grant, true)
        .unwrap();
    assert!(
        state
            .plugins
            .read_object_reference(enhanced, &reader)
            .is_err()
    );
    let current = issue("session-enhanced").unwrap();
    state
        .plugins
        .read_object_reference(current, &reader)
        .unwrap();
    assert_eq!(before, state.sessions.domain_revision().unwrap());
    state.sessions.set_enabled("claude", false).unwrap();
    assert!(
        state
            .plugins
            .read_object_reference(base.clone(), &reader)
            .is_err()
    );
    state.sessions.set_enabled("claude", true).unwrap();
    assert!(state.plugins.read_object_reference(base, &reader).is_err());
    server.abort();
    assert_ne!(before, state.sessions.domain_revision().unwrap());
    assert!(!fixture._root.path().join("data/billing").exists());
    assert!(
        !fixture
            ._root
            .path()
            .join("home/.claude/settings.json")
            .exists()
    );
}

#[tokio::test]
async fn object_readers_keep_frozen_session_project_and_request_filters_without_collecting() {
    use crate::domain::{facts, object_summary::Source, query as sessions};
    use d::object_summary::{ObjectOrigin, ObjectSelector, ObjectSummarySource, SummaryField as F};
    let fixture = Fixture::new().await;
    let state = &fixture.state;
    let scope = input().scope;
    let page = sessions::sessions(
        state,
        d::UnifiedSessionQuery {
            scope: scope.clone(),
            filter: Default::default(),
            snapshot_token: None,
            cursor: None,
        },
    )
    .unwrap();
    let session = &page.items[0];
    let selector = ObjectSelector::Session {
        id: session.record.id.clone(),
        origin: ObjectOrigin::Unified {
            scope: scope.clone(),
            source_id: session.source_id.clone(),
            snapshot_token: page.snapshot_token.clone(),
        },
    };
    let source = Source(state);
    let index_before = state.sessions.domain_revision().unwrap();
    let read = source
        .read_object_summary(&selector, &[F::InputTokens, F::TotalTokens])
        .unwrap();
    source.ensure_object_summary_current(&read).unwrap();
    assert_eq!(
        read.summary.metrics[0].value,
        Some(d::object_summary::SummaryValue::Tokens {
            value: 10_u64.into()
        })
    );
    let excluded = sessions::sessions(
        state,
        d::UnifiedSessionQuery {
            scope: scope.clone(),
            filter: d::SessionFilter {
                query: Some("cannot-match-owned-fixture".into()),
                ..Default::default()
            },
            snapshot_token: None,
            cursor: None,
        },
    )
    .unwrap();
    let mut foreign = selector.clone();
    if let ObjectSelector::Session {
        origin: ObjectOrigin::Unified { snapshot_token, .. },
        ..
    } = &mut foreign
    {
        *snapshot_token = excluded.snapshot_token;
    }
    assert!(
        source
            .read_object_summary(&foreign, &[F::TotalTokens])
            .is_err()
    );
    let projects = facts::facts(
        state,
        d::facts::query::FactQuery {
            scope: scope.clone(),
            kind: d::facts::FactKind::Project,
            project_id: None,
            session_id: None,
            response_id: None,
            snapshot_token: None,
            cursor: None,
        },
    )
    .unwrap();
    let project = ObjectSelector::Project {
        id: projects.items[0].id.clone(),
        origin: ObjectOrigin::Unified {
            scope: scope.clone(),
            source_id: projects.items[0].identity.source_id.clone(),
            snapshot_token: projects.snapshot_token,
        },
    };
    source
        .read_object_summary(&project, &[F::SessionCount])
        .unwrap();
    let mut query = input();
    query.unit = d::requests::RequestUnit::ModelResponse;
    let requests = super::query(state, query).unwrap();
    let request = &requests.items[0].record;
    let selector = ObjectSelector::Request {
        id: request.id.clone(),
        unit: request.unit,
        origin: ObjectOrigin::Unified {
            scope,
            source_id: request.source_id.clone(),
            snapshot_token: requests.snapshot_token,
        },
    };
    let read = source
        .read_object_summary(&selector, &[F::TotalTokens, F::DurationMs, F::AttemptCount])
        .unwrap();
    source.ensure_object_summary_current(&read).unwrap();
    assert_eq!(index_before, state.sessions.domain_revision().unwrap());
    state.sessions.set_enabled("claude", false).unwrap();
    assert!(source.ensure_object_summary_current(&read).is_err());
}
#[tokio::test]
async fn native_requests_use_published_calls_without_body_reads_and_fence_revocation() {
    let f = Fixture::new().await;
    let page = query(&f.state, input()).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].evidence, q::RequestEvidence::HostObserved);
    assert!(page.items[0].host_request_id.is_some());
    let json = serde_json::to_string(&page).unwrap();
    assert!(!json.contains("owned reply"));
    assert!(!json.contains(f.state.sessions.home().to_str().unwrap()));
    let mut frozen = input();
    frozen.snapshot_token = Some(page.snapshot_token.clone());
    assert_eq!(query(&f.state, frozen.clone()).unwrap().items, page.items);
    f.state.sessions.set_enabled("claude", false).unwrap();
    assert_eq!(query(&f.state, frozen).unwrap_err().code, "stale_snapshot");
    let fresh = query(&f.state, input()).unwrap();
    assert!(fresh.items.is_empty());
    assert!(fresh.source_summaries[0].records.is_none());
}
#[tokio::test]
async fn stopped_source_and_inflight_revocation_reject_old_request_publications() {
    let f = Fixture::new().await;
    assert_eq!(
        query_with(&f.state, input(), &|| {
            f.state.sessions.stop_source_plugin("claude").unwrap();
        })
        .unwrap_err()
        .code,
        "stale_snapshot"
    );
    let fresh = query(&f.state, input()).unwrap();
    assert!(fresh.items.is_empty());
    assert!(fresh.source_summaries[0].records.is_none());
    let f = Fixture::new().await;
    assert_eq!(
        query_with(&f.state, input(), &|| {
            f.state.sessions.set_enabled("claude", false).unwrap();
            f.state.sessions.set_enabled("claude", true).unwrap();
        })
        .unwrap_err()
        .code,
        "stale_snapshot"
    );
}

#[tokio::test]
async fn a_gateway_freeze_cannot_outlive_native_enrichment_reader_replacement() {
    let mut f = Fixture::new().await;
    let store = local_agent_core::observation::ObservationStore::open(
        &f._root.path().join("data/observation"),
    )
    .unwrap();
    let observed = serde_json::from_value(serde_json::json!({"id":"gateway-evidence","agentId":"claude","agentProductId":"claude-code-cli","sessionId":null,"responseId":"r1","model":"fixture","provider":"Fixture","providerId":"fixture","endpoint":"https://openrouter.ai/api/v1","bindingRevision":1,"startedAt":"2026-10-01T00:00:00Z","endedAt":"2026-10-01T00:00:01Z","status":"success","httpStatus":200,"ttftMs":100.0,"durationMs":1000.0,"usage":{"inputTokens":10,"outputTokens":3,"totalTokens":13},"reportedCost":{"amount":0,"currency":"USD","source":"openrouter.usage.cost"},"protocol":"messages","revision":1})).unwrap();
    store.save(&observed).unwrap();
    let mut gateway = input();
    gateway.unit = d::requests::RequestUnit::LogicalRequest;
    gateway.include_evidence = Some(true);
    let before = query(&f.state, gateway.clone()).unwrap();
    assert_eq!(before.items.len(), 1);
    assert!(
        matches!(&before.items[0].evidence_detail.as_ref().unwrap().matching,q::RequestMatching::GatewayRequest{matched_responses,..} if matched_responses.value()==1)
    );
    let old = f.state.sessions.clone();
    old.stop_source_plugins().unwrap();
    let replacement =
        local_agent_core::sessions::NativeSessions::new(old.home().into(), &["claude".into()]);
    replacement.refresh().unwrap();
    f.state.sessions = Arc::new(replacement);
    gateway.snapshot_token = Some(before.snapshot_token);
    assert_eq!(query(&f.state, gateway).unwrap_err().code, "stale_snapshot");
    let mut fresh = input();
    fresh.unit = d::requests::RequestUnit::LogicalRequest;
    assert_eq!(
        query(&f.state, fresh).unwrap().source_summaries[0]
            .records
            .as_ref()
            .unwrap()
            .value(),
        1
    );
}
#[tokio::test]
async fn gateway_request_unit_does_not_implicitly_enable_native_sources() {
    let f = Fixture::new().await;
    f.state.sessions.set_enabled("claude", false).unwrap();
    let mut q = input();
    q.unit = d::requests::RequestUnit::LogicalRequest;
    let page = query(&f.state, q).unwrap();
    assert!(page.items.is_empty());
    assert_eq!(
        page.source_summaries[0].records.as_ref().unwrap().value(),
        0
    );
    assert!(
        !f.state
            .sessions
            .sources()
            .unwrap()
            .iter()
            .find(|s| s.agent_id == "claude")
            .unwrap()
            .enabled
    );
}
#[tokio::test]
async fn corrupt_gateway_history_warns_without_hiding_native_responses_or_rewriting_sources() {
    let f = Fixture::new().await;
    let store =
        rusqlite::Connection::open(f._root.path().join("data/observation/observations.sqlite3"))
            .unwrap();
    store
        .execute(
            "INSERT INTO model_observations VALUES('bad','2026-10-01T00:00:00Z',1,'{bad')",
            [],
        )
        .unwrap();
    let mut gateway = input();
    gateway.unit = d::requests::RequestUnit::LogicalRequest;
    let partial = query(&f.state, gateway.clone()).unwrap();
    assert_eq!(partial.source_summaries[0].coverage, d::Coverage::Partial);
    assert_eq!(
        partial.source_states[0].problem.as_deref(),
        Some("partial_history")
    );
    let native = query(&f.state, input()).unwrap();
    assert_eq!(native.items.len(), 1);
    assert_eq!(native.source_summaries[0].coverage, d::Coverage::Complete);
    assert_eq!(
        store
            .query_row(
                "SELECT value FROM model_observations WHERE id='bad'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "{bad"
    );
    store
        .execute("DELETE FROM model_observations WHERE id='bad'", [])
        .unwrap();
    assert_eq!(
        query(&f.state, gateway.clone()).unwrap().source_summaries[0].coverage,
        d::Coverage::Complete
    );
    gateway.snapshot_token = Some(partial.snapshot_token);
    assert_eq!(
        query(&f.state, gateway).unwrap().source_summaries[0].coverage,
        d::Coverage::Partial,
        "a later repair cannot upgrade a frozen partial publication"
    );
}
#[test]
fn reported_response_filters_use_precise_half_open_dates_and_keep_unknown_time_separate() {
    let query = input();
    let now = chrono::Utc::now();
    let row = q::UnifiedRequestItem {
        evidence_detail: None,
        record: q::RequestQueryRecord {
            id: "external-response".into(),
            source_id: "source".into(),
            unit: query.unit,
            product_id: None,
            session_ref: Some("session".into()),
            session_title: Some("Needle".into()),
            project_ref: None,
            project_name: None,
            occurred_at: None,
            ended_at: None,
            model: None,
            provider_key: "source-provider".into(),
            provider_name: "Source".into(),
            outcome: "unknown".into(),
            usage: (&local_agent_core::sessions::Usage::default()).into(),
            usage_missing: 1u64.into(),
            attempts: None,
            duration_ms: None,
            ttft_ms: None,
            throughput: None,
        },
        evidence: q::RequestEvidence::SourceReported,
        host_request_id: None,
        reported_response_id: Some("claimed".into()),
        reported_duration_ms: None,
    };
    assert!(matches(&query, &row, now, true));
    let mut dated = query.clone();
    dated.filter.start = Some("2026-10-01T00:00:00.000001Z".into());
    dated.filter.end = Some("2026-10-01T00:00:00.000003Z".into());
    assert!(!matches(&dated, &row, now, true));
    assert!(matches(&dated, &row, now, false));
    for (at, expected) in [
        ("2026-10-01T00:00:00.000000Z", false),
        ("2026-10-01T00:00:00.000001Z", true),
        ("2026-10-01T00:00:00.000002Z", true),
        ("2026-10-01T00:00:00.000003Z", false),
    ] {
        let mut row = row.clone();
        row.record.occurred_at = Some(at.into());
        assert_eq!(matches(&dated, &row, now, true), expected);
    }
    dated.filter.search = Some("needle".into());
    assert!(matches(&dated, &row, now, false));
    dated.filter.product_id = Some("unrelated".into());
    assert!(!matches(&dated, &row, now, false));
    assert!(serde_json::from_value::<q::UnifiedRequestQuery>(serde_json::json!({"scope":query.scope,"unit":query.unit,"filter":query.filter,"descending":true,"snapshotToken":null,"cursor":null,"allowGuest":true})).is_err());
}
