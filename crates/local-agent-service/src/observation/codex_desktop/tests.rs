use super::{
    Frame, following, frame,
    metrics::{Metrics, Snapshot, Target, TextTotals},
    timings::Timings,
};
use serde_json::{Value, json};
use std::collections::HashMap;

const SESSION: &str = "11111111-1111-4111-8111-111111111111";
const TURN: &str = "22222222-2222-4222-8222-222222222222";
const NEXT: &str = "33333333-3333-4333-8333-333333333333";

#[test]
fn five_second_rates_expire_but_last_sample_time_and_usage_survive_restart_per_message() {
    use local_agent_core::sessions::activity_performance::MessageUsage;
    let mut m = initialized(false);
    m.patches(&delta(11, TURN, "abcdefghijklmnopqrst"), &targets(), 1500);
    let early = m.measurement(SESSION, TURN, 3500).0;
    assert!(early.output_tokens_per_second.is_none());
    assert_eq!(
        early.sampling.as_ref().unwrap().tokens_per_second,
        Some(1.0)
    );
    let mut late = m.measurement(SESSION, TURN, 6500).0;
    assert!(late.sampling.as_ref().unwrap().tokens_per_second.is_none());
    let recent = late.sampling.as_ref().unwrap().recent.clone().unwrap();
    assert_eq!(recent.at, 1500);
    late.sampling.as_mut().unwrap().reported = Some(MessageUsage {
        output_tokens: 900,
        reasoning_tokens: Some(700),
        at: "1970-01-01T00:00:02Z".into(),
    });
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().canonicalize().unwrap().join("timings.json");
    let mut saved = Timings::open(path.clone());
    saved.apply(SESSION, TURN, Some(1000), &mut late, None);
    saved.commit().unwrap();
    let mut saved = Timings::open(path);
    let mut cold = Metrics::default().measurement(SESSION, TURN, 10000).0;
    saved.apply(SESSION, TURN, None, &mut cold, None);
    let sampling = cold.sampling.unwrap();
    assert!(sampling.tokens_per_second.is_none());
    assert_eq!(sampling.recent.unwrap(), recent);
    assert_eq!(sampling.reported.unwrap().reasoning_tokens, Some(700));
    assert_eq!(cold.first_token_ms, early.first_token_ms);
    let mut next = Metrics::default().measurement(SESSION, NEXT, 10000).0;
    saved.apply(SESSION, NEXT, Some(9000), &mut next, None);
    assert!(next.sampling.as_ref().unwrap().recent.is_none());
    assert!(next.sampling.unwrap().reported.is_none());
}

#[test]
fn tools_stop_current_sampling_but_keep_the_explicit_historical_sample() {
    let mut m = initialized(false);
    m.patches(
        &reasoning(11, "content", "abcdefghijklmnopqrst"),
        &targets(),
        1500,
    );
    let mut tool = delta(12, TURN, "");
    tool["params"]["change"]["patches"] = json!([{"op":"add","path":["turnHistory","history","entitiesByKey",format!("tail:0:local:{TURN}"),"items",1],"value":{"type":"mcpToolCall","status":"inProgress","aggregatedOutput":"NOT TOKENS"}}]);
    m.patches(&tool, &targets(), 2000);
    let sample = m.measurement(SESSION, TURN, 2100).0.sampling.unwrap();
    assert!(sample.tokens_per_second.is_none());
    assert_eq!(sample.recent.unwrap().at, 1500);
    m.disconnected("reconnecting");
    let sample = m.measurement(SESSION, TURN, 2300).0.sampling.unwrap();
    assert!(sample.tokens_per_second.is_none());
    assert_eq!(sample.recent.unwrap().at, 1500);
}

#[test]
fn actual_model_output_is_sampled_while_an_asynchronous_tool_is_still_running() {
    let mut m = initialized(false);
    let mut tool = delta(11, TURN, "");
    tool["params"]["change"]["patches"] = json!([{"op":"add","path":["turnHistory","history","entitiesByKey",format!("tail:0:local:{TURN}"),"items",1],"value":{"type":"mcpToolCall","status":"inProgress"}}]);
    m.patches(&tool, &targets(), 1500);
    m.patches(&delta(12, TURN, "abcdefghijklmnopqrst"), &targets(), 2000);
    let measured = m.measurement(SESSION, TURN, 2100).0;
    assert_eq!(measured.sampling.unwrap().tokens_per_second, Some(1.0));
}
fn targets() -> HashMap<String, Target> {
    HashMap::from([(
        SESSION.into(),
        Target {
            turn: TURN.into(),
            started: 1000,
        },
    )])
}
fn snapshot(prior: bool) -> Value {
    json!({"type":"broadcast","method":"thread-stream-state-changed","sourceClientId":"owner","version":11,
    "params":{"conversationId":SESSION,"hostId":"local","change":{"type":"snapshot","revision":10,
        "conversationState":{"turnHistory":{"history":{"entitiesByKey":{
                format!("tail:0:local:{TURN}"):{"turnId":TURN,"turnStartedAtMs":1000,"firstTurnWorkItemStartedAtMs":1200,
                    "params":{"model":"native-test","input":[{"type":"text","text":"继续验证 Bearer abcdefghijklmnop"}]},
                "aeonAssistantMessageStartedAtMsById":if prior {json!({"msg_previous":1200})} else {json!({})},
                "items":[{"type":"commandExecution","aggregatedOutput":"PRIVATE TOOL OUTPUT"}],"status":"inProgress"}
        }}}}}}})
}
fn initialized(prior: bool) -> Metrics {
    let mut m = Metrics::default();
    m.owners.insert(SESSION.into(), "owner".into());
    m.snapshot(
        serde_json::from_value(snapshot(prior)).unwrap(),
        &targets(),
        1300,
    );
    m
}

fn task_params(text: &str) -> Value {
    json!({"input":[],"toolOutput":{"namespace":"codex_app","name":"send_message_to_thread","output":format!("<codex_delegation><source_thread_id>{SESSION}</source_thread_id><input>{text}</input></codex_delegation>")}})
}

