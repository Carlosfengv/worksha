use super::*;
use local_agent_core::providers::upstream_accounts::{AccountEdit, AccountRef};
use std::sync::atomic::AtomicBool;

#[tokio::test]
async fn quota_reset_serializes_gateway_recovery_across_strategies_and_pinned_turns() {
    let (listener, upstream) = super::minimax::tls_upstream(
        "openrouter.ai",
        include_bytes!("fixtures/quota-fixture-cert.der"),
    )
    .await;
    let vault = Arc::new(MemoryVault::default());
    let mut fixture = Fixture::with_upstream(vault.clone(), Some(upstream)).await;
    let sends = Arc::new(AtomicUsize::new(0));
    let backup_sends = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    let resume = Arc::new(tokio::sync::Notify::new());
    let pause = Arc::new(AtomicBool::new(false));
    let limited = Arc::new(AtomicBool::new(false));
    let route = axum::Router::new().route("/api/v1/chat/completions", axum::routing::post({
        let (sends, backup_sends, entered, resume, pause, limited) =
            (sends.clone(), backup_sends.clone(), entered.clone(), resume.clone(), pause.clone(), limited.clone());
        move |headers: axum::http::HeaderMap, axum::Json(body): axum::Json<serde_json::Value>| {
            let (sends, backup_sends, entered, resume, pause, limited) =
                (sends.clone(), backup_sends.clone(), entered.clone(), resume.clone(), pause.clone(), limited.clone());
            async move {
                let primary = match headers["authorization"].to_str().unwrap() {
                    "Bearer quota-upstream-fixture" | "Bearer quota-sibling-fixture" => true,
                    "Bearer quota-backup-fixture" => false,
                    _ => panic!("unexpected fixture credential"),
                };
                if primary { &sends } else { &backup_sends }.fetch_add(1, Ordering::AcqRel);
                if primary && pause.swap(false, Ordering::AcqRel) {
                    entered.notify_one();
                    tokio::time::timeout(Duration::from_secs(10), resume.notified()).await.unwrap();
                }
                if primary && limited.load(Ordering::Acquire) {
                    return (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "60")], axum::Json(
                        json!({"error":{"message":"fixture rate limit","type":"rate_limit_error"}})
                    )).into_response();
                }
                axum::Json(json!({
                    "id":"quota_fixture","object":"chat.completion","model":body["model"],
                    "choices":[{"index":0,"message":{"role":"assistant","content":"OK"},"finish_reason":"stop"}],
                    "usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}
                })).into_response()
            }
        }
    }));
    fixture.tasks.push(tokio::spawn(async move {
        axum::serve(listener, route).await.unwrap()
    }));
    let local_key = uuid::Uuid::new_v4().to_string();
    let upstream_key = uuid::Uuid::new_v4().to_string();
    let backup_key = uuid::Uuid::new_v4().to_string();
    let sibling_key = uuid::Uuid::new_v4().to_string();
    vault.set(&local_key, b"quota-local-fixture").unwrap();
    vault.set(&upstream_key, b"quota-upstream-fixture").unwrap();
    vault.set(&backup_key, b"quota-backup-fixture").unwrap();
    vault.set(&sibling_key, b"quota-sibling-fixture").unwrap();
    let (
        a,
        b,
        with_backup,
        weighted,
        capacity,
        permission,
        permission_off,
        paced,
        reverse_paced,
        fixed_draft,
        default_connection,
        default_draft,
        store_id,
        target,
        account,
        sibling,
        backup,
    ) = {
        let store = fixture.state.store.lock().unwrap();
        let mut provider = store.provider_by_id("openrouter").unwrap().unwrap();
        provider.model = "quota-fixture".into();
        provider.models = vec![provider.model.clone()];
        provider.credential_ref = Some(upstream_key.clone());
        store.save_provider(provider.clone()).unwrap();
        let provider = store.provider_by_id("openrouter").unwrap().unwrap();
        let save_account = |credential_ref, label: &str| {
            store
                .save_upstream_account(
                    &provider,
                    &AccountEdit {
                        id: uuid::Uuid::new_v4().to_string(),
                        expected_revision: 0,
                        provider_revision: provider.revision,
                        label: label.into(),
                        credential_ref,
                    },
                )
                .unwrap()
                .reference()
        };
        let account = save_account(upstream_key, "Primary");
        let backup = save_account(backup_key, "Backup");
        let sibling = save_account(sibling_key, "Sibling");
        let target = store
            .upstream_account_target(&provider, &account)
            .unwrap()
            .0;
        let make = |scope, accounts: Vec<AccountRef>, strategy, rpm, allow_access| {
            let model = uuid::Uuid::new_v4().to_string();
            let configured_pool = !accounts.is_empty();
            let mut draft = json!({
                "id":uuid::Uuid::new_v4().to_string(), "revision":0,"name":scope,"defaultModelId":model,
                "models":[{"id":model,"name":"Quota fixture","providerId":provider.id,"model":provider.model,"capabilities":{"contextWindow":32000,"maxOutputTokens":4096,"tools":true,"thinking":false,"inputModalities":["text"]}}],
                "routingPolicy":{"enabled":true,"maxAttempts":1,"fallbackOnAccessDenied":allow_access,"accountPools":[{"modelId":model,"accounts":accounts,"strategy":strategy}],"limits":[{"modelId":model,"requestsPerMinute":rpm,"maxConcurrent":0}]}
            });
            if !configured_pool {
                draft["routingPolicy"] =
                    json!({"enabled":false,"deadlineSeconds":5,"cooldownSeconds":2});
            }
            let service = store
                .save_model_service(serde_json::from_value(draft.clone()).unwrap())
                .unwrap();
            draft["revision"] = json!(service.revision);
            let binding = store
                .create_model_service_binding("opencode", scope, &local_key, &service)
                .unwrap()
                .0;
            (
                format!(
                    "{}/messages",
                    crate::gateway::dynamic::configured_endpoint(&fixture.state, &binding)
                ),
                draft,
            )
        };
        let (with_backup, fixed_draft) = make(
            "Fixed order",
            vec![account.clone(), backup.clone()],
            "priority",
            0,
            false,
        );
        let standalone = |name, account: AccountRef| {
            let endpoint = make(name, vec![account.clone()], "priority", 0, false).0;
            let (target, _) = store.upstream_account_target(&provider, &account).unwrap();
            (endpoint, target, account)
        };
        let (default_connection, default_draft) =
            make("Default connection", vec![], "priority", 0, false);
        (
            make(
                "Quota order",
                vec![account.clone()],
                "quota-remaining",
                0,
                false,
            )
            .0,
            make(
                "Round robin",
                vec![account.clone()],
                "round-robin",
                3,
                false,
            )
            .0,
            with_backup,
            make(
                "Weighted",
                vec![account.clone(), backup.clone()],
                "weighted",
                0,
                false,
            )
            .0,
            make(
                "Shared capacity",
                vec![account.clone(), backup.clone()],
                "weighted",
                1,
                false,
            )
            .0,
            make(
                "Permission allowed",
                vec![backup.clone(), sibling.clone()],
                "weighted",
                0,
                true,
            )
            .0,
            make(
                "Permission disabled",
                vec![backup.clone(), sibling.clone()],
                "weighted",
                0,
                false,
            )
            .0,
            make(
                "Quota pace",
                vec![account.clone(), backup.clone()],
                "quota-pace",
                0,
                false,
            )
            .0,
            make(
                "Quota pace reversed",
                vec![backup.clone(), account.clone()],
                "quota-pace",
                0,
                false,
            )
            .0,
            fixed_draft,
            default_connection,
            default_draft,
            store.database_id().unwrap(),
            target,
            account,
            standalone("Same member", sibling),
            standalone("Other member", backup),
        )
    };
    let call = |endpoint: &str, session: &str| {
        fixture.client.post(endpoint).bearer_auth("quota-local-fixture")
            .header("x-opencode-session", session)
            .json(&json!({"model":local_agent_core::routing::ALIAS,"max_tokens":30,"messages":[{"role":"user","content":"Quota fixture"}]}))
            .timeout(Duration::from_secs(5))
    };
    let status = |req: reqwest::RequestBuilder| async move {
        let response = req.send().await.unwrap();
        (response.status(), response.text().await.unwrap())
    };
    let preview_for = async |draft: &serde_json::Value| {
        fixture
            .client
            .post(format!(
                "http://{}/api/model-services/preview",
                fixture.state.host
            ))
            .bearer_auth(fixture.state.token.as_str())
            .json(&json!({"storeId":store_id,"draft":draft,"entry":{"kind":"default"}}))
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<serde_json::Value>()
            .await
            .unwrap()
    };
    let preview = || preview_for(&fixed_draft);
    let now = chrono::Utc::now();
    assert_eq!(
        status(call(&a, "ses_fillsharedlimit")).await.0,
        StatusCode::OK
    );
    assert_eq!(
        status(call(&capacity, "ses_capacityrank")).await.0,
        StatusCode::OK
    );
    let capacity_detail = fixture
        .detail_when("ses_capacityrank", |detail| {
            detail["routeTrace"].is_object() && detail["request"]["attempts"][0].is_object()
        })
        .await;
    assert_eq!(
        capacity_detail["request"]["attempts"][0]["routing"]["service"]["upstreamAccountLabel"],
        "Backup"
    );
    assert!(
        capacity_detail["request"]["attempts"][0]["routing"]["service"]["fallbackReason"].is_null()
    );
    assert!(
        capacity_detail["routeTrace"]["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|decision| decision["accountPosition"] == 1
                && decision["reason"] == "configured-rate-limit")
    );
    assert_eq!(sends.swap(0, Ordering::AcqRel), 1);
    assert_eq!(backup_sends.swap(0, Ordering::AcqRel), 1);
    fixture
        .state
        .observation
        .policy
        .resources
        .lock()
        .unwrap()
        .clear();
    let reopen_budget = || {
        let mut store = fixture.state.store.lock().unwrap();
        let mut billing = fixture.state.billing.lock().unwrap();
        let epoch = billing.sources.lease("openrouter").unwrap().epoch;
        billing.persist_openrouter(&store, epoch).unwrap();
        let data = fixture.root.path().join("data");
        let reopened = Store::open(&data).unwrap();
        reopened
            .interrupt_service_quota_probes(chrono::Utc::now().timestamp_millis())
            .unwrap();
        let mut cold = crate::billing::Billing::new(fixture.root.path().join("home"), &data);
        cold.restore_openrouter(&reopened);
        *store = reopened;
        *billing = cold;
    };
    let new_budget_timeline = || {
        *fixture.state.billing.lock().unwrap() = crate::billing::Billing::new(
            fixture.root.path().join("home"),
            &fixture.root.path().join("data"),
        );
    };
    let seed_pace = |target, account, observed, remaining, hours| {
        fixture
            .state
            .billing
            .lock()
            .unwrap()
            .seed_openrouter_budget_snapshot(
                target,
                account,
                observed,
                &json!({"data":{"budgets":[
                    {"scope":"key_limit","limit_usd":100,"remaining_usd":remaining,
                        "include_byok":false,"reset_interval":"weekly",
                        "resets_at":(now + chrono::Duration::hours(hours)).to_rfc3339()}
                ]}}),
            );
    };
    let assert_configured = async |endpoint: &str, session: &str, expected| {
        let (result, body) = status(call(endpoint, session)).await;
        assert_eq!(result, StatusCode::OK, "{body}");
        assert_eq!(
            (
                sends.swap(0, Ordering::AcqRel),
                backup_sends.swap(0, Ordering::AcqRel),
            ),
            expected,
            "Missing a comparable quota should preserve this service's configured order"
        );
        let detail = fixture
            .detail_when(session, |detail| {
                detail["request"]["attempts"][0]["routing"]["service"].is_object()
            })
            .await;
        assert_eq!(
            detail["request"]["attempts"][0]["routing"]["service"]["accountSelectionReason"],
            "quota_unknown"
        );
    };
    seed_pace(&backup.1, &backup.2, now, 40, 8);
    assert_configured(&paced, "ses_quotaUnknown", (1, 0)).await;
    assert_configured(&reverse_paced, "ses_quotaReverse", (0, 1)).await;
    seed_pace(&target, &account, now - chrono::Duration::days(1), 5, 1);
    assert!(
        fixture
            .state
            .billing
            .lock()
            .unwrap()
            .upstream_quota(&target, Some(&account), now)
            .is_none()
    );
    assert_configured(&paced, "ses_quotaExpired", (1, 0)).await;
    fixture
        .state
        .billing
        .lock()
        .unwrap()
        .seed_openrouter_budget_snapshot(
            &target,
            &account,
            now,
            &json!({"data":{"limit":100,"limit_remaining":70,"limit_reset":null}}),
        );
    assert!(
        fixture
            .state
            .billing
            .lock()
            .unwrap()
            .upstream_quota(&target, Some(&account), now)
            .is_some()
    );
    assert_configured(&paced, "ses_quotaIncomparable", (1, 0)).await;
    seed_pace(&target, &account, now, 70, 48);
    let (pace_status, body) = status(call(&paced, "ses_quotapace")).await;
    assert_eq!(pace_status, StatusCode::OK, "{body}");
    assert_eq!(sends.load(Ordering::Acquire), 0);
    assert_eq!(backup_sends.swap(0, Ordering::AcqRel), 1);
    let detail = fixture
        .detail_when("ses_quotapace", |detail| {
            detail["request"]["attempts"][0]["routing"]["service"].is_object()
        })
        .await;
    let service = &detail["request"]["attempts"][0]["routing"]["service"];
    assert_eq!(service["upstreamAccountLabel"], "Backup");
    assert_eq!(service["accountSelectionReason"], "quota_pace");

    new_budget_timeline();
    let seed_member =
        |target,
         reference,
         member,
         observed_offset,
         reset_offset,
         [remaining, key_remaining, workspace_remaining]: [i64; 3]| {
            fixture.state.billing.lock().unwrap().seed_openrouter_budget_snapshot(
            target,
            reference,
            now + chrono::Duration::seconds(observed_offset),
            &json!({"data":{
                "workspace_id":"workspace-fixture", "creator_user_id":member,
                "limit":100, "limit_remaining":key_remaining, "limit_reset":"daily",
                "budgets":[
                    {"scope":"member","guardrail_id":"shared-policy","limit_usd":100,
                        "remaining_usd":remaining,"include_byok":false,"reset_interval":"daily",
                        "resets_at":(now + chrono::Duration::seconds(reset_offset)).to_rfc3339()},
                    {"scope":"key_limit","limit_usd":100,"remaining_usd":key_remaining,
                        "include_byok":false,"reset_interval":"daily",
                        "resets_at":(now - chrono::Duration::seconds(10)).to_rfc3339()},
                    {"scope":"workspace","budget_id":"shared-workspace-budget","limit_usd":100,
                        "remaining_usd":workspace_remaining,"include_byok":false,"reset_interval":"weekly",
                        "resets_at":(now + chrono::Duration::hours(1)).to_rfc3339()}
                ]
            }}),
        );
        };
    let usage_only = json!({"data":{"usage":1.25}});
    // A partial read does not revoke the key's previously verified membership.
    seed_member(&sibling.1, &sibling.2, "alice", -430, 60, [80, 90, 90]);
    fixture
        .state
        .billing
        .lock()
        .unwrap()
        .seed_openrouter_budget_snapshot(
            &sibling.1,
            &sibling.2,
            now - chrono::Duration::seconds(420),
            &usage_only,
        );
    seed_member(&target, &account, "alice", -400, 60, [0, 90, 90]);
    seed_member(&backup.1, &backup.2, "bob", -95, 60, [80, 90, 90]);
    assert!(
        fixture
            .state
            .billing
            .lock()
            .unwrap()
            .upstream_quota(&target, Some(&account), now)
            .is_none()
    );
    assert_eq!(
        status(call(&sibling.0, "ses_memberstale")).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(sends.load(Ordering::Acquire), 0);
    let after_reset = fixture
        .state
        .billing
        .lock()
        .unwrap()
        .upstream_budget(&sibling.1, now + chrono::Duration::seconds(65));
    assert!(!after_reset.exhausted);
    assert!(
        after_reset
            .scopes(&scopes(&sibling.1).0)
            .any(|(_, reset)| reset.is_some())
    );
    {
        let mut billing = fixture.state.billing.lock().unwrap();
        billing.seed_openrouter_budget_snapshot(
            &sibling.1,
            &sibling.2,
            now - chrono::Duration::seconds(98),
            &usage_only,
        );
        assert!(
            billing
                .upstream_quota(&sibling.1, Some(&sibling.2), now)
                .is_none()
        );
    }
    assert_eq!(
        status(call(&sibling.0, "ses_memberpartial")).await.0,
        StatusCode::TOO_MANY_REQUESTS,
        "A usage-only refresh cannot remove proven shared exhaustion"
    );
    assert_eq!(sends.load(Ordering::Acquire), 0);
    {
        let mut billing = fixture.state.billing.lock().unwrap();
        billing.seed_openrouter_budget_snapshot(
            &sibling.1,
            &sibling.2,
            now - chrono::Duration::seconds(97),
            &json!({"data":{"budgets":[],"effective_budget":null}}),
        );
        assert!(!billing.upstream_budget(&sibling.1, now).exhausted);
        assert!(billing.upstream_budget(&target, now).exhausted);
    }

    // A later positive observation supersedes the exhausted member record
    // even after both amounts have expired and an unrelated key is refreshed.
    seed_member(&sibling.1, &sibling.2, "alice", -96, 60, [80, 90, 90]);
    seed_member(&backup.1, &backup.2, "bob", -94, 60, [80, 90, 90]);
    let past_amount_expiry = fixture
        .state
        .billing
        .lock()
        .unwrap()
        .upstream_budget(&target, now + chrono::Duration::seconds(330));
    assert!(!past_amount_expiry.exhausted);
    assert!(
        past_amount_expiry
            .scopes(&scopes(&target).0)
            .all(|(_, reset)| reset.is_none())
    );

    seed_member(&sibling.1, &sibling.2, "alice", -90, 60, [80, 90, 90]);
    seed_member(&backup.1, &backup.2, "bob", -90, 60, [80, 90, 90]);
    seed_member(&target, &account, "alice", -80, 60, [0, 90, 90]);
    {
        let store = fixture.state.store.lock().unwrap();
        let mut billing = fixture.state.billing.lock().unwrap();
        let epoch = billing.sources.lease("openrouter").unwrap().epoch;
        billing.persist_openrouter(&store, epoch).unwrap();
        let data = fixture.root.path().join("data");
        let reopened = Store::open(&data).unwrap();
        let mut cold = crate::billing::Billing::new(fixture.root.path().join("home"), &data);
        cold.restore_openrouter(&reopened);
        assert!(
            cold.upstream_quota(&sibling.1, Some(&sibling.2), now)
                .is_none()
        );
        let later = cold.upstream_budget(&sibling.1, now + chrono::Duration::seconds(61));
        assert!(
            !later.exhausted
                && later
                    .scopes(&scopes(&sibling.1).0)
                    .any(|(_, probe)| probe.is_some())
        );
        *billing = cold;
    }
    assert_eq!(
        preview().await["models"][0]["accounts"][0]["quota"],
        "exhausted"
    );
    assert_eq!(
        status(call(&sibling.0, "ses_memberexhausted")).await.0,
        StatusCode::TOO_MANY_REQUESTS,
        "Fresh member exhaustion applies to a different key with proven membership"
    );
    assert_eq!(sends.load(Ordering::Acquire), 0);
    assert_eq!(
        status(call(&backup.0, "ses_memberindependent")).await.0,
        StatusCode::OK
    );
    assert_eq!(backup_sends.load(Ordering::Acquire), 1);

    seed_member(&sibling.1, &sibling.2, "alice", -70, 60, [60, 90, 90]);
    assert_eq!(
        preview().await["models"][0]["accounts"][0]["quota"],
        serde_json::Value::Null,
        "another key's refresh can lift the block without restoring this key's display snapshot"
    );
    seed_member(&target, &account, "alice", -69, 60, [60, 90, 0]);
    assert_eq!(
        status(call(&backup.0, "ses_workspaceexhausted")).await.0,
        StatusCode::TOO_MANY_REQUESTS,
        "An independent workspace budget can constrain both members"
    );
    assert_eq!(backup_sends.load(Ordering::Acquire), 1);
    seed_member(&backup.1, &backup.2, "bob", -68, 60, [80, 90, 90]);
    // This member's first request arrives after the old post-reset window.
    // A newer refresh for a different member must leave its probe pending.
    new_budget_timeline();
    seed_member(
        &sibling.1,
        &sibling.2,
        "late-alice",
        -410,
        -330,
        [60, 90, 90],
    );
    seed_member(&target, &account, "late-alice", -400, -330, [0, 90, 90]);
    seed_member(&backup.1, &backup.2, "bob", -59, 60, [80, 90, 90]);
    assert_eq!(
        preview().await["models"][0]["accounts"][0]["quota"],
        "recovery"
    );
    let cooldown_scope = scopes(&target).0;
    let expired_at = chrono::Utc::now().timestamp_millis() - 50;
    fixture
        .state
        .store
        .lock()
        .unwrap()
        .service_set_cooldown(&cooldown_scope, expired_at, "upstream_rate_limit")
        .unwrap();
    let db =
        rusqlite::Connection::open(fixture.root.path().join("data/local-agent.sqlite3")).unwrap();
    let probe_counts = || {
        db.query_row(
            "SELECT (SELECT count(*) FROM service_openrouter_probes),
                    (SELECT count(*) FROM service_cooldown_probe_sends)",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap()
    };
    let before = probe_counts();
    for (session, failure) in [
        (
            "ses_quotaAttemptCheckpoint",
            "BEFORE UPDATE ON service_route_attempts WHEN NEW.outcome='pending'",
        ),
        (
            "ses_quotaHttpCheckpoint",
            "BEFORE INSERT ON service_cooldown_probe_sends",
        ),
        (
            "ses_quotaRecoveryCheckpoint",
            "BEFORE INSERT ON service_openrouter_probes",
        ),
    ] {
        db.execute_batch(&format!(
            "CREATE TRIGGER fixture_checkpoint_failure {failure}
             BEGIN SELECT RAISE(FAIL,'fixture checkpoint write failed'); END;"
        ))
        .unwrap();
        let (result, body) = status(call(&a, session)).await;
        db.execute_batch("DROP TRIGGER fixture_checkpoint_failure")
            .unwrap();
        assert_eq!(result, StatusCode::SERVICE_UNAVAILABLE, "{body}");
        assert_eq!(sends.load(Ordering::Acquire), 0);
        assert_eq!(
            probe_counts(),
            before,
            "{session} must leave no probe claim"
        );
        assert_eq!(
            db.query_row(
                "SELECT outcome FROM service_route_attempts WHERE session=?1",
                [format!("opencode:{session}")],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
            "not_sent"
        );
        assert_eq!(
            preview().await["models"][0]["accounts"][0]["quota"],
            "recovery",
            "a failed send checkpoint must not consume quota recovery eligibility"
        );
    }
    assert_eq!(
        fixture
            .state
            .store
            .lock()
            .unwrap()
            .service_cooldowns()
            .unwrap()[&cooldown_scope]
            .0,
        expired_at
    );
    drop(db);
    pause.store(true, Ordering::Release);
    let shared_probe = tokio::spawn(status(call(&default_connection, "ses_membersharedprobe")));
    tokio::time::timeout(Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    let sibling_busy = status(call(&sibling.0, "ses_membersiblingbusy")).await;
    assert_eq!(sibling_busy.0, StatusCode::TOO_MANY_REQUESTS);
    assert!(
        sibling_busy.1.contains("正在探测恢复"),
        "{}",
        sibling_busy.1
    );
    assert_eq!(sends.load(Ordering::Acquire), 1);
    assert_eq!(
        status(call(&weighted, "ses_weightedwhileprobing")).await.0,
        StatusCode::OK
    );
    assert_eq!(
        status(call(&backup.0, "ses_memberduringprobe")).await.0,
        StatusCode::OK
    );
    resume.notify_one();
    assert_eq!(shared_probe.await.unwrap().0, StatusCode::OK);
    let weighted_probe = fixture
        .detail_when("ses_weightedwhileprobing", |detail| {
            detail["routeTrace"].is_object() && detail["request"]["attempts"][0].is_object()
        })
        .await;
    assert_eq!(
        weighted_probe["request"]["attempts"][0]["routing"]["service"]["upstreamAccountLabel"],
        "Backup"
    );
    assert!(
        weighted_probe["request"]["attempts"][0]["routing"]["service"]["fallbackReason"].is_null()
    );
    assert!(
        weighted_probe["routeTrace"]["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|decision| decision["accountPosition"] == 1
                && decision["reason"] == "recovery-in-progress")
    );
    reopen_budget();
    assert_eq!(
        preview().await["models"][0]["accounts"][0]["quota"],
        "recovered"
    );
    assert_eq!(
        status(call(&sibling.0, "ses_membersiblingrestored"))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(sends.swap(0, Ordering::AcqRel), 2);
    assert_eq!(backup_sends.swap(0, Ordering::AcqRel), 3);

    let release_headers_late = || {
        let resume = resume.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(5500)).await;
            resume.notify_waiters();
        })
    };
    pause.store(true, Ordering::Release);
    let healthy = tokio::spawn(status(
        call(&default_connection, "ses_fixedQuotaHealthyHeaders").timeout(Duration::from_secs(8)),
    ));
    tokio::time::timeout(Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    let release = release_headers_late();
    assert_eq!(healthy.await.unwrap().0, StatusCode::OK);
    release.await.unwrap();

    new_budget_timeline();
    let at = chrono::Utc::now();
    fixture.state.billing.lock().unwrap().seed_exhausted_budget(
        &target,
        &account,
        at - chrono::Duration::seconds(90),
        at.timestamp() - 30,
    );
    assert_eq!(
        preview_for(&default_draft).await["models"][0]["accounts"][0]["quota"],
        "recovery"
    );
    pause.store(true, Ordering::Release);
    let probe = tokio::spawn(status(
        call(&default_connection, "ses_fixedQuotaProbeHeaders").timeout(Duration::from_secs(8)),
    ));
    tokio::time::timeout(Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    assert_eq!(
        preview_for(&default_draft).await["models"][0]["accounts"][0]["quota"],
        "recovering"
    );
    let release = release_headers_late();
    let timed_out = probe.await.unwrap();
    assert_eq!(timed_out.0, StatusCode::GATEWAY_TIMEOUT, "{}", timed_out.1);
    assert!(timed_out.1.contains("上游结果未确认"), "{}", timed_out.1);
    assert_eq!(
        preview_for(&default_draft).await["models"][0]["accounts"][0]["quota"],
        "recoveryCooling"
    );
    let during_cooldown = status(call(&a, "ses_quotaProbeDeadlineCooling")).await;
    assert_eq!(during_cooldown.0, StatusCode::TOO_MANY_REQUESTS);
    assert!(
        during_cooldown.1.contains("正在冷却"),
        "{}",
        during_cooldown.1
    );
    assert_eq!(
        status(call(&with_backup, "ses_quotaProbeDeadlineBackup"))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(sends.load(Ordering::Acquire), 2);
    assert_eq!(backup_sends.swap(0, Ordering::AcqRel), 1);
    release.await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let state =
                preview_for(&default_draft).await["models"][0]["accounts"][0]["quota"].clone();
            if state == "recovery" {
                break;
            }
            assert_eq!(state, "recoveryCooling");
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        status(call(&default_connection, "ses_quotaProbeDeadlineRecovered"))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        preview_for(&default_draft).await["models"][0]["accounts"][0]["quota"],
        "recovered"
    );
    assert_eq!(sends.swap(0, Ordering::AcqRel), 3);

    // Start the independent key-budget/RPM scenario with an empty minute window.
    new_budget_timeline();
    fixture
        .state
        .observation
        .policy
        .resources
        .lock()
        .unwrap()
        .clear();
    {
        let mut billing = fixture.state.billing.lock().unwrap();
        billing.seed_openrouter_budget_snapshot(
            &target,
            &account,
            now - chrono::Duration::hours(1),
            &json!({"data":{"limit":100,"limit_remaining":0,"limit_reset":null}}),
        );
        assert!(
            billing
                .upstream_budget(&target, now + chrono::Duration::days(1))
                .exhausted
        );
        billing.seed_openrouter_budget_snapshot(
            &target,
            &account,
            now - chrono::Duration::seconds(2),
            &usage_only,
        );
        assert!(billing.upstream_budget(&target, now).exhausted);
        assert!(
            billing
                .upstream_quota(&target, Some(&account), now)
                .is_none()
        );
    }
    assert_eq!(
        status(call(&a, "ses_keypartial")).await.0,
        StatusCode::TOO_MANY_REQUESTS,
        "A usage-only refresh cannot release a key's confirmed budget block"
    );
    assert_eq!(sends.load(Ordering::Acquire), 0);
    {
        let mut billing = fixture.state.billing.lock().unwrap();
        billing.seed_openrouter_budget_snapshot(
            &target,
            &account,
            now - chrono::Duration::seconds(1),
            &json!({"data":{"limit":null,"limit_remaining":null}}),
        );
        assert!(!billing.upstream_budget(&target, now).exhausted);
        billing.seed_openrouter_budget_snapshot(
            &target,
            &account,
            now,
            &json!({"data":{"limit":100,"limit_remaining":25,"limit_reset":null}}),
        );
        assert!(!billing.upstream_budget(&target, now).exhausted);
    }

    let initial = status(call(&a, "ses_quotaold")).await;
    assert_eq!(initial.0, StatusCode::OK, "{}", initial.1);
    let now = chrono::Utc::now();
    new_budget_timeline();
    let seed = |reset| {
        fixture.state.billing.lock().unwrap().seed_exhausted_budget(
            &target,
            &account,
            now - chrono::Duration::seconds(360),
            reset,
        );
    };
    seed(now.timestamp() + 30);
    {
        let mut billing = fixture.state.billing.lock().unwrap();
        billing.seed_openrouter_budget_snapshot(&target, &account, now, &usage_only);
        let after_reset = billing.upstream_budget(&target, now + chrono::Duration::seconds(31));
        assert!(!after_reset.exhausted && after_reset.key_reset.is_some());
    }
    assert_eq!(
        preview().await["models"][0]["accounts"][0]["quota"],
        "exhausted"
    );
    assert_eq!(
        status(call(&b, "ses_quotaearly")).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        preview_for(&default_draft).await["models"][0]["accounts"][0]["quota"],
        "exhausted"
    );
    let default_blocked = status(call(&default_connection, "ses_defaultquotaearly")).await;
    assert_eq!(default_blocked.0, StatusCode::TOO_MANY_REQUESTS);
    assert!(
        default_blocked.1.contains("已知预算已用尽"),
        "{}",
        default_blocked.1
    );
    assert_eq!(
        status(call(&with_backup, "ses_quotapriorityearly")).await.0,
        StatusCode::OK
    );
    assert_eq!(
        status(call(&weighted, "ses_quotaweightedearly")).await.0,
        StatusCode::OK
    );
    let weighted_detail = fixture
        .detail_when("ses_quotaweightedearly", |detail| {
            detail["routeTrace"].is_object()
        })
        .await;
    assert!(
        weighted_detail["routeTrace"]["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|decision| decision["accountPosition"] == 1
                && decision["reason"] == "account-quota-exhausted"),
        "The exhausted key must be outside the weighted allocation: {weighted_detail}"
    );
    assert_eq!(backup_sends.load(Ordering::Acquire), 2);
    assert_eq!(sends.load(Ordering::Acquire), 1);

    new_budget_timeline();
    seed(now.timestamp() - 330);
    fixture
        .state
        .billing
        .lock()
        .unwrap()
        .seed_openrouter_budget_snapshot(&target, &account, now, &usage_only);
    assert_eq!(
        preview().await["models"][0]["accounts"][0]["quota"],
        "recovery"
    );
    let later = now + chrono::Duration::days(1);
    let stale = fixture
        .state
        .billing
        .lock()
        .unwrap()
        .upstream_budget(&target, later);
    assert!(!stale.exhausted);
    assert!(
        stale
            .key_reset
            .is_some_and(|probe| probe.valid_until > later.timestamp())
    );
    pause.store(true, Ordering::Release);
    let replay = tokio::spawn(status(call(&a, "ses_quotaold")));
    tokio::time::timeout(Duration::from_secs(3), entered.notified())
        .await
        .unwrap();
    assert_eq!(
        preview().await["models"][0]["accounts"][0]["quota"],
        "recovering"
    );
    let busy = status(call(&b, "ses_quotabusy")).await;
    assert_eq!(busy.0, StatusCode::TOO_MANY_REQUESTS);
    assert!(busy.1.contains("正在探测恢复"), "{}", busy.1);
    assert_eq!(
        status(call(&with_backup, "ses_quotabackup")).await.0,
        StatusCode::OK
    );
    assert_eq!(sends.load(Ordering::Acquire), 2);
    assert_eq!(backup_sends.load(Ordering::Acquire), 3);
    resume.notify_one();
    assert_eq!(replay.await.unwrap().0, StatusCode::OK);
    assert_eq!(
        preview().await["models"][0]["accounts"][0]["quota"],
        "recovered"
    );
    assert_eq!(status(call(&b, "ses_quotanext")).await.0, StatusCode::OK);
    assert_eq!(
        status(call(&b, "ses_quotaactualrpm")).await.0,
        StatusCode::TOO_MANY_REQUESTS,
        "The three actual sends across both services consume the shared minute budget"
    );
    assert_eq!(sends.load(Ordering::Acquire), 3);

    new_budget_timeline();
    seed(now.timestamp() - 1);
    assert_eq!(
        preview().await["models"][0]["accounts"][0]["quota"],
        "recovery",
        "A previous successful probe does not apply to another reset window"
    );
    limited.store(true, Ordering::Release);
    assert_eq!(
        status(call(&a, "ses_quotaold")).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        status(call(&b, "ses_quotacooling")).await.0,
        StatusCode::TOO_MANY_REQUESTS,
    );
    assert_eq!(
        preview().await["models"][0]["accounts"][0]["quota"],
        "recoveryCooling"
    );
    let cooldown = fixture
        .state
        .store
        .lock()
        .unwrap()
        .service_cooldown(&scopes(&target).0)
        .unwrap()
        .unwrap();
    assert_eq!(cooldown.1, "upstream_rate_limit");
    assert!(cooldown.0 >= now.timestamp_millis() + 59_000);
    reopen_budget();
    assert_eq!(
        preview().await["models"][0]["accounts"][0]["quota"],
        "recoveryCooling"
    );
    assert_eq!(sends.load(Ordering::Acquire), 4);

    limited.store(false, Ordering::Release);
    fixture
        .state
        .store
        .lock()
        .unwrap()
        .service_set_cooldown(
            &access_scopes(&backup.1).1,
            chrono::Utc::now().timestamp_millis() + 30_000,
            "upstream_model_access_denied",
        )
        .unwrap();
    assert_eq!(
        status(call(&permission_off, "ses_permissionoff")).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(sends.load(Ordering::Acquire), 4);
    assert_eq!(backup_sends.load(Ordering::Acquire), 3);
    assert_eq!(
        status(call(&permission, "ses_permissionrank")).await.0,
        StatusCode::OK
    );
    let permission_detail = fixture
        .detail_when("ses_permissionrank", |detail| {
            detail["routeTrace"].is_object() && detail["request"]["attempts"][0].is_object()
        })
        .await;
    assert!(
        permission_detail["request"]["attempts"][0]["routing"]["service"]["fallbackReason"]
            .is_null()
    );
    assert!(
        permission_detail["routeTrace"]["decisions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|decision| decision["accountPosition"] == 1
                && decision["reason"] == "access-model-denied")
    );
    assert_eq!(sends.load(Ordering::Acquire), 5);
    assert_eq!(backup_sends.load(Ordering::Acquire), 3);
}
