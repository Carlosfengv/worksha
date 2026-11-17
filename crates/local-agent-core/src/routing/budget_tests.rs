use super::{budget::*, *};
use crate::StoreWriter;
use crate::model_services::policy::SendBoundary;
use chrono::{DateTime, Utc};
use std::{sync::Barrier, thread};

fn uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn policy(tokens: u64, usd: Option<u64>) -> Policy {
    Policy {
        revision: 0,
        timezone: "Asia/Shanghai".into(),
        fallback_service_id: None,
        warn_percent: 80,
        windows: [WindowKind::Daily, WindowKind::Monthly]
            .map(|kind| WindowPolicy {
                kind,
                primary: Amounts {
                    tokens: Some(tokens),
                    usd_micros: usd,
                },
                fallback: Amounts::default(),
            })
            .to_vec(),
    }
}

fn binding(store: &Store) -> super::Binding {
    let mut provider = store.providers().unwrap().remove(0);
    provider.endpoint = "http://127.0.0.1:32000".into();
    provider.configured = true;
    provider.protocol = "messages".into();
    provider.model = "fixture".into();
    provider.models = vec![provider.model.clone()];
    provider.credential_ref = None;
    store
        .dynamic_create("claude", "test-fixture", &uuid(), &provider)
        .unwrap()
}

fn prepare<'a>(binding: &'a str, id: &'a str, time: DateTime<Utc>) -> Prepare<'a> {
    Prepare {
        binding_id: binding,
        session: None,
        turn: "fixture",
        request_id: id,
        observation_id: id,
        provider_id: "fixture-provider",
        model: "fixture-model",
        selected_tier: Tier::Primary,
        continuation: false,
        now: time,
        estimate: Amounts {
            tokens: Some(50),
            usd_micros: Some(100),
        },
        fallback: None,
    }
}

