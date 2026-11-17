use super::*;
use crate::{
    StoreWriter,
    model_services::policy::SendBoundary,
    routing::budget::{Attempt, AttemptState, Prepare, Review, ReviewMetric, Tier},
};
use chrono::Utc;
use std::{sync::Barrier, thread};

const SESSION: &str = "claude:fixture";

fn id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn next_turn(index: u32) -> Turn {
    Turn {
        session: Some(SESSION.into()),
        digest: format!("turn-{index}"),
        chain: (0..=index)
            .map(|index| format!("message-{index}"))
            .collect(),
        continuation: false,
        portable: true,
    }
}

fn open() -> (tempfile::TempDir, Store, String) {
    let path = tempfile::tempdir().unwrap();
    let store = Store::open(path.path()).unwrap();
    let mut provider = store.providers().unwrap().remove(0);
    provider.endpoint = "http://127.0.0.1:32000".into();
    provider.configured = true;
    provider.protocol = "messages".into();
    provider.model = "fixture".into();
    provider.models = vec![provider.model.clone()];
    provider.credential_ref = None;
    let binding = store
        .dynamic_create("claude", "owned-fixture", &id(), &provider)
        .unwrap();
    (path, store, binding.id)
}

fn advance(store: &Store, binding: &str, index: u32) -> (Turn, Scope) {
    let mut turn = next_turn(index);
    assert!(
        store
            .dynamic_admit(binding, &turn)
            .unwrap()
            .advances_session_head
    );
    let scope = store.loop_admit(binding, &turn, None).unwrap().unwrap();
    turn.continuation = true;
    (turn, scope)
}

fn batch(index: u32, ids: impl IntoIterator<Item = u32>, errors: bool) -> Batch {
    Batch {
        fingerprint: format!("batch-{index}"),
        calls: ids
            .into_iter()
            .map(|id| Call {
                id_hash: format!("call-{id}"),
                action_hash: format!("action-{id}"),
            })
            .collect(),
        has_error: errors,
        has_success: !errors,
    }
}

fn read(store: &Store, binding: &str) -> Head {
    head(&store.connection, binding, SESSION).unwrap().unwrap()
}

fn override_input(store: &Store, binding: &str) -> Override {
    let current = read(store, binding);
    Override {
        store_id: store.database_id().unwrap(),
        operation_id: id(),
        binding_id: binding.into(),
        session_id: SESSION.into(),
        expected_turn_digest: current.digest,
        expected_override_epoch: current.guard.override_epoch,
    }
}

fn errors_trip(store: &Store, binding: &str, turn: &Turn, start: u32) {
    for i in 0..4 {
        store
            .loop_admit(binding, turn, Some(&batch(start + i, [start + i], true)))
            .unwrap();
    }
    assert_eq!(read(store, binding).guard.status, Status::Warned);
    assert_eq!(
        store
            .loop_admit(binding, turn, Some(&batch(start + 4, [start + 4], true)))
            .unwrap_err()
            .code,
        CIRCUIT
    );
}

