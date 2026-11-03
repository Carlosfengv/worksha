//! Recent user-message projections built during the existing read-only index pass.
use super::{
    activity_input, activity_performance::MessageUsage, live::NativeActivity,
    parser::SessionSummary,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::VecDeque;

#[derive(Clone)]
struct Message {
    usage: Option<MessageUsage>,
    id: String,
    started_at: String,
    event_at: String,
    input: Option<activity_input::UserInput>,
    model: Option<String>,
    phase: &'static str,
    ttft: Option<f64>,
}

#[derive(Clone, Default)]
pub(super) struct Collector {
    session: Option<String>,
    rows: VecDeque<Message>,
    current: Option<String>,
}
impl Collector {
    fn begin(&mut self, id: &str, at: &str) {
        if !valid_id(id) {
            self.current = None;
            return;
        }
        self.current = Some(id.into());
        if self.rows.iter().any(|row| row.id == id) {
            return;
        }
        if self.rows.len() >= 128 {
            self.rows.pop_front();
        }
        self.rows.push_back(Message {
            usage: None,
            id: id.into(),
            started_at: at.into(),
            event_at: at.into(),
            input: None,
            model: None,
            phase: "processing",
            ttft: None,
        });
    }

    pub fn observe(&mut self, agent: &str, value: &Value) {
        let Some(at) = value["timestamp"]
            .as_str()
            .and_then(|at| DateTime::parse_from_rfc3339(at).ok())
        else {
            return;
        };
        if at > Utc::now() + chrono::Duration::minutes(5) {
            return;
        }
        let at = at.with_timezone(&Utc).to_rfc3339();
        let kind = value["type"].as_str().unwrap_or("");
        let p = &value["payload"];
        let event = p["type"].as_str().unwrap_or("");
        if agent == "codex" && kind == "session_meta" {
            self.session = p["id"].as_str().map(str::to_owned);
        }
        if agent == "codex" && kind == "token_usage_record" {
            if let Some(session) = &self.session
                && let Some(turn) = p["turn_id"].as_str()
                && let Some(row) = self.rows.iter_mut().find(|row| row.id == turn)
            {
                MessageUsage::merge(&mut row.usage, MessageUsage::codex(value, session, turn));
            }
            return;
        }
        if agent == "claude" && value["isSidechain"] == true {
            return;
        }
        if agent == "codex"
            && kind == "event_msg"
            && event == "task_started"
            && let Some(id) = p["turn_id"].as_str()
        {
            self.begin(id, &at);
        }
        let input = if agent == "claude"
            && kind == "user"
            && !value["message"]["content"]
                .as_array()
                .is_some_and(|parts| parts.iter().any(|p| p["type"] == "tool_result"))
        {
            activity_input::preview(&value["message"]["content"])
        } else if agent == "codex" {
            activity_input::codex(value, self.current.as_deref())
        } else {
            None
        };
        if input.is_some() {
            if agent == "claude" {
                self.begin(value["uuid"].as_str().unwrap_or(&at), &at);
            } else if self.current.is_none() {
                self.begin(p["turn_id"].as_str().unwrap_or(&at), &at);
            }
        }
        let Some(row) = self
            .rows
            .iter_mut()
            .find(|row| Some(&row.id) == self.current.as_ref())
        else {
            return;
        };
        // Explicit references to another turn cannot update this message.
        if agent == "codex" && p["turn_id"].as_str().is_some_and(|id| id != row.id) {
            return;
        }
        let has_input = input.is_some();
        if let Some(text) = input {
            row.input = Some(activity_input::UserInput {
                text,
                at: at.clone(),
            });
        }
        if at < row.event_at {
            return;
        }
        let terminal = if agent == "codex" && kind == "event_msg" {
            match event {
                "task_complete" if p["error"].is_null() => Some("waiting_input"),
                "turn_aborted" => Some("cancelled"),
                "task_complete" | "error" | "stream_error" | "turn_failed" => Some("error"),
                _ => None,
            }
        } else if agent == "claude" && kind == "system" && value["subtype"] == "turn_duration" {
            Some("waiting_input")
        } else if agent == "claude" && value["isApiErrorMessage"] == true {
            Some("error")
        } else {
            None
        };
        if let Some(phase) = terminal {
            row.phase = phase;
            if phase == "waiting_input" {
                row.ttft = p["time_to_first_token_ms"]
                    .as_f64()
                    .filter(|n| n.is_finite() && *n >= 0.0);
            }
        }
        if terminal.is_none()
            && !has_input
            && ["waiting_input", "cancelled", "error"].contains(&row.phase)
        {
            return;
        }
        if (agent == "codex" && kind == "turn_context")
            || (agent == "claude" && kind == "assistant")
        {
            let model = if agent == "codex" {
                &p["model"]
            } else {
                &value["message"]["model"]
            };
            if let Some(model) = model
                .as_str()
                .filter(|model| !model.is_empty() && *model != "<synthetic>")
            {
                row.model = Some(super::sanitize_label(model, 250));
            }
        }
        let business_event = if agent == "codex" {
            (kind == "response_item"
                && matches!(
                    event,
                    "message"
                        | "reasoning"
                        | "function_call"
                        | "custom_tool_call"
                        | "function_call_output"
                        | "custom_tool_call_output"
                )
                && (p["role"] != "user" || has_input))
                || (kind == "event_msg"
                    && (matches!(
                        event,
                        "task_started"
                            | "task_complete"
                            | "turn_aborted"
                            | "error"
                            | "stream_error"
                            | "turn_failed"
                            | "item_completed"
                    ) || (event == "user_message" && has_input)))
        } else {
            matches!(kind, "user" | "assistant") || terminal.is_some()
        };
        if business_event {
            row.event_at = at;
        }
    }

    pub fn finish(self, summary: &SessionSummary) -> Vec<NativeActivity> {
        let now = Utc::now();
        let latest = self.rows.back().map(|row| row.id.clone());
        self.rows
            .into_iter()
            .filter(|row| {
                DateTime::parse_from_rfc3339(&row.event_at).is_ok_and(|at| {
                    let age = now.signed_duration_since(at).num_milliseconds();
                    age >= 0 && (age <= 86_400_000 || latest.as_ref() == Some(&row.id))
                })
            })
            .map(|row| NativeActivity {
                message_usage: row.usage,
                turn_id: Some(row.id),
                #[cfg(any(target_os = "macos", test))]
                call_id: None,
                agent_product_id: None,
                agent_attribution: None,
                id: summary.id.clone(),
                agent_id: summary.agent_id.clone(),
                session_id: summary.id.clone(),
                title: summary.title.clone(),
                project: summary.project_name.clone(),
                model: row.model,
                phase: row.phase.into(),
                step: if row.phase == "waiting_input" {
                    "本轮已完成，等待输入"
                } else {
                    "已恢复历史消息"
                }
                .into(),
                user_input: row.input,
                tool_names: vec![],
                message_started_at: Some(row.started_at),
                event_at: row.event_at,
                turn_ttft_ms: row.ttft,
                realtime: None,
                stale: !["waiting_input", "cancelled", "error"].contains(&row.phase),
            })
            .collect()
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 512
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b":._-+".contains(&b))
}