#[test]
fn desktop_task_input_metadata_is_redacted_and_never_counts_as_model_output() {
    let mut value = snapshot(false);
    value["params"]["change"]["conversationState"]["turnHistory"]["history"]["entitiesByKey"]
        [format!("tail:0:local:{TURN}")]["params"] =
        task_params("第一条任务 Bearer abcdefghijklmnop");
    let mut m = Metrics::default();
    m.owners.insert(SESSION.into(), "owner".into());
    m.snapshot(
        serde_json::from_value(value.clone()).unwrap(),
        &targets(),
        1300,
    );
    assert_eq!(m.metadata(SESSION, TURN).1, Some("第一条任务 [已隐藏凭据]"));
    let v = m.measurement(SESSION, TURN, 1400).0;
    assert!(
        v.first_token_ms.is_none()
            && v.output_tokens_per_second.is_none()
            && v.average_text_tokens_per_second.is_none()
    );

    value["params"]["change"]["conversationState"]["turnHistory"]["history"]["entitiesByKey"]
        [format!("tail:0:local:{TURN}")]["params"]["toolOutput"]["namespace"] = json!("shell");
    let mut unknown = Metrics::default();
    unknown.owners.insert(SESSION.into(), "owner".into());
    unknown.snapshot(serde_json::from_value(value).unwrap(), &targets(), 1300);
    assert!(unknown.metadata(SESSION, TURN).1.is_none());
}

#[test]
fn new_desktop_input_patches_keep_each_native_message_preview_separate() {
    let mut m = initialized(false);
    let old = m.metadata(SESSION, TURN).1.unwrap().to_owned();
    let mut v = delta(11, NEXT, "");
    v["params"]["change"]["patches"] = json!([{"op":"add","path":["turnHistory","history","entitiesByKey",format!("tail:1:local:{NEXT}")],"value":{"turnId":NEXT,"turnStartedAtMs":5000,"params":task_params("第二条独立输入")}}]);
    m.patches(&v, &targets(), 5200);
    assert_eq!(m.metadata(SESSION, NEXT).1, Some("第二条独立输入"));
    assert_eq!(m.metadata(SESSION, TURN).1, Some(old.as_str()));
    assert!(
        m.measurement(SESSION, NEXT, 5200)
            .0
            .first_token_ms
            .is_none()
    );

    let mut v = delta(12, NEXT, "");
    v["params"]["change"]["patches"] = json!([{"op":"replace","path":["turnHistory","history","entitiesByKey",format!("tail:1:local:{NEXT}"),"params","toolOutput"],"value":task_params("第二条更新输入")["toolOutput"]}]);
    m.patches(&v, &targets(), 5300);
    assert_eq!(m.metadata(SESSION, NEXT).1, Some("第二条更新输入"));
    assert_eq!(m.metadata(SESSION, TURN).1, Some(old.as_str()));
    assert!(
        m.measurement(SESSION, NEXT, 5300)
            .0
            .output_tokens_per_second
            .is_none()
    );
}
fn delta(revision: u64, turn: &str, text: &str) -> Value {
    json!({"type":"broadcast","method":"thread-stream-state-changed","sourceClientId":"owner","version":11,
        "params":{"conversationId":SESSION,"hostId":"local","change":{"type":"patches","baseRevision":revision-1,"revision":revision,"patches":[],
            "acceptedTextChanges":[{"key":{"hostId":"local","threadId":SESSION,"entityKey":format!("tail:0:local:{turn}"),"itemId":"msg_live"},"target":{"field":"text"},"edits":[{"at":0,"deleteCount":0,"insert":text}]}]}}})
}
#[test]
fn live_text_and_first_text_are_measured_per_message_without_snapshot_replay() {
    let mut m = initialized(false);
    m.patches(&delta(11, TURN, "abcd"), &targets(), 1500);
    let mut next = delta(12, TURN, "中文");
    next["params"]["change"]["acceptedTextChanges"][0]["edits"][0]["at"] = json!(4);
    m.patches(&next, &targets(), 2000);
    let (v, phase) = m.measurement(SESSION, TURN, 2100);
    assert_eq!(v.first_text_ms, Some(500.0));
    assert_eq!(v.first_token_ms, Some(500.0));
    assert_eq!(v.first_output_ms, Some(200.0));
    assert_eq!(v.text_tokens_per_second, Some(3.0));
    assert_eq!(v.average_text_tokens_per_second, Some(6.0));
    assert_eq!(phase, Some(("streaming", 2000)));
    assert!(m.measurement(SESSION, NEXT, 2100).0.first_text_ms.is_none());
    assert_eq!(
        m.measurement(SESSION, TURN, 3100).0.text_tokens_per_second,
        None
    );
}
#[test]
fn joining_mid_turn_does_not_invent_first_token_or_count_history_as_streamed_text() {
    let mut m = initialized(true);
    assert!(
        m.measurement(SESSION, TURN, 1300)
            .0
            .text_tokens_per_second
            .is_none()
    );
    m.patches(&delta(11, TURN, "abcd"), &targets(), 1500);
    assert!(m.measurement(SESSION, TURN, 1500).0.first_text_ms.is_none());
    assert!(
        m.measurement(SESSION, TURN, 1500)
            .0
            .first_token_ms
            .is_none()
    );
    assert!(m.measurement(SESSION, TURN, 1500).0.first_token_missed);
    assert_eq!(
        m.measurement(SESSION, TURN, 1500).0.text_tokens_per_second,
        Some(1.0)
    );
}
#[test]
fn gaps_wrong_owner_wrong_thread_corrections_and_other_protocol_versions_are_not_samples() {
    let mut m = initialized(false);
    let mut v = delta(11, TURN, "abcd");
    v["sourceClientId"] = json!("stranger");
    m.patches(&v, &targets(), 1500);
    assert!(
        m.measurement(SESSION, TURN, 1500)
            .0
            .text_tokens_per_second
            .is_none()
    );
    v = delta(11, TURN, "abcd");
    v["params"]["change"]["acceptedTextChanges"][0]["key"]["threadId"] = json!("other");
    m.patches(&v, &targets(), 1500);
    assert!(
        m.measurement(SESSION, TURN, 1500)
            .0
            .text_tokens_per_second
            .is_none()
    );
    v = delta(12, TURN, "abcd");
    v["params"]["change"]["acceptedTextChanges"][0]["edits"][0]["deleteCount"] = json!(1);
    m.patches(&v, &targets(), 1500);
    assert!(
        m.measurement(SESSION, TURN, 1500)
            .0
            .text_tokens_per_second
            .is_none()
    );
    assert_eq!(
        m.patches(&delta(14, TURN, "abcd"), &targets(), 1600),
        Some(SESSION.into())
    );
    assert!(
        m.measurement(SESSION, TURN, 1600)
            .0
            .text_tokens_per_second
            .is_none()
    );
    let mut bad = snapshot(false);
    bad["version"] = json!(12);
    m.snapshot(serde_json::from_value(bad).unwrap(), &targets(), 1700);
    assert_eq!(m.measurement(SESSION, TURN, 1700).0.state, "incompatible");
}
#[test]
fn tool_phase_and_disconnect_clear_live_rate_but_preserve_recorded_first_text() {
    let mut m = initialized(false);
    m.patches(&delta(11, TURN, "abcd"), &targets(), 1500);
    let mut v = delta(12, TURN, "");
    v["params"]["change"]["patches"] = json!([{"op":"add","path":["turnHistory","history","entitiesByKey",format!("tail:0:local:{TURN}"),"items",1],"value":{"type":"mcpToolCall","status":"inProgress","arguments":{"secret":"NOT RETAINED"}}}]);
    m.patches(&v, &targets(), 1600);
    assert!(
        m.measurement(SESSION, TURN, 1600)
            .0
            .text_tokens_per_second
            .is_none()
    );
    assert_eq!(
        m.measurement(SESSION, TURN, 1600).1,
        Some(("tool_requested", 1600))
    );
    m.disconnected("reconnecting");
    let v = m.measurement(SESSION, TURN, 1700).0;
    assert_eq!(v.first_text_ms, Some(500.0));
    assert_eq!(v.state, "reconnecting");
    assert!(v.text_tokens_per_second.is_none());
}
#[test]
fn large_snapshots_are_projected_and_wire_frames_are_bounded_and_fragment_safe() {
    let mut v = snapshot(false);
    v["params"]["change"]["conversationState"]["ignoredToolOutput"] =
        json!("x".repeat(1024 * 1024));
    let data = serde_json::to_vec(&v).unwrap();
    let mut input = (data.len() as u32).to_le_bytes().to_vec();
    input.extend(data);
    let Frame::Snapshot(v) = frame(&mut &input[..]).unwrap() else {
        panic!("must use projection")
    };
    let mut m = Metrics::default();
    m.owners.insert(SESSION.into(), "owner".into());
    m.snapshot(v, &targets(), 1300);
    assert_eq!(
        m.measurement(SESSION, TURN, 1300).0.first_output_ms,
        Some(200.0)
    );
    assert!(frame(&mut &0u32.to_le_bytes()[..]).is_err());
    assert!(frame(&mut &(268_435_457u32).to_le_bytes()[..]).is_err());
    assert!(frame(&mut &input[..input.len() - 1]).is_err());
    let _: Snapshot = serde_json::from_value(snapshot(true)).unwrap();
}
#[test]
fn follower_messages_only_subscribe_and_unsubscribe() {
    for value in [true, false] {
        let message = following("client", SESSION, "owner", value);
        assert_eq!(message["method"], "thread-stream-following-changed");
        assert_eq!(message["params"]["following"], value);
        assert_eq!(message["targetClientIds"], json!(["owner"]));
    }
}