#[test]
fn unspent_older_lease_cannot_unlock_and_override_keeps_entire_dedup_history() {
    let (_root, store, binding) = open();
    let (old, old_scope) = advance(&store, &binding, 0);
    let old_prepared = prepare(&store, &binding, &old, old_scope, amounts(100, None));
    let (turn, _) = advance(&store, &binding, 1);
    // An A -> B -> A replay, or a changed result with the same tool ID,
    // cannot increment the action count twice or count the exact batch again.
    let a = batch(0, [0, 1], false);
    let b = batch(1, [1, 2], false);
    for item in [&a, &b, &a] {
        store.loop_admit(&binding, &turn, Some(item)).unwrap();
    }
    let guard = read(&store, &binding).guard;
    assert_eq!(guard.steps, 2);
    assert_eq!(guard.seen_tool_call_ids.len(), 3);
    assert!(guard.actions.values().all(|count| *count == 1));
    errors_trip(&store, &binding, &turn, 10);
    let tripped = read(&store, &binding);
    assert_eq!(tripped.guard.reason, Some(Reason::ToolErrorStreak));
    let _ = store.dynamic_admit(&binding, &next_turn(0));
    assert_eq!(
        store.loop_admit(&binding, &old, None).unwrap_err().code,
        CIRCUIT
    );
    assert_eq!(begin_send(&store, &old_prepared).unwrap_err().code, CIRCUIT);
    assert_eq!(read(&store, &binding).seq, tripped.seq);
    assert_eq!(read(&store, &binding).guard.errors, 5);

    let operation = override_input(&store, &binding);
    assert_eq!(store.loop_override(&operation).unwrap().override_epoch, 1);
    assert_eq!(store.loop_override(&operation).unwrap().override_epoch, 1);
    let mut changed = operation.clone();
    changed.expected_override_epoch += 1;
    assert_eq!(store.loop_override(&changed).unwrap_err().code, "conflict");
    store.loop_admit(&binding, &turn, Some(&a)).unwrap();
    assert_eq!(read(&store, &binding).guard.steps, tripped.guard.steps);
    store
        .loop_admit(&binding, &turn, Some(&batch(100, [100], false)))
        .unwrap();
    errors_trip(&store, &binding, &turn, 101);
    let mut next = next_turn(2);
    next.chain[0] = "untrusted-prefix".into();
    next.digest = "untrusted-turn".into();
    assert_eq!(
        store.dynamic_admit(&binding, &next).unwrap_err().code,
        CIRCUIT
    );
    assert_eq!(
        store.loop_admit(&binding, &next, None).unwrap_err().code,
        CIRCUIT
    );
    advance(&store, &binding, 2);
    let next = read(&store, &binding);
    assert_eq!(next.guard.override_epoch, 0);
    assert_eq!(next.guard.status, Status::Normal);
    assert!(next.guard.seen_batch_fingerprints.is_empty());
    assert!(next.guard.seen_tool_call_ids.is_empty());
    assert_eq!(
        begin_send(&store, &old_prepared).unwrap_err().code,
        "route_turn_superseded"
    );
}

#[test]
fn parallel_actions_and_exact_step_limits_warn_and_trip() {
    let (_root, store, binding) = open();
    let (turn, _) = advance(&store, &binding, 0);
    let repeated = |index, range| {
        let mut result = batch(index, range, false);
        for call in &mut result.calls {
            call.action_hash = "same tool and arguments".into();
        }
        result
    };
    store
        .loop_admit(&binding, &turn, Some(&repeated(0, 0..4)))
        .unwrap();
    let warning = read(&store, &binding).guard;
    assert_eq!(warning.steps, 1);
    assert_eq!(warning.reason, Some(Reason::ActionPingPong));
    assert_eq!(warning.status, Status::Warned);
    assert_eq!(
        store
            .loop_admit(&binding, &turn, Some(&repeated(1, 4..6)))
            .unwrap_err()
            .code,
        CIRCUIT
    );
    store
        .loop_override(&override_input(&store, &binding))
        .unwrap();
    assert!(read(&store, &binding).guard.actions.is_empty());
    store
        .loop_admit(&binding, &turn, Some(&repeated(0, 0..4)))
        .unwrap();
    for i in 2..21 {
        store
            .loop_admit(&binding, &turn, Some(&batch(i, [i + 100], false)))
            .unwrap();
    }
    assert_eq!(
        store
            .loop_admit(&binding, &turn, Some(&batch(21, [121], false)))
            .unwrap_err()
            .code,
        CIRCUIT
    );
    let (next, _) = advance(&store, &binding, 1);
    for i in 0..44 {
        store
            .loop_admit(&binding, &next, Some(&batch(i, [i + 200], false)))
            .unwrap();
        if i == 24 {
            assert_eq!(read(&store, &binding).guard.status, Status::Warned);
            assert_eq!(
                read(&store, &binding).guard.reason,
                Some(Reason::UnattendedBurn)
            );
        }
    }
    assert_eq!(
        store
            .loop_admit(&binding, &next, Some(&batch(44, [244], false)))
            .unwrap_err()
            .code,
        CIRCUIT
    );
}

fn fill(
    store: &Store,
    binding: &str,
    turn: &Turn,
    entries: impl IntoIterator<Item = Batch>,
) -> Option<Override> {
    let mut override_receipt = None;
    for entry in entries {
        if let Err(error) = store.loop_admit(binding, turn, Some(&entry)) {
            assert_eq!(error.code, CIRCUIT);
            let input = override_input(store, binding);
            store.loop_override(&input).unwrap();
            override_receipt = Some(input);
        }
    }
    override_receipt
}

