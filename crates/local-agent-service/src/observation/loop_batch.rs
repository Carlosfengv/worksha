use local_agent_core::{
    providers::files::hash,
    routing::loop_guard::{Batch, Call},
    sessions::activity_input::preview,
};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Default)]
struct History<'a> {
    offset: usize,
    calls: BTreeMap<&'a str, (&'a str, &'a Value)>,
    results: BTreeMap<&'a str, &'a Value>,
}

impl<'a> History<'a> {
    fn clear(&mut self, offset: usize) {
        self.offset = offset;
        self.calls.clear();
        self.results.clear();
    }

    fn call(&mut self, id: &'a Value, name: &'a Value, args: &'a Value) {
        if let Some((id, name)) = id.as_str().zip(name.as_str()) {
            self.calls.insert(id, (name, args));
        }
    }

    fn result(&mut self, id: &'a Value, result: &'a Value) {
        if let Some(id) = id.as_str().filter(|id| self.calls.contains_key(id)) {
            self.results.insert(id, result);
        }
    }

    fn batch(self, turn: &str) -> Option<Batch> {
        if self.results.is_empty() {
            return None;
        }
        let mut calls = Vec::with_capacity(self.results.len());
        let mut fingerprints = Vec::with_capacity(self.results.len());
        let mut has_error = false;
        let mut has_success = false;
        for (id, result) in self.results {
            let (name, input) = self.calls[id];
            let parsed_args = input
                .as_str()
                .and_then(|text| serde_json::from_str::<Value>(text).ok());
            let args_hash = content_hash(parsed_args.as_ref().unwrap_or(input));
            let content = result
                .get("content")
                .or_else(|| result.get("output"))
                .unwrap_or(&Value::Null);
            let error = has_structured_error(result) || has_structured_error(content);
            has_error |= error;
            has_success |= !error;
            // The output hash includes typed result evidence so a changed result
            // for the same call is a new batch, without counting the call again.
            let output_hash = content_hash(&(content, error));
            calls.push(Call {
                id_hash: hash(id.as_bytes()),
                action_hash: content_hash(&(name, &args_hash)),
            });
            fingerprints.push((id, name, args_hash, output_hash));
        }
        Some(Batch {
            fingerprint: content_hash(&(turn, self.offset, fingerprints)),
            calls,
            has_error,
            has_success,
        })
    }
}

fn has_structured_error(value: &Value) -> bool {
    match value {
        Value::Array(parts) => parts.iter().any(has_structured_error),
        Value::Object(fields) => {
            fields.get("is_error").and_then(Value::as_bool) == Some(true)
                || fields.get("ok").and_then(Value::as_bool) == Some(false)
                || ["exit_code", "exitCode"].into_iter().any(|key| {
                    fields
                        .get(key)
                        .and_then(Value::as_i64)
                        .is_some_and(|code| code != 0)
                        || fields
                            .get(key)
                            .and_then(Value::as_u64)
                            .is_some_and(|code| code != 0)
                })
        }
        _ => false,
    }
}

fn content_hash(value: &impl serde::Serialize) -> String {
    hash(&serde_json::to_vec(value).expect("JSON values and string tuples are serializable"))
}

pub(super) fn extract(protocol: &str, body: &Value, turn: &str) -> Option<Batch> {
    let mut history = History::default();
    if protocol == "responses" {
        let input = body["input"].as_array()?;
        let mut previous_call = false;
        for (offset, item) in input.iter().enumerate() {
            match item["type"].as_str() {
                Some("function_call") => {
                    if !previous_call {
                        history.clear(offset);
                    }
                    history.call(&item["call_id"], &item["name"], &item["arguments"]);
                    previous_call = true;
                    continue;
                }
                Some("function_call_output") => history.result(&item["call_id"], item),
                _ if item["role"] == "user" && preview(&item["content"]).is_some() => {
                    history.clear(offset)
                }
                _ => (),
            }
            previous_call = false;
        }
    } else {
        for (offset, message) in body["messages"].as_array()?.iter().enumerate() {
            let parts = message["content"].as_array().into_iter().flatten();
            match message["role"].as_str() {
                Some("assistant") => {
                    if let Some(calls) = message["tool_calls"]
                        .as_array()
                        .filter(|calls| !calls.is_empty())
                    {
                        history.clear(offset);
                        for call in calls {
                            history.call(
                                &call["id"],
                                &call["function"]["name"],
                                &call["function"]["arguments"],
                            );
                        }
                    } else if parts.clone().any(|part| part["type"] == "tool_use") {
                        history.clear(offset);
                        for part in parts.filter(|part| part["type"] == "tool_use") {
                            history.call(&part["id"], &part["name"], &part["input"]);
                        }
                    }
                }
                Some("user") => {
                    let has_tool = parts.clone().any(|part| part["type"] == "tool_result");
                    if !has_tool && preview(&message["content"]).is_some() {
                        history.clear(offset);
                    }
                    for part in parts.filter(|part| part["type"] == "tool_result") {
                        history.result(&part["tool_use_id"], part);
                    }
                }
                Some("tool") => history.result(&message["tool_call_id"], message),
                _ => (),
            }
        }
    }
    history.batch(turn)
}

#[cfg(test)]
mod tests;