fn reasoning(revision: u64, field: &str, text: &str) -> Value {
    let mut v = delta(revision, TURN, text);
    let change = &mut v["params"]["change"]["acceptedTextChanges"][0];
    change["key"]["itemId"] = json!("rs_reasoning");
    change["target"] = json!({"field":field,"index":0});
    v
}

#[test]
fn reasoning_streams_before_body_and_tool_execution_keeps_ttft_without_counting_results() {
    let mut m = initialized(false);
    m.patches(&reasoning(11, "summary", "abcd"), &targets(), 1500);
    let mut second = reasoning(12, "summary", "中文");
    second["params"]["change"]["acceptedTextChanges"][0]["edits"][0]["at"] = json!(4);
    m.patches(&second, &targets(), 2000);
    let v = m.measurement(SESSION, TURN, 2100).0;
    assert_eq!(v.output_tokens_per_second, Some(3.0));
    assert_eq!(
        serde_json::to_value(&v).unwrap()["outputTokensPerSecond"],
        3.0
    );
    assert_eq!(v.first_token_ms, Some(500.0));
    assert!(v.first_text_ms.is_none() && v.text_tokens_per_second.is_none());
    assert!(v.average_text_tokens_per_second.is_none());
    for (revision, index) in [(13, 1), (14, 2)] {
        let mut tool = delta(revision, TURN, "");
        tool["params"]["change"]["patches"] = json!([{"op":"add","path":["turnHistory","history","entitiesByKey",format!("tail:0:local:{TURN}"),"items",index],"value":{"type":"mcpToolCall","status":"inProgress","arguments":{"private":"DO NOT COUNT"},"aggregatedOutput":"DO NOT COUNT"}}]);
        m.patches(&tool, &targets(), 2200);
    }
    assert_eq!(
        m.measurement(SESSION, TURN, 2300)
            .0
            .output_tokens_per_second,
        Some(3.0)
    );
    assert_eq!(
        m.measurement(SESSION, TURN, 3300)
            .0
            .output_tokens_per_second,
        None
    );
    for (revision, index, phase) in [(15, 1, "tool_requested"), (16, 2, "tool_returned")] {
        let mut returned = delta(revision, TURN, "");
        returned["params"]["change"]["patches"] = json!([{"op":"replace","path":["turnHistory","history","entitiesByKey",format!("tail:0:local:{TURN}"),"items",index,"status"],"value":"completed"}]);
        let change = &mut returned["params"]["change"]["acceptedTextChanges"][0];
        change["target"] = json!({"field":"aggregatedOutput"});
        change["edits"][0]["insert"] = json!("TOOL OUTPUT IS NOT MODEL OUTPUT");
        m.patches(&returned, &targets(), 3400);
        let (v, p) = m.measurement(SESSION, TURN, 3500);
        assert_eq!(p, Some((phase, 3400)));
        assert_eq!(v.output_tokens_per_second, None);
        assert_eq!(v.first_token_ms, Some(500.0));
    }
    m.disconnected("reconnecting");
    assert!(
        m.measurement(SESSION, TURN, 3600)
            .0
            .output_tokens_per_second
            .is_none()
    );
    assert_eq!(
        m.measurement(SESSION, TURN, 3600).0.first_token_ms,
        Some(500.0)
    );
}