#[test]
fn both_capacity_boundaries_are_atomic_persistent_and_deny_overrides() {
    for tools in [false, true] {
        for concurrent in [false, true] {
            let (path, store, binding) = open();
            let (turn, _) = advance(&store, &binding, 0);
            let max = if tools {
                MAX_TOOL_IDS as u32
            } else {
                MAX_BATCHES as u32
            };
            let last = if concurrent { max - 1 } else { max };
            let entries: Vec<_> = if tools {
                [batch(0, 0..255, false), batch(1, 255..last, false)].to_vec()
            } else {
                (0..last)
                    .map(|index| batch(index, [index], false))
                    .collect()
            };
            let past = fill(&store, &binding, &turn, entries.iter().cloned());
            let before = read(&store, &binding).guard;
            store
                .loop_admit(&binding, &turn, Some(&entries[0]))
                .unwrap();
            assert_eq!(before.steps, read(&store, &binding).guard.steps);
            let expected_increment = usize::from(concurrent);
            let store = if concurrent {
                let writer = StoreWriter::new(store);
                let ready = Barrier::new(2);
                let result = thread::scope(|scope| {
                    [max + 1, max + 2]
                        .map(|i| {
                            let (writer, ready, binding, turn) = (&writer, &ready, &binding, &turn);
                            scope.spawn(move || {
                                ready.wait();
                                writer.lock().unwrap().loop_admit(
                                    binding,
                                    turn,
                                    Some(&batch(i, [i], false)),
                                )
                            })
                        })
                        .map(|worker| worker.join().unwrap())
                });
                assert_eq!(result.iter().filter(|result| result.is_ok()).count(), 1);
                assert_eq!(
                    result.into_iter().find_map(Result::err).unwrap().code,
                    CAPACITY
                );
                drop(writer);
                Store::open(path.path()).unwrap()
            } else {
                assert_eq!(
                    store
                        .loop_admit(
                            &binding,
                            &turn,
                            Some(&batch(max + 1, [max + 1, max + 2], false))
                        )
                        .unwrap_err()
                        .code,
                    CAPACITY
                );
                drop(store);
                Store::open(path.path()).unwrap()
            };
            let after = read(&store, &binding).guard;
            assert_eq!(after.steps, before.steps + expected_increment as u32);
            assert_eq!(after.errors, before.errors);
            assert_eq!(
                after.seen_tool_call_ids.len(),
                before.seen_tool_call_ids.len() + expected_increment
            );
            assert_eq!(
                after.seen_batch_fingerprints.len(),
                before.seen_batch_fingerprints.len() + expected_increment
            );
            assert_eq!(
                after.actions.len(),
                before.actions.len() + expected_increment
            );
            assert_eq!(after.reason, Some(Reason::DedupCapacity));
            assert_eq!(
                store
                    .loop_admit(&binding, &turn, Some(&entries[0]))
                    .unwrap_err()
                    .code,
                CAPACITY
            );
            assert_eq!(
                store
                    .loop_override(&override_input(&store, &binding))
                    .unwrap_err()
                    .code,
                CAPACITY
            );
            if let Some(past) = past {
                assert!(store.loop_override(&past).is_ok());
            }
            advance(&store, &binding, 1);
            assert!(read(&store, &binding).guard.seen_tool_call_ids.is_empty());
            assert!(
                read(&store, &binding)
                    .guard
                    .seen_batch_fingerprints
                    .is_empty()
            );
        }
    }
}

fn amounts(tokens: u64, usd_micros: Option<u64>) -> Amounts {
    Amounts {
        tokens: Some(tokens),
        usd_micros,
    }
}

fn prepare(store: &Store, binding: &str, turn: &Turn, scope: Scope, estimate: Amounts) -> Attempt {
    let request = id();
    store
        .budget_prepare_monitored(
            &Prepare {
                binding_id: binding,
                session: Some(SESSION),
                turn: &turn.digest,
                request_id: &request,
                observation_id: &request,
                provider_id: "fixture-provider",
                model: "fixture-model",
                selected_tier: Tier::Primary,
                continuation: turn.continuation,
                now: Utc::now(),
                estimate,
                fallback: None,
            },
            Some(scope),
        )
        .unwrap()
        .unwrap()
}

