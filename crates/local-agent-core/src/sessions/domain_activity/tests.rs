use super::*;
use crate::sessions::{
    activity_input::UserInput,
    activity_performance::{ActivitySampling, MessageUsage, RateSample},
    live::NativeRealtime,
};
use std::cell::Cell;

const EVENT: &str = "2026-10-31T02:00:00Z";
const OBSERVED: &str = "2026-10-31T02:00:02Z";
fn activity(agent: &str) -> NativeActivity {
    NativeActivity {
        message_usage: Some(MessageUsage {
            output_tokens: 900,
            reasoning_tokens: Some(700),
            at: EVENT.into(),
        }),
        turn_id: Some("turn-1".into()),
        call_id: Some("private-call-id".into()),
        agent_product_id: Some(
            if agent == "claude" {
                "claude-code"
            } else {
                "codex-desktop"
            }
            .into(),
        ),
        agent_attribution: Some("native_record".into()),
        id: format!("{agent}:s:message:turn-1"),
        agent_id: agent.into(),
        session_id: format!("{agent}:s"),
        title: "activity title".into(),
        project: Some("/private/managed-project".into()),
        model: Some("fixture-model".into()),
        phase: "thinking".into(),
        step: "正在思考".into(),
        user_input: Some(UserInput {
            text: "redacted user excerpt".into(),
            at: EVENT.into(),
        }),
        tool_names: vec!["read".into()],
        message_started_at: Some(EVENT.into()),
        event_at: EVENT.into(),
        turn_ttft_ms: Some(12.25),
        realtime: Some(NativeRealtime {
            sampling: Some(ActivitySampling {
                at: 1793412002000,
                window_ms: 5000,
                tokens_per_second: Some(42.5),
                recent: Some(RateSample {
                    at: 1793412001000,
                    window_ms: 5000,
                    tokens_per_second: 41.0,
                }),
                reported: None,
            }),
            state: "connected".into(),
            first_token_ms: Some(10.75),
            first_token_missed: false,
            first_text_ms: Some(12.5),
            first_output_ms: Some(10.75),
            text_tokens_per_second: Some(40.0),
            output_tokens_per_second: Some(42.5),
            average_text_tokens_per_second: Some(21.0),
        }),
        stale: false,
    }
}
fn request(context: &SourceContext<'_>, agent: &str) -> a::ActivityRequest {
    let provider = d::Provider {
        provider_key: format!("builtin:domain.activity-source@1:{agent}"),
        plugin_id: format!("native.{agent}"),
        implementation_version: "0.1.8".into(),
        implementation_identity: "fixture-build".into(),
        capability: d::Capability {
            name: "domain.activity-source".into(),
            major: 1,
            instance_id: agent.into(),
        },
        execution_kind: d::ExecutionKind::Builtin,
    };
    a::ActivityRequest {
        invocation: context.domain_invocation(&provider),
        source_id: agent.into(),
        observed_at: OBSERVED.into(),
    }
}
fn reader<'a>(
    context: &'a SourceContext<'a>,
    expected: a::ActivityRequest,
    broker: &'a mut dyn FnMut() -> NativeResult<Vec<NativeActivity>>,
) -> NativeRead<'a> {
    NativeRead {
        context,
        expected,
        broker,
        private: HashMap::new(),
        collection_micros: 1234,
        limited: false,
        error: None,
    }
}
fn sessions(home: &tempfile::TempDir) -> NativeSessions {
    NativeSessions::new(
        home.path().canonicalize().unwrap(),
        &["claude".into(), "codex".into()],
    )
}

#[test]
fn common_activity_result_preserves_legacy_fields_and_separates_clocks_and_private_evidence() {
    let home = tempfile::tempdir().unwrap();
    let native = sessions(&home);
    native
        .with_source_context("codex", None, true, |_, context| {
            let expected = request(context, "codex");
            let row = activity("codex");
            let json = serde_json::to_value(&row).unwrap();
            let mut broker = || Ok(vec![row.clone()]);
            let mut reader = reader(context, expected.clone(), &mut broker);
            let public = reader.collect_activity(&expected).unwrap();
            assert_eq!(public.observed_at, OBSERVED);
            assert_eq!(public.activities[0].event_at, EVENT);
            assert_eq!(public.collection_micros.value(), 1234);
            let encoded = serde_json::to_string(&public).unwrap();
            for private in [
                "/private/managed-project",
                "private-call-id",
                "tokensPerSecond",
                "firstTokenMs",
                "messageUsage",
                "turnTtftMs",
            ] {
                assert!(!encoded.contains(private));
            }
            assert!(encoded.contains("\"collectionMicros\":\"1234\""));
            let result = reader.consume(public).unwrap();
            assert_eq!(serde_json::to_value(&result[0]).unwrap(), json);
            assert_eq!(result[0].call_id.as_deref(), Some("private-call-id"));
            Ok(())
        })
        .unwrap();
}

#[test]
fn activity_consumer_uses_public_business_fields_instead_of_returning_the_original_row() {
    let home = tempfile::tempdir().unwrap();
    let native = sessions(&home);
    native
        .with_source_context("claude", None, true, |_, context| {
            let expected = request(context, "claude");
            let mut broker = || Ok(vec![activity("claude")]);
            let mut reader = reader(context, expected.clone(), &mut broker);
            let mut public = reader.collect_activity(&expected).unwrap();
            public.activities[0].title = "replacement title".into();
            public.activities[0].phase = "waiting_input".into();
            public.activities[0].step = "等待输入".into();
            public.activities[0].tool_names = vec!["different-tool".into()];
            public.activities[0].stale = true;
            let result = reader.consume(public).unwrap();
            assert_eq!(result[0].title, "replacement title");
            assert_eq!(result[0].phase, "waiting_input");
            assert_eq!(result[0].step, "等待输入");
            assert_eq!(result[0].tool_names, ["different-tool"]);
            assert!(result[0].stale);
            assert_eq!(result[0].message_usage.as_ref().unwrap().output_tokens, 900);
            Ok(())
        })
        .unwrap();
}