#[test]
fn native_silence_and_usage_settlements_do_not_invent_live_reasoning_speed() {
    let mut m = initialized(false);
    m.patches(&reasoning(11, "summary", "abcd"), &targets(), 1500);
    let v = m.measurement(SESSION, TURN, 2500).0;
    assert_eq!(v.output_tokens_per_second, Some(1.0));
    let v = m.measurement(SESSION, TURN, 2501).0;
    assert_eq!(v.output_tokens_per_second, None);
    assert_eq!(v.first_token_ms, Some(500.0));
    let mut settlement = delta(12, TURN, "");
    settlement["params"]["change"]["patches"] = json!([{
        "op":"replace", "path":["latestTokenUsageInfo"],
        "value":{"total":{"outputTokens":5000,"reasoningOutputTokens":3000},
            "last":{"outputTokens":1000,"reasoningOutputTokens":600}}
    }]);
    m.patches(&settlement, &targets(), 2600);
    let (v, phase) = m.measurement(SESSION, TURN, 2700);
    assert_eq!(phase, Some(("thinking", 1500)));
    assert_eq!(v.output_tokens_per_second, None);
    assert_eq!(v.first_token_ms, Some(500.0));
    let mut next = reasoning(13, "summary", "中文");
    next["params"]["change"]["acceptedTextChanges"][0]["edits"][0]["at"] = json!(4);
    m.patches(&next, &targets(), 2800);
    assert_eq!(
        m.measurement(SESSION, TURN, 2900)
            .0
            .output_tokens_per_second,
        Some(2.0)
    );
    assert_eq!(
        m.measurement(SESSION, NEXT, 2900)
            .0
            .output_tokens_per_second,
        None
    );
}

#[test]
fn live_empty_then_filled_reasoning_items_sample_once_without_snapshot_or_splice_replay() {
    let item = |revision, op, summary: Value| {
        let mut v = delta(revision, TURN, "");
        v["params"]["change"]["patches"] = json!([{"op":op,"path":["turnHistory","history","entitiesByKey",format!("tail:0:local:{TURN}"),"items",0],"value":{"type":"reasoning","id":"rs_batch","summary":summary,"content":[]}}]);
        v
    };
    let mut m = initialized(false);
    m.patches(&item(11, "add", json!([])), &targets(), 1400);
    assert!(
        m.measurement(SESSION, TURN, 1400)
            .0
            .output_tokens_per_second
            .is_none()
    );
    m.patches(&item(12, "replace", json!(["abcd"])), &targets(), 1500);
    assert_eq!(
        m.measurement(SESSION, TURN, 1600)
            .0
            .output_tokens_per_second,
        Some(1.0)
    );
    m.patches(
        &item(13, "replace", json!(["rewritten content"])),
        &targets(),
        1700,
    );
    assert_eq!(
        m.measurement(SESSION, TURN, 1800)
            .0
            .output_tokens_per_second,
        Some(1.0)
    );
    assert!(
        m.measurement(SESSION, TURN, 1800)
            .0
            .average_text_tokens_per_second
            .is_none()
    );
    let mut joined = initialized(true);
    joined.patches(&item(11, "replace", json!(["old body"])), &targets(), 1500);
    assert!(
        joined
            .measurement(SESSION, TURN, 1600)
            .0
            .output_tokens_per_second
            .is_none()
    );
    let mut streamed = initialized(false);
    streamed.patches(&item(11, "add", json!([])), &targets(), 1400);
    let mut filled = item(12, "replace", json!(["abcd"]));
    let mut splice = reasoning(12, "summary", "abcd");
    splice["params"]["change"]["acceptedTextChanges"][0]["key"]["itemId"] = json!("rs_batch");
    filled["params"]["change"]["acceptedTextChanges"] =
        splice["params"]["change"]["acceptedTextChanges"].clone();
    streamed.patches(&filled, &targets(), 1500);
    assert_eq!(
        streamed
            .measurement(SESSION, TURN, 1600)
            .0
            .output_tokens_per_second,
        Some(1.0)
    );
}

#[test]
fn native_reasoning_uses_utf16_offsets_and_prefers_content_without_double_counting_summary() {
    let mut m = initialized(false);
    m.patches(&reasoning(11, "summary", "😀"), &targets(), 1500);
    m.patches(&reasoning(12, "content", "😀"), &targets(), 1600);
    assert_eq!(
        m.measurement(SESSION, TURN, 1700)
            .0
            .output_tokens_per_second,
        Some(0.5)
    );
    let mut ignored = reasoning(13, "summary", "summary");
    ignored["params"]["change"]["acceptedTextChanges"][0]["edits"][0]["at"] = json!(2);
    m.patches(&ignored, &targets(), 1800);
    let mut text = reasoning(14, "content", "x");
    text["params"]["change"]["acceptedTextChanges"][0]["edits"][0]["at"] = json!(2);
    m.patches(&text, &targets(), 1900);
    assert_eq!(
        m.measurement(SESSION, TURN, 2000)
            .0
            .output_tokens_per_second,
        Some(0.75)
    );
    text["params"]["change"]["baseRevision"] = json!(14);
    text["params"]["change"]["revision"] = json!(15);
    text["params"]["change"]["acceptedTextChanges"][0]["edits"][0]["at"] = json!(99);
    m.patches(&text, &targets(), 2100);
    assert!(
        m.measurement(SESSION, TURN, 2200)
            .0
            .output_tokens_per_second
            .is_none()
    );
    assert_eq!(
        m.measurement(SESSION, TURN, 2200).0.first_token_ms,
        Some(500.0)
    );
}

#[test]
fn late_join_and_revision_gaps_never_replay_reasoning_or_carry_rates_into_the_next_message() {
    let mut m = initialized(true);
    assert!(
        m.measurement(SESSION, TURN, 1400)
            .0
            .output_tokens_per_second
            .is_none()
    );
    let mut later = reasoning(11, "summary", "new");
    later["params"]["change"]["acceptedTextChanges"][0]["edits"][0]["at"] = json!(50);
    m.patches(&later, &targets(), 1500);
    let v = m.measurement(SESSION, TURN, 1600).0;
    assert_eq!(v.output_tokens_per_second, Some(0.75));
    assert!(v.first_token_ms.is_none() && v.first_token_missed);
    assert!(
        m.measurement(SESSION, NEXT, 1600)
            .0
            .output_tokens_per_second
            .is_none()
    );
    assert_eq!(
        m.patches(&reasoning(15, "summary", "gap"), &targets(), 1700),
        Some(SESSION.into())
    );
    assert!(
        m.measurement(SESSION, TURN, 1800)
            .0
            .output_tokens_per_second
            .is_none()
    );
}