#[test]
fn recover_preparing_and_sending_then_partially_reconcile_across_windows() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let binding = binding(&store);
    let store_id = store.database_id().unwrap();
    store
        .save_budget_policy(&store_id, &binding.id, policy(200, Some(400)))
        .unwrap();
    let time = "2026-10-31T15:59:59Z".parse().unwrap();
    let first = store
        .budget_prepare(&prepare(&binding.id, &uuid(), time))
        .unwrap()
        .unwrap();
    let second = store
        .budget_prepare(&prepare(&binding.id, &uuid(), time))
        .unwrap()
        .unwrap();
    store
        .service_begin_scoped_send(
            None,
            None,
            None,
            SendBoundary {
                budget_attempt: Some(&second.attempt_key),
                ..SendBoundary::default()
            },
        )
        .unwrap();
    drop(store);

    let store = Store::open(root.path()).unwrap();
    store.budget_recover().unwrap();
    let pending = store.budget_uncertain(&binding.id).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].attempt_key, second.attempt_key);
    assert_eq!(
        store
            .budget_complete(&first.attempt_key, Amounts::default(), None)
            .unwrap()
            .state,
        AttemptState::Released
    );
    assert!(
        pending[0]
            .windows
            .iter()
            .any(|window| window.key == "2026-10-31")
    );
    assert!(
        pending[0]
            .windows
            .iter()
            .any(|window| window.key == "2026-10")
    );
    let new_windows = store
        .budget_window_status(&binding.id, "2026-10-31T16:00:01Z".parse().unwrap())
        .unwrap();
    assert!(
        new_windows
            .iter()
            .all(|window| window.ledger.tokens.uncertain == 0)
    );

    let review = Review {
        store_id,
        operation_id: uuid(),
        binding_id: binding.id.clone(),
        attempt_key: second.attempt_key.clone(),
        expected_revision: pending[0].revision,
        upstream_request_id: "fixture-request".into(),
        source: "Mock upstream invoice item #4".into(),
        tokens: Some(ReviewMetric::Settle { amount: 70 }),
        usd_micros: None,
    };
    let receipt = store.budget_review(&review, "owner-fixture").unwrap();
    assert_eq!(receipt.attempt.state, AttemptState::Uncertain);
    assert_eq!(
        receipt.attempt.frozen,
        Amounts {
            tokens: None,
            usd_micros: Some(100)
        }
    );
    assert_eq!(
        store
            .budget_review(&review, "owner-fixture")
            .unwrap()
            .attempt
            .revision,
        receipt.attempt.revision
    );
    let conflicting = Review {
        tokens: Some(ReviewMetric::Release),
        ..review.clone()
    };
    assert_eq!(
        store
            .budget_review(&conflicting, "owner-fixture")
            .unwrap_err()
            .code,
        "conflict"
    );
    assert!(
        store
            .budget_review(
                &Review {
                    operation_id: uuid(),
                    source: String::new(),
                    tokens: None,
                    usd_micros: Some(ReviewMetric::Release),
                    expected_revision: receipt.attempt.revision,
                    ..review.clone()
                },
                "owner-fixture"
            )
            .is_err()
    );

    let late = store
        .budget_complete(
            &second.attempt_key,
            Amounts {
                tokens: Some(99),
                usd_micros: Some(0),
            },
            Some("fixture-request"),
        )
        .unwrap();
    assert_eq!(late.state, AttemptState::Committed);
    assert_eq!(
        late.actual,
        Amounts {
            tokens: Some(70),
            usd_micros: Some(0)
        }
    );
    assert_eq!(late.late_evidence.tokens, Some(99));
    assert_eq!(
        store
            .budget_review(
                &Review {
                    operation_id: uuid(),
                    tokens: None,
                    usd_micros: Some(ReviewMetric::Release),
                    expected_revision: receipt.attempt.revision,
                    ..review
                },
                "owner-fixture"
            )
            .unwrap_err()
            .code,
        "conflict"
    );
    for window in store.budget_window_status(&binding.id, time).unwrap() {
        assert_eq!(window.ledger.tokens.committed, 70);
        assert_eq!(window.ledger.tokens.uncertain, 0);
        assert_eq!(window.ledger.tokens.reserved, 0);
        assert_eq!(window.ledger.usd_micros.uncertain, 0);
    }

    let racing = store
        .budget_prepare(&prepare(&binding.id, &uuid(), time))
        .unwrap()
        .unwrap();
    mark(&store, &racing.attempt_key).unwrap();
    store
        .budget_record_response(&racing.attempt_key, Some("racing-request"), false)
        .unwrap();
    let frozen = store
        .budget_complete(&racing.attempt_key, Amounts::default(), None)
        .unwrap();
    let review = Review {
        store_id: store.database_id().unwrap(),
        operation_id: uuid(),
        binding_id: binding.id.clone(),
        attempt_key: frozen.attempt_key.clone(),
        expected_revision: frozen.revision,
        upstream_request_id: "racing-request".into(),
        source: "Mock request detail".into(),
        tokens: None,
        usd_micros: Some(ReviewMetric::Release),
    };
    assert_eq!(
        store
            .budget_review(
                &Review {
                    upstream_request_id: "unrelated-request".into(),
                    ..review.clone()
                },
                "owner-fixture",
            )
            .unwrap_err()
            .code,
        "invalid_input"
    );
    let store = StoreWriter::new(store);
    let ready = Barrier::new(2);
    let manual = thread::scope(|scope| {
        let automatic = scope.spawn(|| {
            ready.wait();
            store
                .lock()
                .unwrap()
                .budget_complete(
                    &frozen.attempt_key,
                    Amounts {
                        tokens: Some(11),
                        usd_micros: Some(13),
                    },
                    Some("racing-request"),
                )
                .unwrap();
        });
        ready.wait();
        let manual = store
            .lock()
            .unwrap()
            .budget_review(&review, "owner-fixture");
        automatic.join().unwrap();
        manual
    });
    let expected_cost = match manual {
        Ok(receipt) => {
            assert_eq!(receipt.confirmed.usd_micros, Some(0));
            0
        }
        Err(error) => {
            assert_eq!(error.code, "conflict");
            13
        }
    };
    let store = store.lock().unwrap();
    assert!(store.budget_uncertain(&binding.id).unwrap().is_empty());
    for window in store.budget_window_status(&binding.id, time).unwrap() {
        assert_eq!(window.ledger.tokens.committed, 81);
        assert_eq!(window.ledger.usd_micros.committed, expected_cost);
        assert_eq!(window.ledger.usd_micros.uncertain, 0);
    }
}

#[test]
fn disabled_unpriced_or_over_limit_attempt_does_not_get_a_send_marker() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let binding = binding(&store);
    let store_id = store.database_id().unwrap();
    let time = "2026-10-31T15:00:00Z".parse().unwrap();
    let id = uuid();
    let request = prepare(&binding.id, &id, time);
    assert!(store.budget_prepare(&request).unwrap().is_none());
    store
        .save_budget_policy(&store_id, &binding.id, policy(49, Some(400)))
        .unwrap();
    assert_eq!(
        store.budget_prepare(&request).unwrap_err().code,
        "local_agent_budget_exceeded"
    );
    let unknown = Prepare {
        estimate: Amounts {
            tokens: Some(1),
            usd_micros: None,
        },
        ..request
    };
    assert_eq!(
        store.budget_prepare(&unknown).unwrap_err().code,
        "budget_pricing_unavailable"
    );
    assert!(
        store
            .budget_window_status(&binding.id, time)
            .unwrap()
            .iter()
            .all(|window| window.ledger.tokens.reserved == 0)
    );
}

