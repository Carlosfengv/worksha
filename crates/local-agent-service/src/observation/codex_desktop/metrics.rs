//! Protocol 11 desktop follower events. Only metrics and redacted message previews are retained.
use local_agent_core::sessions::activity_performance::{
    ActivitySampling, RATE_WINDOW_MS, RateSample,
};
use local_agent_core::sessions::live::NativeRealtime;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, PartialEq)]
pub(super) struct Target {
    pub turn: String,
    pub started: i64,
}
#[derive(Default)]
pub(super) struct Metrics {
    pub state: &'static str,
    pub owners: HashMap<String, String>,
    revisions: HashMap<String, u64>,
    turns: HashMap<(String, String), Turn>,
    entities: HashMap<(String, String), Target>,
    restored_text: HashMap<(String, String), TextTotals>,
}
#[derive(Clone, Copy, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct TextTotals {
    pub tokens: f64,
    pub milliseconds: i64,
}
impl TextTotals {
    pub fn average(self) -> Option<f64> {
        (self.tokens.is_finite()
            && self.tokens >= 0.0
            && (1..=86_400_000).contains(&self.milliseconds))
        .then_some(self.tokens * 1000.0 / self.milliseconds as f64)
        .filter(|value| value.is_finite())
    }
}
#[derive(Default)]
struct Turn {
    recent_sample: Option<RateSample>,
    model: Option<String>,
    input: Option<String>,
    started: i64,
    first_output: Option<i64>,
    first_token: Option<i64>,
    missed_first_token: bool,
    first_text: Option<i64>,
    missed_first_text: bool,
    phase: Option<&'static str>,
    last_event: i64,
    window: VecDeque<(i64, f64)>,
    output_window: VecDeque<OutputSample>,
    output_observed: bool,
    reasoning_parts: HashMap<(String, &'static str, u64), u64>,
    reasoning_content: HashSet<String>,
    tool_items: HashSet<u64>,
    running_tools: HashSet<u64>,
    pending_reasoning: HashSet<String>,
    items: HashMap<String, TextItem>,
    previous_text: TextTotals,
}
impl Turn {
    fn text_totals(&self) -> TextTotals {
        self.items
            .values()
            .filter(|i| i.chunks > 1 && i.last > i.first)
            .fold(self.previous_text, |total, i| TextTotals {
                tokens: total.tokens + i.tokens,
                milliseconds: total.milliseconds + i.last - i.first,
            })
    }
    fn end_text_segment(&mut self) {
        self.previous_text = self.text_totals();
        self.items.clear();
        self.window.clear();
        self.output_window.clear();
        self.output_observed = false;
        self.reasoning_parts.clear();
        self.reasoning_content.clear();
        self.tool_items.clear();
        self.running_tools.clear();
        self.pending_reasoning.clear();
    }
    fn output(&mut self, now: i64, tokens: f64, reasoning: Option<(String, bool)>) {
        self.output_observed = true;
        self.output_window
            .retain(|s| (0..i64::from(RATE_WINDOW_MS)).contains(&(now - s.at)));
        if self.output_window.len() < 4096 {
            self.output_window.push_back(OutputSample {
                at: now,
                tokens,
                reasoning,
            });
            let rate = self.output_window.iter().map(|s| s.tokens).sum::<f64>()
                / (f64::from(RATE_WINDOW_MS) / 1000.0);
            if rate.is_finite() && rate > 0.0 {
                self.recent_sample = Some(RateSample {
                    at: now,
                    window_ms: RATE_WINDOW_MS,
                    tokens_per_second: rate,
                });
            }
        }
    }
    fn tool_status(&mut self, index: u64, status: &str) {
        match status {
            "inProgress" => {
                self.running_tools.insert(index);
            }
            "completed" | "failed" | "declined" => {
                self.running_tools.remove(&index);
            }
            _ => return,
        }
        self.phase = Some(if self.running_tools.is_empty() {
            "tool_returned"
        } else {
            "tool_requested"
        });
        self.window.clear();
    }
    fn reasoning_item(&mut self, value: &Value, added: bool, has_delta: bool, now: i64) {
        let Some(id) = value["id"]
            .as_str()
            .filter(|id| id.starts_with("rs_") && id.len() <= 256)
        else {
            return;
        };
        let tokens = |field: &str| {
            value[field]
                .as_array()
                .into_iter()
                .flatten()
                .take(256)
                .filter_map(Value::as_str)
                .map(estimate)
                .sum::<f64>()
        };
        let content = tokens("content");
        let summary = tokens("summary");
        if added && content == 0.0 && summary == 0.0 && self.pending_reasoning.len() < 256 {
            self.pending_reasoning.insert(id.to_owned());
        } else if (content > 0.0 || summary > 0.0)
            && self.pending_reasoning.remove(id)
            && !has_delta
        {
            // Some desktop turns publish a whole reasoning item after an empty
            // live start. Count that delivery once; snapshots/corrections never
            // establish this witness, and text-splice deliveries take precedence.
            self.output(
                now,
                if content > 0.0 { content } else { summary },
                Some((id.to_owned(), content == 0.0)),
            );
        }
    }
}
struct OutputSample {
    at: i64,
    tokens: f64,
    // The raw content and summary of one reasoning item are alternative streams.
    reasoning: Option<(String, bool)>,
}
#[derive(Default)]
struct TextItem {
    first: i64,
    last: i64,
    tokens: f64,
    chunks: usize,
    utf16_length: u64,
}

// Large snapshots are decoded directly from a bounded reader. Unknown fields,
// including all tool results and the full transcript, are skipped by serde.
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Snapshot {
    pub version: Option<u64>,
    pub source_client_id: Option<String>,
    #[serde(default)]
    pub params: SnapshotParams,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SnapshotParams {
    pub conversation_id: Option<String>,
    pub host_id: Option<String>,
    #[serde(default)]
    pub change: SnapshotChange,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SnapshotChange {
    #[serde(rename = "type")]
    pub kind: String,
    pub revision: Option<u64>,
    #[serde(default)]
    pub conversation_state: Conversation,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Conversation {
    #[serde(default)]
    turn_history: History,
}
#[derive(Default, Deserialize)]
struct History {
    #[serde(default)]
    history: Entities,
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entities {
    #[serde(default, deserialize_with = "bounded_map")]
    entities_by_key: HashMap<String, SnapshotTurn>,
}
fn bounded_map<'de, D, T>(d: D) -> Result<HashMap<String, T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct Visitor<T>(std::marker::PhantomData<T>);
    impl<'de, T: Deserialize<'de>> serde::de::Visitor<'de> for Visitor<T> {
        type Value = HashMap<String, T>;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("bounded desktop metadata map")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut map: A,
        ) -> Result<Self::Value, A::Error> {
            let mut result = HashMap::new();
            while let Some(key) = map.next_key::<String>()? {
                if result.len() >= 512 || key.len() > 512 {
                    return Err(serde::de::Error::custom("desktop metadata limit"));
                }
                result.insert(key, map.next_value()?);
            }
            Ok(result)
        }
    }
    d.deserialize_map(Visitor(std::marker::PhantomData))
}
#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotTurn {
    #[serde(default)]
    params: TurnParams,
    turn_id: Option<String>,
    turn_started_at_ms: Option<i64>,
    first_turn_work_item_started_at_ms: Option<i64>,
    status: Option<String>,
    #[serde(default, rename = "items", deserialize_with = "has_generated_tokens")]
    has_generated_tokens: bool,
    #[serde(default, deserialize_with = "bounded_map")]
    aeon_assistant_message_started_at_ms_by_id: HashMap<String, i64>,
}
// Only retain whether a snapshot already contains generated text. Neither
// reasoning content nor tool output is kept in the observer's metadata.
#[derive(Default, Deserialize)]
struct TokenEvidence {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default, deserialize_with = "nonempty_text")]
    text: bool,
    #[serde(default, deserialize_with = "nonempty_text")]
    summary: bool,
    #[serde(default, deserialize_with = "nonempty_text")]
    content: bool,
}
impl TokenEvidence {
    fn generated(&self) -> bool {
        matches!(self.kind.as_str(), "agentMessage" | "plan" | "reasoning")
            && (self.text || self.summary || self.content)
    }
}
fn nonempty_text<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = bool;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("text or text parts")
        }
        fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<bool, E> {
            Ok(!value.is_empty())
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<bool, A::Error> {
            let mut found = false;
            while let Some(value) = seq.next_element::<TextEvidence>()? {
                found |= value.0;
            }
            Ok(found)
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<bool, A::Error> {
            while map
                .next_entry::<serde::de::IgnoredAny, serde::de::IgnoredAny>()?
                .is_some()
            {}
            Ok(false)
        }
        fn visit_unit<E: serde::de::Error>(self) -> Result<bool, E> {
            Ok(false)
        }
    }
    d.deserialize_any(Visitor)
}
struct TextEvidence(bool);
impl<'de> Deserialize<'de> for TextEvidence {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        nonempty_text(d).map(Self)
    }
}
fn has_generated_tokens<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    struct Visitor;
    impl<'de> serde::de::Visitor<'de> for Visitor {
        type Value = bool;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("desktop items")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<bool, A::Error> {
            let mut found = false;
            while let Some(item) = seq.next_element::<TokenEvidence>()? {
                found |= item.generated();
            }
            Ok(found)
        }
    }
    d.deserialize_seq(Visitor)
}
#[derive(Default, Deserialize)]
struct TurnParams {
    model: Option<String>,
    #[serde(default, rename = "collaborationMode")]
    collaboration_mode: Option<Collaboration>,
    #[serde(default)]
    input: Vec<InputPart>,
    #[serde(default, rename = "toolOutput")]
    tool_output: Option<TaskInput>,
}
impl TurnParams {
    fn preview(&self) -> Option<String> {
        self.input
            .iter()
            .take(64)
            .filter(|part| part.kind == "text")
            .find_map(|part| part.text.clone())
            .or_else(|| {
                let task = self.tool_output.as_ref()?;
                (task.namespace.as_deref() == Some("codex_app")
                    && task.name.as_deref() == Some("send_message_to_thread"))
                .then(|| task.output.clone())
                .flatten()
            })
    }
}
#[derive(Default, Deserialize)]
struct TaskInput {
    namespace: Option<String>,
    name: Option<String>,
    #[serde(default, deserialize_with = "redacted_task_preview")]
    output: Option<String>,
}
fn redacted_task_preview<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<String>, D::Error> {
    let text = Option::<String>::deserialize(d)?;
    Ok(text.and_then(|text| {
        local_agent_core::sessions::activity_input::codex_task_preview(
            "codex_app",
            "send_message_to_thread",
            &text,
        )
    }))
}
#[derive(Default, Deserialize)]
struct Collaboration {
    #[serde(default)]
    settings: ModelSettings,
}
#[derive(Default, Deserialize)]
struct ModelSettings {
    model: Option<String>,
}
#[derive(Deserialize)]
struct InputPart {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default, deserialize_with = "redacted_preview")]
    text: Option<String>,
}
fn redacted_preview<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let text = Option::<String>::deserialize(d)?;
    Ok(text
        .filter(|s| s.len() <= 1024 * 1024)
        .and_then(|s| local_agent_core::sessions::activity_input::preview(&Value::String(s))))
}
fn turn_key(key: &str) -> Option<&str> {
    let rest = key.strip_prefix("tail:")?;
    let (generation, rest) = rest.split_once(':')?;
    if generation.len() > 12 || generation.parse::<u64>().is_err() {
        return None;
    }
    let id = rest.strip_prefix("local:")?;
    uuid::Uuid::parse_str(id).ok()?;
    Some(id)
}
fn latency(start: i64, end: i64) -> Option<f64> {
    (start > 0 && end >= start && end - start <= 86_400_000).then_some((end - start) as f64)
}
impl Metrics {
    pub fn with_text_totals(restored_text: HashMap<(String, String), TextTotals>) -> Self {
        Self {
            restored_text,
            ..Default::default()
        }
    }
    pub fn text_totals(&self, session: &str, turn: &str) -> Option<TextTotals> {
        self.turns
            .get(&(session.into(), turn.into()))
            .map(Turn::text_totals)
            .filter(|total| total.average().is_some())
    }
    pub fn session_disconnected(&mut self, session: &str) {
        self.entities.retain(|(s, _), _| s != session);
        self.owners.remove(session);
        self.revisions.remove(session);
        for ((s, _), t) in &mut self.turns {
            if s == session {
                t.end_text_segment();
                t.missed_first_text = true;
                t.missed_first_token = true;
            }
        }
    }
    pub fn disconnected(&mut self, state: &'static str) {
        self.state = state;
        self.owners.clear();
        self.revisions.clear();
        self.entities.clear();
        for turn in self.turns.values_mut() {
            turn.end_text_segment();
            turn.missed_first_text = true;
            turn.missed_first_token = true;
        }
    }
    pub fn retain(&mut self, targets: &HashMap<String, Target>) {
        self.owners.retain(|s, _| targets.contains_key(s));
        self.revisions.retain(|s, _| targets.contains_key(s));
        self.entities.retain(|(s, _), _| targets.contains_key(s));
        self.turns.retain(|(s, _), t| {
            targets.contains_key(s)
                && chrono::Utc::now().timestamp_millis() - t.last_event < 10_800_000
        });
    }
    fn entry(&mut self, session: &str, turn: &str, started: i64) -> &mut Turn {
        let key = (session.to_owned(), turn.to_owned());
        if self.turns.len() >= 256
            && !self.turns.contains_key(&key)
            && let Some(key) = self
                .turns
                .iter()
                .min_by_key(|(_, t)| t.last_event)
                .map(|(k, _)| k.clone())
        {
            self.turns.remove(&key);
        }
        let previous_text = self.restored_text.remove(&key).unwrap_or_default();
        let result = self.turns.entry(key).or_insert_with(|| Turn {
            previous_text,
            ..Default::default()
        });
        if started > 0 {
            result.started = started;
        }
        result
    }
    pub fn snapshot(&mut self, snapshot: Snapshot, targets: &HashMap<String, Target>, now: i64) {
        let p = snapshot.params;
        let Some(session) = p.conversation_id.filter(|s| targets.contains_key(s)) else {
            return;
        };
        if p.host_id.as_deref() != Some("local")
            || p.change.kind != "snapshot"
            || self.owners.get(&session) != snapshot.source_client_id.as_ref()
        {
            return;
        }
        if snapshot.version != Some(11) {
            self.disconnected("incompatible");
            return;
        }
        let Some(revision) = p.change.revision else {
            return;
        };
        if self
            .revisions
            .get(&session)
            .is_some_and(|current| revision < *current)
        {
            return;
        }
        self.revisions.insert(session.clone(), revision);
        self.state = "connected";
        for (key, value) in p
            .change
            .conversation_state
            .turn_history
            .history
            .entities_by_key
        {
            if turn_key(&key).is_none() {
                continue;
            }
            let Some(turn) = value
                .turn_id
                .as_deref()
                .filter(|id| uuid::Uuid::parse_str(id).is_ok())
            else {
                continue;
            };
            if targets[&session].turn != turn {
                continue;
            }
            self.entities.insert(
                (session.clone(), key),
                Target {
                    turn: turn.into(),
                    started: value.turn_started_at_ms.unwrap_or(0),
                },
            );
            let t = self.entry(
                &session,
                turn,
                value
                    .turn_started_at_ms
                    .unwrap_or(targets[&session].started),
            );
            t.first_output = value.first_turn_work_item_started_at_ms;
            t.input = value.params.preview().or_else(|| t.input.take());
            t.model = value
                .params
                .model
                .or_else(|| {
                    value
                        .params
                        .collaboration_mode
                        .and_then(|mode| mode.settings.model)
                })
                .map(|model| local_agent_core::sessions::sanitize_label(&model, 300));
            t.missed_first_text |= !value.aeon_assistant_message_started_at_ms_by_id.is_empty();
            t.missed_first_token |= value.has_generated_tokens
                || !value.aeon_assistant_message_started_at_ms_by_id.is_empty();
            t.last_event = now;
            t.phase = match value.status.as_deref() {
                Some("inProgress") => Some("processing"),
                Some("completed") => Some("waiting_input"),
                Some("failed") => Some("error"),
                Some("interrupted") => Some("cancelled"),
                _ => None,
            };
        }
    }
    /// A mismatch is never replayed as fresh text. Ask the owner for a new snapshot.
    pub fn patches(
        &mut self,
        v: &Value,
        targets: &HashMap<String, Target>,
        now: i64,
    ) -> Option<String> {
        let p = &v["params"];
        let session = p["conversationId"].as_str()?;
        if !targets.contains_key(session)
            || p["hostId"] != "local"
            || self.owners.get(session).map(String::as_str) != v["sourceClientId"].as_str()
        {
            return None;
        }
        if v["version"] != 11 {
            self.disconnected("incompatible");
            return None;
        }
        let c = &p["change"];
        if c["type"] != "patches" {
            return None;
        }
        let base = c["baseRevision"].as_u64()?;
        let revision = c["revision"].as_u64()?;
        if self.revisions.get(session) != Some(&base) || base.checked_add(1) != Some(revision) {
            self.revisions.remove(session);
            for ((s, _), t) in &mut self.turns {
                if s == session {
                    t.end_text_segment();
                    t.missed_first_text = true;
                    t.missed_first_token = true;
                }
            }
            return Some(session.to_owned());
        }
        self.revisions.insert(session.to_owned(), revision);
        for patch in c["patches"].as_array().into_iter().flatten() {
            let Some(path) = patch["path"].as_array() else {
                continue;
            };
            if path.len() < 4
                || path[0] != "turnHistory"
                || path[1] != "history"
                || path[2] != "entitiesByKey"
            {
                continue;
            }
            let Some(key) = path[3].as_str().filter(|key| turn_key(key).is_some()) else {
                continue;
            };
            let value = &patch["value"];
            let field = path.get(4).and_then(Value::as_str);
            let entity_key = (session.to_owned(), key.to_owned());
            if self.entities.len() >= 512 && !self.entities.contains_key(&entity_key) {
                continue;
            }
            let entity = self.entities.entry(entity_key).or_insert_with(|| Target {
                turn: String::new(),
                started: 0,
            });
            if path.len() == 4 {
                if let Some(turn) = value["turnId"]
                    .as_str()
                    .filter(|id| uuid::Uuid::parse_str(id).is_ok())
                {
                    entity.turn = turn.into();
                }
                if let Some(start) = value["turnStartedAtMs"].as_i64() {
                    entity.started = start;
                }
            }
            if field == Some("turnId")
                && let Some(turn) = value
                    .as_str()
                    .filter(|id| uuid::Uuid::parse_str(id).is_ok())
            {
                entity.turn = turn.into();
            }
            if field == Some("turnStartedAtMs")
                && let Some(start) = value.as_i64()
            {
                entity.started = start;
            }
            let entity = entity.clone();
            if entity.turn.is_empty() {
                continue;
            }
            let turn = entity.turn.as_str();
            let start = if entity.started > 0 {
                entity.started
            } else if targets[session].turn == turn {
                targets[session].started
            } else {
                0
            };
            let t = self.entry(session, turn, start);
            t.last_event = now;
            if path.len() == 4
                && patch["op"] == "add"
                && let Ok(metadata) = serde_json::from_value::<SnapshotTurn>(value.clone())
            {
                t.input = metadata.params.preview().or_else(|| t.input.take());
                t.missed_first_token |= metadata.has_generated_tokens;
                t.missed_first_text |= !metadata
                    .aeon_assistant_message_started_at_ms_by_id
                    .is_empty();
            }
            if field == Some("params") && matches!(patch["op"].as_str(), Some("add" | "replace")) {
                let params = if path.len() == 5 {
                    Some(value.clone())
                } else if path.len() == 6
                    && matches!(path[5].as_str(), Some("input" | "toolOutput"))
                {
                    let mut object = serde_json::Map::new();
                    object.insert(path[5].as_str().unwrap().into(), value.clone());
                    Some(Value::Object(object))
                } else {
                    None
                };
                if let Some(input) = params
                    .and_then(|params| serde_json::from_value::<TurnParams>(params).ok())
                    .and_then(|params| params.preview())
                {
                    t.input = Some(input);
                }
            }
            if field == Some("turnStartedAtMs")
                && let Some(start) = value.as_i64()
            {
                t.started = start;
            }
            if field == Some("firstTurnWorkItemStartedAtMs") {
                t.first_output = value.as_i64();
            }
            if field == Some("status") {
                let phase = match value.as_str() {
                    Some("completed") => Some("waiting_input"),
                    Some("failed") => Some("error"),
                    Some("interrupted") => Some("cancelled"),
                    _ => None,
                };
                if let Some(phase) = phase {
                    t.phase = Some(phase);
                    t.window.clear();
                    t.output_window.clear();
                    t.output_observed = false;
                }
            }
            if field == Some("items") && path.len() == 6 {
                let index = path[5].as_u64().filter(|i| *i < 512);
                if patch["op"] == "remove" {
                    // Removal shifts array indices; wait for fresh item identities.
                    t.tool_items.clear();
                    t.running_tools.clear();
                }
                if t.first_token.is_none()
                    && !t.missed_first_token
                    && matches!(patch["op"].as_str(), Some("add" | "replace"))
                    && serde_json::from_value::<TokenEvidence>(value.clone())
                        .is_ok_and(|item| item.generated())
                {
                    t.first_token = Some(now);
                }
                match value["type"].as_str() {
                    Some("reasoning") => {
                        t.phase = Some("thinking");
                        t.window.clear();
                        let has_delta = c["acceptedTextChanges"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .any(|change| {
                                change["key"]["itemId"] == value["id"]
                                    && !change["edits"].as_array().is_none_or(Vec::is_empty)
                            });
                        t.reasoning_item(value, patch["op"] == "add", has_delta, now);
                    }
                    Some("commandExecution" | "mcpToolCall" | "fileChange" | "dynamicToolCall") => {
                        if let Some(index) = index
                            && t.tool_items.len() < 256
                        {
                            t.tool_items.insert(index);
                            if let Some(status) = value["status"].as_str() {
                                t.tool_status(index, status);
                            }
                        }
                    }
                    _ => {}
                }
            }
            if field == Some("items")
                && path.len() == 7
                && path[6] == "status"
                && let Some(index) = path[5].as_u64().filter(|i| t.tool_items.contains(i))
                && let Some(status) = value.as_str()
            {
                t.tool_status(index, status);
            }
        }
        for change in c["acceptedTextChanges"].as_array().into_iter().flatten() {
            if change["key"]["hostId"] != "local" || change["key"]["threadId"] != session {
                continue;
            }
            let field = change["target"]["field"].as_str().unwrap_or_default();
            let reasoning = matches!(field, "summary" | "content");
            if field != "text" && !reasoning {
                continue;
            }
            let Some(entity) = change["key"]["entityKey"]
                .as_str()
                .and_then(|key| self.entities.get(&(session.into(), key.into())))
                .cloned()
            else {
                continue;
            };
            let turn = entity.turn.as_str();
            if turn.is_empty() {
                continue;
            }
            let Some(item) = change["key"]["itemId"].as_str().filter(|s| {
                s.len() <= 256 && s.starts_with(if reasoning { "rs_" } else { "msg_" })
            }) else {
                continue;
            };
            let Some(edits) = change["edits"].as_array() else {
                continue;
            };
            // Replacements/deletions are corrections, never new generation samples.
            if edits.len() != 1
                || edits.iter().any(|e| {
                    e["deleteCount"] != 0
                        || e["at"].as_u64().is_none()
                        || e["insert"].as_str().is_none_or(|s| s.len() > 256 * 1024)
                })
            {
                // A correction is evidence that generated text already existed.
                if !edits.is_empty() {
                    let t = self.entry(session, turn, entity.started);
                    t.missed_first_token = true;
                    t.output_window.clear();
                    t.output_observed = false;
                }
                continue;
            }
            let at = edits[0]["at"].as_u64().unwrap_or(0);
            if edits[0]["insert"].as_str().is_none_or(str::is_empty) {
                continue;
            }
            let start = if entity.started > 0 {
                entity.started
            } else if targets[session].turn == turn {
                targets[session].started
            } else {
                0
            };
            let t = self.entry(session, turn, start);
            let text = edits[0]["insert"].as_str().unwrap_or_default();
            let length = text.encode_utf16().count() as u64;
            let tokens = estimate(text);
            if reasoning {
                let Some(index) = change["target"]["index"].as_u64().filter(|i| *i < 256) else {
                    continue;
                };
                let field = if field == "summary" {
                    "summary"
                } else {
                    "content"
                };
                let key = (item.to_owned(), field, index);
                t.pending_reasoning.remove(item);
                if t.reasoning_parts.len() >= 256 && !t.reasoning_parts.contains_key(&key) {
                    continue;
                }
                if t.reasoning_parts
                    .get(&key)
                    .is_some_and(|expected| *expected != at)
                {
                    t.output_window.clear();
                    t.output_observed = false;
                    t.reasoning_parts.insert(key, at.saturating_add(length));
                    continue;
                }
                t.reasoning_parts.insert(key, at.saturating_add(length));
                if field == "content" {
                    if t.reasoning_content.insert(item.to_owned()) {
                        t.output_window.retain(|s| {
                            s.reasoning
                                .as_ref()
                                .is_none_or(|(id, summary)| id != item || !summary)
                        });
                    }
                } else if t.reasoning_content.contains(item) {
                    continue;
                }
            }
            if t.first_token.is_none() {
                // A nonzero offset/part is a late join, never the first token.
                t.missed_first_token |=
                    at > 0 || (reasoning && change["target"]["index"].as_u64() != Some(0));
                if !t.missed_first_token {
                    t.first_token = Some(now);
                }
            }
            if reasoning {
                t.last_event = now;
                t.phase = Some("thinking");
                t.window.clear();
                t.output(now, tokens, Some((item.to_owned(), field == "summary")));
                continue;
            }
            if tokens == 0.0 {
                continue;
            }
            if t.items.len() >= 256 && !t.items.contains_key(item) {
                continue;
            }
            if t.items
                .get(item)
                .is_some_and(|text| text.utf16_length != at)
            {
                t.window.clear();
                t.output_window.clear();
                t.output_observed = false;
                t.missed_first_text = true;
                continue;
            }
            if !t.items.contains_key(item) && at > 0 {
                t.missed_first_text = true;
            }
            if !t.missed_first_text {
                t.first_text.get_or_insert(now);
            }
            let text = t.items.entry(item.to_owned()).or_default();
            if text.chunks == 0 {
                text.first = now;
            }
            text.last = now;
            text.tokens += tokens;
            text.chunks += 1;
            text.utf16_length = at.saturating_add(length);
            t.last_event = now;
            t.phase = Some("streaming");
            t.window.retain(|(at, _)| now - at <= 1000);
            if t.window.len() < 4096 {
                t.window.push_back((now, tokens));
            }
            t.output(now, tokens, None);
        }
        None
    }
    pub fn started(&self, session: &str, turn: &str) -> Option<i64> {
        self.turns
            .get(&(session.into(), turn.into()))
            .map(|t| t.started)
            .filter(|at| *at > 0)
    }
    pub fn metadata(&self, session: &str, turn: &str) -> (Option<&str>, Option<&str>) {
        self.turns
            .get(&(session.into(), turn.into()))
            .map(|t| (t.model.as_deref(), t.input.as_deref()))
            .unwrap_or_default()
    }
    pub fn measurement(
        &self,
        session: &str,
        turn: &str,
        now: i64,
    ) -> (NativeRealtime, Option<(&'static str, i64)>) {
        let t = self.turns.get(&(session.into(), turn.into()));
        let ready = self.state == "connected" && self.revisions.contains_key(session);
        let speed = t
            .filter(|t| ready && t.phase == Some("streaming") && now - t.last_event <= 30_000)
            .map(|t| {
                t.window
                    .iter()
                    .filter(|(at, _)| (0..=1000).contains(&(now - at)))
                    .map(|(_, n)| n)
                    .sum()
            })
            .filter(|value: &f64| *value > 0.0);
        (
            NativeRealtime {
                sampling: Some(ActivitySampling {
                    at: now,
                    window_ms: RATE_WINDOW_MS,
                    tokens_per_second: t
                        .filter(|t| {
                            ready
                                && matches!(t.phase, Some("thinking" | "streaming"))
                                && (0..=30_000).contains(&(now - t.last_event))
                        })
                        .map(|t| {
                            t.output_window
                                .iter()
                                .filter(|s| (0..i64::from(RATE_WINDOW_MS)).contains(&(now - s.at)))
                                .map(|s| s.tokens)
                                .sum::<f64>()
                                / (f64::from(RATE_WINDOW_MS) / 1000.0)
                        })
                        .filter(|value| value.is_finite() && *value > 0.0),
                    recent: t.and_then(|t| t.recent_sample.clone()),
                    reported: None,
                }),
                state: if ready {
                    "connected"
                } else if self.state == "connected" {
                    "resyncing"
                } else if self.state.is_empty() {
                    "connecting"
                } else {
                    self.state
                }
                .into(),
                first_token_ms: t.and_then(|t| t.first_token.and_then(|at| latency(t.started, at))),
                first_token_missed: t
                    .is_some_and(|t| t.first_token.is_none() && t.missed_first_token),
                first_text_ms: t.and_then(|t| t.first_text.and_then(|at| latency(t.started, at))),
                first_output_ms: t
                    .and_then(|t| t.first_output.and_then(|at| latency(t.started, at))),
                text_tokens_per_second: speed,
                output_tokens_per_second: t
                    .filter(|t| {
                        ready
                            && t.output_observed
                            && (0..=30_000).contains(&(now - t.last_event))
                            && matches!(
                                t.phase,
                                Some("thinking" | "streaming" | "tool_requested" | "tool_returned")
                            )
                    })
                    .map(|t| {
                        t.output_window
                            .iter()
                            .filter(|s| (0..=1000).contains(&(now - s.at)))
                            .map(|s| s.tokens)
                            .fold(0.0, |sum, value| sum + value)
                    })
                    // Silence on the desktop stream does not measure hidden reasoning.
                    .filter(|value| *value > 0.0),
                average_text_tokens_per_second: t.and_then(|t| t.text_totals().average()),
            },
            t.filter(|_| ready)
                .and_then(|t| t.phase.map(|phase| (phase, t.last_event))),
        )
    }
}
fn estimate(text: &str) -> f64 {
    text.chars()
        .map(|c| {
            if c.is_ascii() {
                0.25
            } else if ('\u{4e00}'..='\u{9fff}').contains(&c) {
                1.0
            } else {
                0.5
            }
        })
        .sum()
}