#[test]
fn first_reasoning_token_publishes_ttft_before_body_or_turn_completion_and_stays_fixed() {
    for field in ["summary", "content"] {
        let mut m = initialized(false);
        // Starting a reasoning item is not a token.
        let mut empty = reasoning(11, field, "");
        empty["params"]["change"]["patches"] = json!([{"op":"add","path":["turnHistory","history","entitiesByKey",format!("tail:0:local:{TURN}"),"items",0],"value":{"type":"reasoning","id":"rs_reasoning","summary":[],"content":[]}}]);
        m.patches(&empty, &targets(), 1400);
        assert!(
            m.measurement(SESSION, TURN, 1400)
                .0
                .first_token_ms
                .is_none()
        );
        m.patches(&reasoning(12, field, "思"), &targets(), 1500);
        let (live, phase) = m.measurement(SESSION, TURN, 1500);
        assert_eq!(live.first_token_ms, Some(500.0));
        assert!(!live.first_token_missed);
        assert!(live.first_text_ms.is_none());
        assert!(live.text_tokens_per_second.is_none());
        assert_eq!(phase, Some(("thinking", 1500)));
        m.patches(&delta(13, TURN, "body"), &targets(), 5000);
        let live = m.measurement(SESSION, TURN, 5000).0;
        assert_eq!(live.first_token_ms, Some(500.0));
        assert_eq!(live.first_text_ms, Some(4000.0));
        assert_eq!(live.text_tokens_per_second, Some(1.0));
        m.disconnected("reconnecting");
        assert_eq!(
            m.measurement(SESSION, TURN, 6000).0.first_token_ms,
            Some(500.0)
        );
    }
}

#[test]
fn joining_after_reasoning_does_not_mislabel_later_body_as_first_token() {
    let mut s = snapshot(false);
    s["params"]["change"]["conversationState"]["turnHistory"]["history"]["entitiesByKey"]
        [format!("tail:0:local:{TURN}")]["items"] = json!([
        {"type":"reasoning","summary":["prior reasoning"],"content":[]},
        {"type":"mcpToolCall","content":[{"type":"text","text":"PRIVATE"}]}
    ]);
    let mut m = Metrics::default();
    m.owners.insert(SESSION.into(), "owner".into());
    m.snapshot(serde_json::from_value(s).unwrap(), &targets(), 1300);
    m.patches(&delta(11, TURN, "body"), &targets(), 5000);
    let live = m.measurement(SESSION, TURN, 5000).0;
    assert!(live.first_token_ms.is_none());
    assert!(live.first_token_missed);
    assert_eq!(live.first_text_ms, Some(4000.0));
}

#[test]
fn nonzero_reasoning_offsets_parts_and_revision_gaps_cannot_invent_ttft() {
    for part in [false, true] {
        let mut m = initialized(false);
        let mut v = reasoning(11, "summary", "later");
        if part {
            v["params"]["change"]["acceptedTextChanges"][0]["target"]["index"] = json!(1);
        } else {
            v["params"]["change"]["acceptedTextChanges"][0]["edits"][0]["at"] = json!(50);
        }
        m.patches(&v, &targets(), 1500);
        m.patches(&delta(12, TURN, "body"), &targets(), 5000);
        assert!(
            m.measurement(SESSION, TURN, 5000)
                .0
                .first_token_ms
                .is_none()
        );
    }
    let mut m = initialized(false);
    assert_eq!(
        m.patches(&reasoning(12, "summary", "lost"), &targets(), 1500),
        Some(SESSION.into())
    );
    let mut s = snapshot(false);
    s["params"]["change"]["revision"] = json!(20);
    m.snapshot(serde_json::from_value(s).unwrap(), &targets(), 1600);
    m.patches(&delta(21, TURN, "body"), &targets(), 5000);
    assert!(
        m.measurement(SESSION, TURN, 5000)
            .0
            .first_token_ms
            .is_none()
    );
}

#[test]
fn first_token_of_next_message_has_its_own_clock_even_after_missed_previous_message() {
    let mut m = initialized(true);
    let mut next = delta(11, NEXT, "abcd");
    next["params"]["change"]["patches"] = json!([{"op":"add","path":["turnHistory","history","entitiesByKey",format!("tail:1:local:{NEXT}")],"value":{"turnId":NEXT,"turnStartedAtMs":5000,"items":[]}}]);
    next["params"]["change"]["acceptedTextChanges"][0]["key"]["entityKey"] =
        json!(format!("tail:1:local:{NEXT}"));
    m.patches(&next, &targets(), 5200);
    assert_eq!(
        m.measurement(SESSION, NEXT, 5200).0.first_token_ms,
        Some(200.0)
    );
    assert!(
        m.measurement(SESSION, TURN, 5200)
            .0
            .first_token_ms
            .is_none()
    );
}

#[test]
fn full_generated_items_are_tokens_but_tool_text_and_empty_items_are_not() {
    for (item, expected) in [
        (
            json!({"type":"reasoning","summary":["first"],"content":[]}),
            Some(500.0),
        ),
        (json!({"type":"agentMessage","text":"first"}), Some(500.0)),
        (
            json!({"type":"reasoning","summary":[""],"content":[]}),
            None,
        ),
        (
            json!({"type":"mcpToolCall","text":"tool output","summary":["tool"],"content":[{"text":"tool"}]}),
            None,
        ),
    ] {
        let mut m = initialized(false);
        let mut v = delta(11, TURN, "");
        v["params"]["change"]["patches"] = json!([{"op":"add","path":["turnHistory","history","entitiesByKey",format!("tail:0:local:{TURN}"),"items",1],"value":item}]);
        m.patches(&v, &targets(), 1500);
        let live = m.measurement(SESSION, TURN, 1500).0;
        assert_eq!(live.first_token_ms, expected);
        assert!(live.text_tokens_per_second.is_none());
    }
    let mut m = initialized(false);
    let mut correction = delta(11, TURN, "replacement");
    correction["params"]["change"]["acceptedTextChanges"][0]["edits"][0]["deleteCount"] = json!(10);
    m.patches(&correction, &targets(), 1500);
    m.patches(&delta(12, TURN, "new body"), &targets(), 5000);
    assert!(
        m.measurement(SESSION, TURN, 5000)
            .0
            .first_token_ms
            .is_none()
    );
}