#[test]
fn grace_limits_requests_and_uses_fallback_only_with_a_confirmed_primary_route() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let primary = super::binding_creation_tests::service(&store, "Primary");
    let alternate = super::binding_creation_tests::service(&store, "Alternate");
    let (binding, _) = store
        .create_model_service_binding("claude", "owned-fixture", &uuid(), &primary)
        .unwrap();
    let store_id = store.database_id().unwrap();
    let mut rule = policy(30_000, None);
    rule.fallback_service_id = Some(alternate.id);
    for window in &mut rule.windows {
        window.fallback.tokens = Some(15_000);
    }
    rule = store
        .save_budget_policy(&store_id, &binding.id, rule)
        .unwrap();
    let time = "2026-10-31T15:00:00Z".parse().unwrap();
    let turn = Turn {
        session: Some("claude:fixture".into()),
        digest: "turn-a".into(),
        chain: vec!["chain-a".into()],
        continuation: false,
        portable: true,
    };
    let selected = store
        .model_service_admit(&binding.id, &turn, ALIAS)
        .unwrap();
    assert!(selected.advances_session_head);
    assert!(
        !store
            .model_service_admit(&binding.id, &turn, ALIAS)
            .unwrap()
            .advances_session_head
    );
    let id = uuid();
    let first = store
        .budget_prepare(&scoped(&binding.id, &turn, &id, time))
        .unwrap()
        .unwrap();
    mark(&store, &first.attempt_key).unwrap();
    let blocked = Prepare {
        continuation: true,
        ..scoped(&binding.id, &turn, &id, time)
    };
    assert_eq!(
        store.budget_prepare(&blocked).unwrap_err().code,
        "dispatch_not_committed"
    );
    store
        .budget_record_response(&first.attempt_key, Some("upstream-fixture"), true)
        .unwrap();
    store
        .budget_complete(
            &first.attempt_key,
            Amounts {
                tokens: Some(29_999),
                usd_micros: None,
            },
            None,
        )
        .unwrap();
    let aborted_id = uuid();
    let aborted = store
        .budget_prepare(&Prepare {
            continuation: true,
            ..scoped(&binding.id, &turn, &aborted_id, time)
        })
        .unwrap()
        .unwrap();
    store
        .budget_complete(&aborted.attempt_key, Amounts::default(), None)
        .unwrap();
    for actual in [0, 42] {
        let id = uuid();
        let attempt = store
            .budget_prepare(&Prepare {
                continuation: true,
                ..scoped(&binding.id, &turn, &id, time)
            })
            .unwrap()
            .unwrap();
        assert!(attempt.grace);
        assert_eq!(attempt.tier, Tier::Fallback);
        mark(&store, &attempt.attempt_key).unwrap();
        store
            .budget_complete(
                &attempt.attempt_key,
                Amounts {
                    tokens: Some(actual),
                    usd_micros: None,
                },
                None,
            )
            .unwrap();
    }
    let store = StoreWriter::new(store);
    let ready = Barrier::new(2);
    let [a, b] = thread::scope(|scope| {
        [uuid(), uuid()]
            .map(|request_id| {
                let (store, ready, binding, turn) = (&store, &ready, &binding, &turn);
                scope.spawn(move || {
                    ready.wait();
                    store.lock().unwrap().budget_prepare(&Prepare {
                        continuation: true,
                        ..scoped(&binding.id, turn, &request_id, time)
                    })
                })
            })
            .map(|worker| worker.join().unwrap())
    });
    let last = match (a, b) {
        (Ok(Some(last)), Err(denied)) | (Err(denied), Ok(Some(last))) => {
            assert_eq!(denied.code, "budget_grace_exceeded");
            last
        }
        result => panic!("expected exactly one request to receive the last grace slot: {result:?}"),
    };
    let store = store.lock().unwrap();
    assert!(last.grace);
    mark(&store, &last.attempt_key).unwrap();
    store
        .budget_complete(
            &last.attempt_key,
            Amounts {
                tokens: Some(43),
                usd_micros: None,
            },
            None,
        )
        .unwrap();
    let id = uuid();
    assert_eq!(
        store
            .budget_prepare(&Prepare {
                continuation: true,
                ..scoped(&binding.id, &turn, &id, time)
            })
            .unwrap_err()
            .code,
        "budget_grace_exceeded"
    );
    let windows = store.budget_window_status(&binding.id, time).unwrap();
    assert!(
        windows
            .iter()
            .filter(|w| w.tier == Tier::Fallback)
            .all(|w| w.ledger.tokens.committed == 85)
    );
    rule.fallback_service_id = None;
    for window in &mut rule.windows {
        window.fallback = Amounts::default();
    }
    store
        .save_budget_policy(&store_id, &binding.id, rule)
        .unwrap();
    assert_eq!(
        store
            .budget_prepare(&Prepare {
                continuation: true,
                ..scoped(&binding.id, &turn, &id, time)
            })
            .unwrap_err()
            .code,
        "local_agent_budget_exceeded"
    );
}

fn scoped<'a>(binding: &'a str, turn: &'a Turn, id: &'a str, time: DateTime<Utc>) -> Prepare<'a> {
    Prepare {
        session: turn.session.as_deref(),
        turn: &turn.digest,
        estimate: Amounts {
            tokens: Some(100),
            usd_micros: None,
        },
        ..prepare(binding, id, time)
    }
}

fn mark(store: &Store, key: &str) -> Result<(), ConfigError> {
    store.service_begin_scoped_send(
        None,
        None,
        None,
        SendBoundary {
            budget_attempt: Some(key),
            ..SendBoundary::default()
        },
    )
}