fn begin_send(store: &Store, attempt: &Attempt) -> Result<(), ConfigError> {
    store.service_begin_scoped_send(
        None,
        None,
        None,
        SendBoundary {
            budget_attempt: Some(&attempt.attempt_key),
            ..SendBoundary::default()
        },
    )
}

#[test]
fn settlement_after_override_uses_the_send_epoch_even_without_a_budget_policy() {
    let (root, store, binding) = open();
    let (turn, scope) = advance(&store, &binding, 0);
    let seq = scope.seq.unwrap();
    let [release, complete, reconcile, not_admitted] = std::array::from_fn(|_| {
        prepare(
            &store,
            &binding,
            &turn,
            scope,
            amounts(100_000, Some(10_000)),
        )
    });
    for attempt in [&release, &complete, &reconcile] {
        begin_send(&store, attempt).unwrap();
    }
    errors_trip(&store, &binding, &turn, 42);
    let operation = override_input(&store, &binding);
    store.loop_override(&operation).unwrap();
    begin_send(&store, &not_admitted).unwrap();
    drop(store);
    let store = Store::open(root.path()).unwrap();
    store.budget_recover().unwrap();
    let current = usage(&store.connection, &binding, SESSION, seq, 1).unwrap();
    assert_eq!(current.frozen.tokens, Some(100_000));
    assert_eq!(current.committed.tokens, None);
    store
        .budget_complete(&release.attempt_key, amounts(0, Some(0)), None)
        .unwrap();
    for _ in 0..2 {
        store
            .budget_complete(
                &complete.attempt_key,
                amounts(700_000, Some(3_100_000)),
                None,
            )
            .unwrap();
    }
    let frozen = store
        .budget_uncertain(&binding)
        .unwrap()
        .into_iter()
        .find(|a| a.attempt_key == reconcile.attempt_key)
        .unwrap();
    let review = Review {
        store_id: store.database_id().unwrap(),
        operation_id: id(),
        binding_id: binding.clone(),
        attempt_key: frozen.attempt_key,
        expected_revision: frozen.revision,
        upstream_request_id: "fixture-reconcile".into(),
        source: "fixture invoice".into(),
        tokens: Some(ReviewMetric::Settle { amount: 87 }),
        usd_micros: Some(ReviewMetric::Release),
    };
    for _ in 0..2 {
        store.budget_review(&review, "local-owner").unwrap();
    }
    let previous = usage(&store.connection, &binding, SESSION, seq, 0).unwrap();
    assert_eq!(previous.committed, amounts(700_087, Some(3_100_000)));
    assert!(previous.frozen.empty());
    assert_eq!(
        usage(&store.connection, &binding, SESSION, seq, 1)
            .unwrap()
            .totals(),
        current.totals()
    );
    assert_eq!(read(&store, &binding).guard.status, Status::Normal);
    assert_eq!(store.loop_override(&operation).unwrap().override_epoch, 1);
    let current_prepared = prepare(&store, &binding, &turn, scope, amounts(149_999, None));
    let extra = prepare(&store, &binding, &turn, scope, amounts(150_000, None));
    begin_send(&store, &current_prepared).unwrap();
    assert_eq!(begin_send(&store, &extra).unwrap_err().code, CIRCUIT);
    assert_eq!(
        read(&store, &binding).guard.reason,
        Some(Reason::UnattendedBurn)
    );
    let at_limit = usage(&store.connection, &binding, SESSION, seq, 1).unwrap();
    assert_eq!(at_limit.reserved.tokens, Some(149_999));
    assert_eq!(at_limit.usd_unknown, 1);
    assert!(
        !store
            .budget_complete(&extra.attempt_key, Amounts::default(), None)
            .unwrap()
            .sent
    );
    advance(&store, &binding, 1);
    store
        .budget_complete(
            &current_prepared.attempt_key,
            amounts(800_000, Some(6_000_000)),
            None,
        )
        .unwrap();
    assert_eq!(read(&store, &binding).guard.status, Status::Normal);
    assert_eq!(
        usage(&store.connection, &binding, SESSION, seq, 1)
            .unwrap()
            .committed
            .tokens,
        Some(800_000)
    );
}