#[test]
fn desktop_entity_ids_map_to_server_turn_ids_and_backfill_missing_tail_start_time() {
    let ui = "44444444-4444-4444-8444-444444444444";
    let mut s = snapshot(false);
    let entities =
        s["params"]["change"]["conversationState"]["turnHistory"]["history"]["entitiesByKey"]
            .as_object_mut()
            .unwrap();
    let value = entities.remove(&format!("tail:0:local:{TURN}")).unwrap();
    let mut value = value;
    value["params"]["model"] = Value::Null;
    value["params"]["collaborationMode"] = json!({"settings":{"model":"native-test"}});
    entities.insert(format!("tail:7:local:{ui}"), value);
    let targets = HashMap::from([(
        SESSION.into(),
        Target {
            turn: TURN.into(),
            started: 0,
        },
    )]);
    let mut m = Metrics::default();
    m.owners.insert(SESSION.into(), "owner".into());
    m.snapshot(serde_json::from_value(s).unwrap(), &targets, 1300);
    let mut v = delta(11, TURN, "abcd");
    v["params"]["change"]["acceptedTextChanges"][0]["key"]["entityKey"] =
        json!(format!("tail:7:local:{ui}"));
    m.patches(&v, &targets, 1500);
    let measured = m.measurement(SESSION, TURN, 1500).0;
    assert_eq!(measured.first_text_ms, Some(500.0));
    assert_eq!(measured.text_tokens_per_second, Some(1.0));
    assert_eq!(m.started(SESSION, TURN), Some(1000));
    assert_eq!(
        m.metadata(SESSION, TURN),
        (Some("native-test"), Some("继续验证 [已隐藏凭据]"))
    );
    assert!(
        m.measurement(SESSION, ui, 1500)
            .0
            .text_tokens_per_second
            .is_none()
    );
    m.session_disconnected("another-session");
    assert_eq!(
        m.measurement(SESSION, TURN, 1500).0.text_tokens_per_second,
        Some(1.0)
    );
    m.session_disconnected(SESSION);
    assert!(
        m.measurement(SESSION, TURN, 1500)
            .0
            .text_tokens_per_second
            .is_none()
    );
}

#[test]
fn new_message_in_the_same_session_keeps_its_own_clock_and_late_snapshots_do_not_replay() {
    let mut m = initialized(false);
    m.patches(&delta(11, TURN, "abcd"), &targets(), 1500);
    m.snapshot(
        serde_json::from_value(snapshot(false)).unwrap(),
        &targets(),
        1600,
    );
    let mut next = delta(12, NEXT, "abcd");
    next["params"]["change"]["patches"] = json!([{"op":"add","path":["turnHistory","history","entitiesByKey",format!("tail:1:local:{NEXT}")],"value":{"turnId":NEXT,"turnStartedAtMs":5000}}]);
    next["params"]["change"]["acceptedTextChanges"][0]["key"]["entityKey"] =
        json!(format!("tail:1:local:{NEXT}"));
    m.patches(&next, &targets(), 5200);
    assert_eq!(
        m.measurement(SESSION, TURN, 5200).0.first_text_ms,
        Some(500.0)
    );
    assert_eq!(
        m.measurement(SESSION, NEXT, 5200).0.first_text_ms,
        Some(200.0)
    );
    assert_eq!(
        m.measurement(SESSION, NEXT, 5200).0.text_tokens_per_second,
        Some(1.0)
    );
}

#[test]
fn restart_restores_first_token_before_reconnection_and_only_new_text_resumes_rate() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().canonicalize().unwrap().join("timings.json");
    let mut m = initialized(false);
    m.patches(
        &reasoning(11, "summary", "PRIVATE REASONING"),
        &targets(),
        1500,
    );
    let mut measured = m.measurement(SESSION, TURN, 1500).0;
    let mut saved = Timings::open(path.clone());
    saved.apply(SESSION, TURN, m.started(SESSION, TURN), &mut measured, None);
    saved.commit().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("PRIVATE"));
    assert!(!String::from_utf8_lossy(&bytes).contains("TokenPerSecond"));
    drop(saved);
    drop(m);

    let mut reopened = Timings::open(path);
    let m = Metrics::default();
    let mut reconnecting = m.measurement(SESSION, TURN, 4000).0;
    reopened.apply(SESSION, TURN, None, &mut reconnecting, None);
    assert_eq!(reconnecting.first_token_ms, Some(500.0));
    assert!(!reconnecting.first_token_missed);
    assert!(reconnecting.text_tokens_per_second.is_none());
    let mut m = initialized(true);
    let mut late = m.measurement(SESSION, TURN, 4000).0;
    assert!(late.first_token_missed);
    reopened.apply(SESSION, TURN, m.started(SESSION, TURN), &mut late, None);
    assert_eq!(late.first_token_ms, Some(500.0));
    assert!(late.text_tokens_per_second.is_none());

    let mut after_restart = delta(11, TURN, "abcd");
    after_restart["params"]["change"]["acceptedTextChanges"][0]["edits"][0]["at"] = json!(500);
    m.patches(&after_restart, &targets(), 4500);
    let mut live = m.measurement(SESSION, TURN, 4500).0;
    reopened.apply(SESSION, TURN, m.started(SESSION, TURN), &mut live, None);
    assert_eq!(live.first_token_ms, Some(500.0));
    assert!(!live.first_token_missed);
    assert_eq!(live.text_tokens_per_second, Some(1.0));
    assert!(live.first_text_ms.is_none());
    assert_eq!(
        m.measurement(SESSION, TURN, 5600).0.text_tokens_per_second,
        None
    );
    m.disconnected("reconnecting");
    let mut disconnected = m.measurement(SESSION, TURN, 6000).0;
    reopened.apply(SESSION, TURN, None, &mut disconnected, None);
    assert_eq!(disconnected.first_token_ms, Some(500.0));
    assert!(disconnected.text_tokens_per_second.is_none());
}