#[test]
fn activity_performance_and_paths_cannot_be_rebound_to_other_events_turns_or_products() {
    let home = tempfile::tempdir().unwrap();
    let native = sessions(&home);
    native
        .with_source_context("codex", None, true, |_, context| {
            for mutation in 0..9 {
                let expected = request(context, "codex");
                let mut broker = || Ok(vec![activity("codex")]);
                let mut reader = reader(context, expected.clone(), &mut broker);
                let mut public = reader.collect_activity(&expected).unwrap();
                match mutation {
                    0 => public.activities[0].source_id = "claude".into(),
                    1 => public.activities[0].session_id = "codex:other".into(),
                    2 => public.activities[0].message_id = Some("turn-2".into()),
                    3 => public.activities[0].message_started_at = Some(OBSERVED.into()),
                    4 => public.activities[0].event_at = OBSERVED.into(),
                    5 => public.activities[0].id = "other".into(),
                    6 => public.activities[0].product_id = Some("codex-cli".into()),
                    7 => public.activities[0].attribution = Some("guessed".into()),
                    _ => public.collection_micros = 999999u64.into(),
                }
                assert!(reader.consume(public).is_err(), "mutation {mutation}");
            }
            Ok(())
        })
        .unwrap();
}

#[test]
fn forged_activity_invocation_never_calls_the_sampler() {
    let home = tempfile::tempdir().unwrap();
    let native = sessions(&home);
    native
        .with_source_context("claude", None, true, |_, context| {
            let expected = request(context, "claude");
            let calls = Cell::new(0);
            let mut broker = || {
                calls.set(calls.get() + 1);
                Ok(vec![])
            };
            let mut reader = reader(context, expected.clone(), &mut broker);
            for mutation in 0..5 {
                let mut forged = expected.clone();
                match mutation {
                    0 => forged.source_id = "codex".into(),
                    1 => forged.observed_at = EVENT.into(),
                    2 => forged.invocation.epoch = 999u64.into(),
                    3 => forged.invocation.resource_ref = "other-resource".into(),
                    _ => forged.invocation.implementation_identity = "other-build".into(),
                }
                assert_eq!(
                    reader.collect_activity(&forged).unwrap_err().code,
                    d::ErrorCode::StaleSnapshot
                );
            }
            assert_eq!(calls.get(), 0);
            Ok(())
        })
        .unwrap();
}

#[test]
fn revocation_during_activity_broker_never_publishes_rows_or_private_measurements() {
    let home = tempfile::tempdir().unwrap();
    let native = sessions(&home);
    let result = native.with_source_context("claude", None, true, |_, context| {
        let expected = request(context, "claude");
        let mut broker = || {
            native.set_enabled("claude", false)?;
            Ok(vec![activity("claude")])
        };
        let mut reader = reader(context, expected.clone(), &mut broker);
        assert_eq!(
            reader.collect_activity(&expected).unwrap_err().code,
            d::ErrorCode::GrantRevoked
        );
        assert!(reader.private.is_empty());
        Ok(())
    });
    assert_eq!(result.unwrap_err().code, "unauthorized");
}

#[test]
fn scope_lease_rejects_stop_and_remount_with_unchanged_consent_while_other_origins_continue() {
    let home = tempfile::tempdir().unwrap();
    let mut native = sessions(&home);
    let captured = scopes(&native).unwrap();
    native.stop_source_plugins().unwrap();
    native.source_plugins = super::super::source_plugins::NativeSourcePlugins::builtin().unwrap();
    let (rows, failed) = publish(
        &native,
        &captured,
        vec![activity("claude"), activity("codex")],
        OBSERVED,
        1,
        false,
    );
    assert!(failed);
    assert!(rows.is_empty());
    let captured = scopes(&native).unwrap();
    native.set_enabled("claude", false).unwrap();
    let (rows, failed) = publish(
        &native,
        &captured,
        vec![activity("claude"), activity("codex")],
        OBSERVED,
        1,
        false,
    );
    assert!(failed);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].agent_id, "codex");
}

#[test]
fn duplicate_oversized_or_numeric_claims_fail_activity_contract() {
    let home = tempfile::tempdir().unwrap();
    let native = sessions(&home);
    native
        .with_source_context("claude", None, true, |_, context| {
            let expected = request(context, "claude");
            let mut broker = || Ok(vec![activity("claude")]);
            let mut reader = reader(context, expected.clone(), &mut broker);
            let public = reader.collect_activity(&expected).unwrap();
            let mut duplicate = public.clone();
            duplicate.activities.push(duplicate.activities[0].clone());
            assert_eq!(
                duplicate.validate(&expected).unwrap_err().code,
                d::ErrorCode::InvalidInput
            );
            let mut bounded = expected.clone();
            bounded.invocation.budget.max_records = 0u64.into();
            assert_eq!(
                public.validate(&bounded).unwrap_err().code,
                d::ErrorCode::ResourceLimit
            );
            let mut json = serde_json::to_value(&public).unwrap();
            json["activities"][0]["tokensPerSecond"] = serde_json::json!(999);
            assert!(serde_json::from_value::<a::ActivitySnapshot>(json).is_err());
            let mut json = serde_json::to_value(&public).unwrap();
            json["collectionMicros"] = serde_json::json!(1234);
            assert!(serde_json::from_value::<a::ActivitySnapshot>(json).is_err());
            Ok(())
        })
        .unwrap();
}