#[test]
fn simultaneous_sends_share_the_admission_limit_and_usd_settlement_can_trip_it() {
    let (_root, store, binding) = open();
    let (turn, scope) = advance(&store, &binding, 0);
    errors_trip(&store, &binding, &turn, 0);
    store
        .loop_override(&override_input(&store, &binding))
        .unwrap();
    let attempts: [Attempt; 2] = std::array::from_fn(|_| {
        prepare(&store, &binding, &turn, scope, amounts(125_000, Some(100)))
    });
    let store = StoreWriter::new(store);
    let ready = Barrier::new(2);
    let result = thread::scope(|s| {
        attempts
            .each_ref()
            .map(|a| {
                let (store, ready) = (&store, &ready);
                s.spawn(move || {
                    ready.wait();
                    begin_send(&store.lock().unwrap(), a)
                })
            })
            .map(|w| w.join().unwrap())
    });
    assert_eq!(result.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        result.into_iter().find_map(Result::err).unwrap().code,
        CIRCUIT
    );
    let store = store.lock().unwrap();
    let current = read(&store, &binding);
    assert_eq!(current.guard.reason, Some(Reason::UnattendedBurn));
    assert_eq!(
        usage(&store.connection, &binding, SESSION, current.seq, 1)
            .unwrap()
            .reserved
            .tokens,
        Some(125_000)
    );
    store
        .loop_override(&override_input(&store, &binding))
        .unwrap();
    let next = prepare(&store, &binding, &turn, scope, amounts(100, None));
    begin_send(&store, &next).unwrap();
    let settled = store
        .budget_complete(&next.attempt_key, amounts(20, Some(1_500_000)), None)
        .unwrap();
    assert_eq!(settled.state, AttemptState::Committed);
    assert_eq!(
        read(&store, &binding).guard.reason,
        Some(Reason::UnattendedBurn)
    );
    assert_eq!(
        usage(&store.connection, &binding, SESSION, current.seq, 2)
            .unwrap()
            .usd_unknown,
        0
    );
}

#[test]
fn opencode_auxiliary_title_does_not_anchor_the_user_turn() {
    let (_root, store, _) = open();
    let mut provider = store.providers().unwrap().remove(0);
    provider.endpoint = "http://127.0.0.1:32000".into();
    provider.configured = true;
    provider.protocol = "messages".into();
    provider.model = "fixture".into();
    provider.models = vec![provider.model.clone()];
    let id = store
        .dynamic_create("opencode", "owned-title", &id(), &provider)
        .unwrap()
        .id;
    let session = "opencode:ses_fixture";
    let mut title = next_turn(0);
    title.session = Some(session.into());
    title.digest = "title-request".into();
    title.chain = vec!["auxiliary request wrapper".into()];
    let admit_title = || {
        store.gateway_admit_request(
            &id,
            &title,
            super::super::GatewayAdmission {
                compatible: &[],
                opencode_proof: None,
                model: None,
                requested_output: None,
                auxiliary_title: true,
            },
        )
    };
    let (selection, _) = admit_title().unwrap();
    assert_eq!(selection.reason, "auxiliary_title");
    assert!(!selection.advances_session_head);
    assert!(head(&store.connection, &id, session).unwrap().is_none());
    let mut user = next_turn(0);
    user.session = Some(session.into());
    assert!(
        store
            .dynamic_admit(&id, &user)
            .unwrap()
            .advances_session_head
    );
    let seq = store
        .loop_admit_auxiliary(&id, Some(session))
        .unwrap()
        .unwrap()
        .seq;
    assert_eq!(
        seq,
        Some(head(&store.connection, &id, session).unwrap().unwrap().seq)
    );
    user.continuation = true;
    for index in 0..5 {
        let result = store.loop_admit(&id, &user, Some(&batch(index, [index], true)));
        if index < 4 {
            result.unwrap();
        } else {
            assert_eq!(result.unwrap_err().code, CIRCUIT);
        }
    }
    assert_eq!(admit_title().err().unwrap().code, CIRCUIT);
    assert_eq!(
        head(&store.connection, &id, session).unwrap().unwrap().seq,
        seq.unwrap()
    );
}