#[test]
fn durable_timings_use_session_and_server_message_ids_without_rewriting_or_retiming() {
    use std::os::unix::fs::MetadataExt;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().canonicalize().unwrap().join("timings.json");
    let mut cache = Timings::open(path.clone());
    let mut first = initialized(false);
    first.patches(&delta(11, TURN, "abcd"), &targets(), 1500);
    let mut live = first.measurement(SESSION, TURN, 1500).0;
    cache.apply(SESSION, TURN, Some(1000), &mut live, None);
    cache.commit().unwrap();
    let metadata = std::fs::metadata(&path).unwrap();
    assert_eq!(metadata.mode() & 0o777, 0o600);
    let mut recovered = Metrics::default().measurement(SESSION, TURN, 5000).0;
    cache.apply(SESSION, TURN, Some(999), &mut recovered, None);
    assert_eq!(recovered.first_token_ms, Some(500.0));
    cache.commit().unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().ino(), metadata.ino());
    for (session, turn) in [(SESSION, NEXT), (NEXT, TURN)] {
        let mut unknown = Metrics::default().measurement(session, turn, 5000).0;
        cache.apply(session, turn, Some(1000), &mut unknown, None);
        assert!(unknown.first_token_ms.is_none());
    }
    let mut new_value = live.clone();
    new_value.first_token_ms = Some(800.0);
    cache.apply(SESSION, NEXT, Some(5000), &mut new_value, None);
    cache.commit().unwrap();
    let mut cache = Timings::open(path);
    let mut restored = Metrics::default().measurement(SESSION, NEXT, 7000).0;
    cache.apply(SESSION, NEXT, Some(5000), &mut restored, None);
    assert_eq!(restored.first_token_ms, Some(800.0));
    cache.apply(SESSION, TURN, Some(1000), &mut restored, None);
    assert_eq!(restored.first_token_ms, Some(500.0));
}

#[test]
fn revoked_timings_are_removed_from_disk_and_do_not_resurrect_on_reenable() {
    use std::os::unix::fs::MetadataExt;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().canonicalize().unwrap().join("timings.json");
    let mut cache = Timings::open(path.clone());
    let mut m = initialized(false);
    m.patches(&delta(11, TURN, "abcd"), &targets(), 1500);
    let mut measured = m.measurement(SESSION, TURN, 1500).0;
    cache.apply(SESSION, TURN, Some(1000), &mut measured, None);
    cache.commit().unwrap();
    cache.forget();
    cache.commit().unwrap();
    let inode = std::fs::metadata(&path).unwrap().ino();
    cache.forget();
    cache.commit().unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().ino(), inode);
    let mut reopened = Timings::open(path);
    let mut unknown = Metrics::default().measurement(SESSION, TURN, 4000).0;
    reopened.apply(SESSION, TURN, Some(1000), &mut unknown, None);
    assert!(unknown.first_token_ms.is_none());
}

#[test]
fn corrupt_invalid_and_linked_timing_files_never_supply_unmeasured_or_foreign_values() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let path = root.join("timings.json");
    for contents in [
        "{broken".to_owned(),
        json!({"version":99,"rows":[]}).to_string(),
        json!({"version":1,"rows":[{"session":SESSION,"turn":TURN,"started":1000,"ttftMs":-1}]})
            .to_string(),
        json!({"version":1,"rows":[{"session":"foreign","turn":TURN,"started":1000,"ttftMs":10}]})
            .to_string(),
    ] {
        std::fs::write(&path, contents).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut cache = Timings::open(path.clone());
        assert!(cache.error.is_some());
        let mut unknown = Metrics::default().measurement(SESSION, TURN, 4000).0;
        cache.apply(SESSION, TURN, Some(1000), &mut unknown, None);
        assert!(unknown.first_token_ms.is_none());
    }
    std::fs::remove_file(&path).unwrap();
    let protected = root.join("protected.json");
    std::fs::write(&protected, b"PRIVATE").unwrap();
    std::fs::set_permissions(&protected, std::fs::Permissions::from_mode(0o600)).unwrap();
    for hard in [false, true] {
        if hard {
            std::fs::hard_link(&protected, &path).unwrap();
        } else {
            symlink(&protected, &path).unwrap();
        }
        let mut cache = Timings::open(path.clone());
        assert!(cache.error.is_some());
        let mut m = initialized(false);
        m.patches(&delta(11, TURN, "abcd"), &targets(), 1500);
        let mut measured = m.measurement(SESSION, TURN, 1500).0;
        cache.apply(SESSION, TURN, Some(1000), &mut measured, None);
        assert!(cache.commit().is_err());
        assert_eq!(std::fs::read(&protected).unwrap(), b"PRIVATE");
        std::fs::remove_file(&path).unwrap();
    }
}

#[test]
fn durable_timings_are_bounded_and_keep_true_zero_values() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().canonicalize().unwrap().join("timings.json");
    let mut cache = Timings::open(path.clone());
    let mut measured = Metrics::default().measurement(SESSION, TURN, 4000).0;
    for n in 0..1025 {
        measured.first_token_ms = Some(0.0);
        let turn = uuid::Uuid::from_u128(n + 1).to_string();
        cache.apply(SESSION, &turn, Some(n as i64 + 1000), &mut measured, None);
    }
    cache.commit().unwrap();
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["rows"].as_array().unwrap().len(), 1024);
    let mut reopened = Timings::open(path);
    measured.first_token_ms = None;
    reopened.apply(
        SESSION,
        &uuid::Uuid::from_u128(1025).to_string(),
        None,
        &mut measured,
        None,
    );
    assert_eq!(measured.first_token_ms, Some(0.0));
    measured.first_token_ms = None;
    reopened.apply(
        SESSION,
        &uuid::Uuid::from_u128(1).to_string(),
        None,
        &mut measured,
        None,
    );
    assert!(measured.first_token_ms.is_none());
}

#[test]
fn completed_body_average_is_durable_before_reconnection_and_isolated_per_message() {
    use std::os::unix::fs::MetadataExt;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().canonicalize().unwrap().join("timings.json");
    let mut m = initialized(false);
    m.patches(&delta(11, TURN, "abcd"), &targets(), 1500);
    let mut second = delta(12, TURN, "中文");
    second["params"]["change"]["acceptedTextChanges"][0]["edits"][0]["at"] = json!(4);
    m.patches(&second, &targets(), 2000);
    let mut done = delta(13, TURN, "");
    done["params"]["change"]["patches"] = json!([{"op":"replace","path":["turnHistory","history","entitiesByKey",format!("tail:0:local:{TURN}"),"status"],"value":"completed"}]);
    m.patches(&done, &targets(), 2500);
    let mut measured = m.measurement(SESSION, TURN, 2500).0;
    assert_eq!(measured.average_text_tokens_per_second, Some(6.0));
    assert!(measured.text_tokens_per_second.is_none());
    let mut cache = Timings::open(path.clone());
    cache.apply(
        SESSION,
        TURN,
        m.started(SESSION, TURN),
        &mut measured,
        m.text_totals(SESSION, TURN),
    );
    cache.commit().unwrap();
    drop(cache);
    drop(m);
    let inode = std::fs::metadata(&path).unwrap().ino();
    let mut cache = Timings::open(path.clone());
    for _ in 0..3 {
        let mut reconnecting = Metrics::default().measurement(SESSION, TURN, 6000).0;
        cache.apply(SESSION, TURN, None, &mut reconnecting, None);
        assert_eq!(reconnecting.average_text_tokens_per_second, Some(6.0));
        assert_eq!(reconnecting.first_token_ms, Some(500.0));
        assert!(reconnecting.text_tokens_per_second.is_none());
        cache.commit().unwrap();
    }
    assert_eq!(std::fs::metadata(&path).unwrap().ino(), inode);
    for (session, turn) in [(SESSION, NEXT), (NEXT, TURN)] {
        let mut unknown = Metrics::default().measurement(session, turn, 6000).0;
        cache.apply(session, turn, Some(5000), &mut unknown, None);
        assert!(unknown.average_text_tokens_per_second.is_none());
    }
    cache.forget();
    cache.commit().unwrap();
    let mut cleared = Timings::open(path);
    let mut unknown = Metrics::default().measurement(SESSION, TURN, 6000).0;
    cleared.apply(SESSION, TURN, None, &mut unknown, None);
    assert!(unknown.average_text_tokens_per_second.is_none());
}

#[test]
fn resumed_body_totals_add_only_new_timed_segments_without_counting_snapshots_or_disconnects_twice()
{
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().canonicalize().unwrap().join("timings.json");
    let mut cache = Timings::open(path.clone());
    let mut value = Metrics::default().measurement(SESSION, TURN, 2000).0;
    cache.apply(
        SESSION,
        TURN,
        Some(1000),
        &mut value,
        Some(TextTotals {
            tokens: 3.0,
            milliseconds: 500,
        }),
    );
    cache.commit().unwrap();
    let cache = Timings::open(path);
    let mut m = Metrics::with_text_totals(cache.text_totals());
    m.owners.insert(SESSION.into(), "owner".into());
    m.snapshot(
        serde_json::from_value(snapshot(true)).unwrap(),
        &targets(),
        4000,
    );
    assert_eq!(
        m.measurement(SESSION, TURN, 4000)
            .0
            .average_text_tokens_per_second,
        Some(6.0)
    );
    assert!(
        m.measurement(SESSION, TURN, 4000)
            .0
            .text_tokens_per_second
            .is_none()
    );
    let mut first = delta(11, TURN, "abcd");
    first["params"]["change"]["acceptedTextChanges"][0]["edits"][0]["at"] = json!(500);
    m.patches(&first, &targets(), 4500);
    let mut second = delta(12, TURN, "abcd");
    second["params"]["change"]["acceptedTextChanges"][0]["edits"][0]["at"] = json!(504);
    m.patches(&second, &targets(), 5000);
    assert_eq!(
        m.measurement(SESSION, TURN, 5000)
            .0
            .average_text_tokens_per_second,
        Some(5.0)
    );
    assert_eq!(
        m.measurement(SESSION, TURN, 5000).0.text_tokens_per_second,
        Some(2.0)
    );
    m.disconnected("reconnecting");
    m.session_disconnected(SESSION);
    m.disconnected("reconnecting");
    assert_eq!(
        m.measurement(SESSION, TURN, 6000)
            .0
            .average_text_tokens_per_second,
        Some(5.0)
    );
    assert!(
        m.measurement(SESSION, TURN, 6000)
            .0
            .text_tokens_per_second
            .is_none()
    );
    m.owners.insert(SESSION.into(), "owner".into());
    m.snapshot(
        serde_json::from_value(snapshot(true)).unwrap(),
        &targets(),
        6100,
    );
    let mut gap = delta(15, TURN, "old replay");
    gap["params"]["change"]["baseRevision"] = json!(14);
    assert_eq!(m.patches(&gap, &targets(), 6200), Some(SESSION.into()));
    assert_eq!(
        m.measurement(SESSION, TURN, 6200)
            .0
            .average_text_tokens_per_second,
        Some(5.0)
    );
}

#[test]
fn v1_ttft_upgrade_preserves_values_and_body_only_samples_do_not_invent_ttft() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().canonicalize().unwrap().join("timings.json");
    std::fs::write(
        &path,
        json!({"version":1,"rows":[{"session":SESSION,"turn":TURN,"started":1000,"ttftMs":500}]})
            .to_string(),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut cache = Timings::open(path.clone());
    assert!(cache.error.is_none());
    let mut value = Metrics::default().measurement(SESSION, TURN, 4000).0;
    cache.apply(
        SESSION,
        TURN,
        None,
        &mut value,
        Some(TextTotals {
            tokens: 3.0,
            milliseconds: 500,
        }),
    );
    assert_eq!(value.first_token_ms, Some(500.0));
    assert_eq!(value.average_text_tokens_per_second, Some(6.0));
    let mut late = Metrics::default().measurement(SESSION, NEXT, 4000).0;
    late.first_token_missed = true;
    cache.apply(
        SESSION,
        NEXT,
        Some(3000),
        &mut late,
        Some(TextTotals {
            tokens: 0.0,
            milliseconds: 500,
        }),
    );
    cache.commit().unwrap();
    let saved: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["version"], 3);
    assert!(!saved.to_string().contains("textTokensPerSecond"));
    let mut cache = Timings::open(path.clone());
    cache.apply(SESSION, NEXT, None, &mut late, None);
    assert_eq!(late.average_text_tokens_per_second, Some(0.0));
    assert!(late.first_token_ms.is_none());
    assert!(late.first_token_missed);
    for body in [
        json!({"tokens":-1,"milliseconds":500}),
        json!({"tokens":3,"milliseconds":0}),
        json!({"tokens":3,"milliseconds":86400001}),
    ] {
        std::fs::write(&path, json!({"version":2,"rows":[{"session":SESSION,"turn":TURN,"started":1000,"ttftMs":null,"body":body}]}).to_string()).unwrap();
        let cache = Timings::open(path.clone());
        assert!(cache.error.is_some());
        assert!(cache.text_totals().is_empty());
    }
}
