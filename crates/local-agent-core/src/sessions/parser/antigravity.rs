use super::{Builder, NativeError, NativeResult, Parsed, Usage, Window, safe_text};
use crate::sessions::{antigravity::History, antigravity_proto::Message};
use serde_json::{Value, json};
use std::{collections::HashMap, path::Path};

pub(in crate::sessions) fn model(meta: &Message<'_>) -> Option<String> {
    let info = meta.child(24).ok()?;
    info.text(8)
        .or(info.text(12))
        .filter(|s| !s.is_empty())
        .map(|s| safe_text(s, 300).0)
        .or_else(|| {
            meta.number(11)
                .filter(|n| *n != 0)
                .map(|n| format!("Antigravity model #{n}"))
        })
}
fn state(status: u64) -> &'static str {
    match status {
        3 => "recorded",
        6 | 12 => "cancelled",
        7 => "error",
        _ => "unknown",
    }
}
pub(in crate::sessions) fn parse(
    path: &Path,
    h: History,
    window: Option<Window<'_>>,
) -> NativeResult<Parsed> {
    if let Some(anchor) = window.as_ref().and_then(|w| w.anchor) {
        let parsed = build(path, &h, None)?;
        let index = parsed
            .events
            .iter()
            .position(|e| e.id == anchor)
            .ok_or_else(|| NativeError::new("not_found", "此调用不存在或来源已更新"))?;
        return build(path, &h, Some(index / 40 * 40));
    }
    build(path, &h, window.map(|w| w.offset))
}
fn build(path: &Path, h: &History, window: Option<usize>) -> NativeResult<Parsed> {
    let mut b = Builder::new("antigravity", &h.id, window);
    b.native_id = Some(h.id.clone());
    b.product = Some("antigravity".into());
    b.product_evidence = Some("native_app_store");
    let metadata = Message::parse(&h.metadata)?;
    let mut projects = std::collections::BTreeSet::new();
    for raw in metadata.all(1) {
        let workspace = Message::parse(raw)?;
        if let Some(path) = workspace
            .text(1)
            .and_then(|s| url::Url::parse(s).ok())
            .filter(|u| u.scheme() == "file")
            .and_then(|u| u.to_file_path().ok())
            .filter(|p| p.is_absolute())
        {
            projects.insert(path.to_string_lossy().into_owned());
        }
    }
    if projects.len() == 1 {
        b.project = projects.into_iter().next();
    }
    b.created = metadata.time(2);
    let mut warnings = h.warnings.clone();
    let mut proposed_by = HashMap::new();
    let mut ambiguous_proposals = std::collections::HashSet::new();
    let mut native_ids = HashMap::new();
    for step in &h.steps {
        let result: NativeResult<()> = (|| {
            let payload = Message::parse(&step.payload)?;
            if payload.number(1) != Some(step.kind) || payload.number(4).unwrap_or(0) != step.status
            {
                return Err(NativeError::new(
                    "unsupported",
                    "步骤身份与数据库字段不一致",
                ));
            }
            let meta = payload.child(5)?;
            let ts = meta.time(32).or_else(|| meta.time(1));
            let end = meta.time(8);
            if let Some(t) = end.as_ref().or(ts.as_ref()) {
                if b.updated.as_ref().is_none_or(|old| t > old) {
                    b.updated = Some(t.clone());
                }
                if b.created.is_none() {
                    b.created = Some(t.clone());
                }
            }
            let key = format!("step:{}", step.index);
            let line = step.index + 1;
            if step.kind == 14 {
                let user = payload.child(19)?;
                let mut texts = Vec::new();
                for raw in user.all(3) {
                    if let Some(s) = Message::parse(raw)?.text(1) {
                        texts.push(s.to_owned());
                    }
                }
                if texts.is_empty()
                    && let Some(text) = user.text(1).or(user.text(2))
                {
                    texts.push(text.to_owned());
                }
                let text = texts.join("\n");
                b.turn = key.clone();
                proposed_by.clear();
                ambiguous_proposals.clear();
                if b.title.is_none() && !text.trim().is_empty() {
                    b.title = Some(safe_text(text.trim(), 200).0);
                    b.title_source = "first_user_message".into();
                }
                let i = b.event(&key, "message", "用户", ts.as_deref(), line);
                b.events[i].role = Some("user".into());
                b.write_text(i, "text", &text);
                return Ok(());
            }
            b.model = model(&meta);
            if step.kind == 17 {
                let error = payload.child(24)?.child(3)?;
                let i = b.event(
                    &key,
                    "event",
                    "Antigravity 原生异常 · 调用归属未知",
                    ts.as_deref(),
                    line,
                );
                b.events[i].status = "error".into();
                if let Some(message) = error.text(1).or(error.text(2)).or(error.text(3)) {
                    b.write_text(i, "text", message);
                }
                // Error-message steps do not establish a separate model request.
                return Ok(());
            }
            if step.kind == 15 {
                let response = payload.child(20)?;
                let i = b.event(
                    &format!("model:{key}"),
                    "model",
                    "Antigravity 回复",
                    ts.as_deref(),
                    line,
                );
                b.events[i].role = Some("assistant".into());
                b.events[i].model = b.model.clone();
                b.events[i].end_time = end;
                b.events[i].status = state(step.status).into();
                if let Some(text) = response.text(8).or(response.text(1)) {
                    b.write_text(i, "text", text);
                }
                // Hidden thinking, signatures and provider headers never cross IPC.
                if let Some(raw) = meta.bytes(9).filter(|_| step.status == 3) {
                    let usage = Message::parse(raw)?;
                    let input = usage.number(2).unwrap_or(0);
                    let output = usage.number(3).unwrap_or(0);
                    if input.saturating_add(output) > 0
                        && input <= i64::MAX as u64
                        && output <= i64::MAX as u64
                    {
                        b.events[i].usage = Some(Usage {
                            input_tokens: Some(input),
                            output_tokens: Some(output),
                            total_tokens: input.checked_add(output),
                            cached_input_tokens: usage.number(5),
                            cache_write_input_tokens: usage.number(4),
                            reasoning_output_tokens: usage.number(9),
                        });
                        b.events[i].raw_usage = b.content(i).then(||json!({"input_tokens":input,"output_tokens":output,"cache_read_tokens":usage.number(5),"cache_write_tokens":usage.number(4)}).to_string());
                    }
                }
                if step.status == 7 {
                    let error = payload.child(31)?;
                    let message = error.text(1).or(error.text(2)).or(error.text(3));
                    let mut failure = crate::sessions::failure::native(
                        &json!({"message":message}),
                        "antigravity_step",
                    );
                    if !b.content(i) {
                        failure.message = None;
                    }
                    b.events[i].failure = Some(failure);
                }
                for raw in response.all(7) {
                    let tool = Message::parse(raw)?;
                    if let Some(id) = tool.text(1) {
                        if ambiguous_proposals.contains(id) {
                            continue;
                        }
                        if proposed_by
                            .insert(id.to_owned(), format!("model:{key}"))
                            .is_some()
                        {
                            proposed_by.remove(id);
                            ambiguous_proposals.insert(id.to_owned());
                        }
                    }
                }
                return Ok(());
            }
            let mcp = payload.child(47)?;
            let tool = if step.kind == 38 {
                mcp.child(2)?
            } else {
                meta.child(4)?
            };
            if let Some(name) = tool.text(2).filter(|s| !s.is_empty()) {
                let input = tool
                    .text(3)
                    .and_then(|s| serde_json::from_str::<Value>(s).ok())
                    .unwrap_or(Value::Null);
                // Step-local IDs avoid collapsing retries or duplicated native call IDs.
                let i = b.tool(&key, name, &input, ts.as_deref(), line);
                let canonical_write = match name {
                    "write_to_file" => Some("Write"),
                    "replace_file_content" | "multi_replace_file_content" => Some("Edit"),
                    _ => None,
                };
                if let (Some(canonical), Some(project), Some(file)) = (
                    canonical_write,
                    b.project.as_deref(),
                    input["TargetFile"].as_str(),
                ) {
                    let files = crate::projects::writes::proposed(
                        canonical,
                        &json!({"file_path":file}),
                        project,
                    );
                    if !files.is_empty() {
                        b.proposed_writes.insert(key.clone(), files);
                    }
                    if step.status == 3 {
                        b.confirmed_writes.insert(key.clone());
                    }
                }
                b.events[i].model = b.model.clone();
                if let Some(id) = tool.text(1) {
                    native_ids.insert(key.clone(), safe_text(id, 512).0);
                    b.events[i].model_response_id = proposed_by.get(id).cloned();
                }
                if step.kind == 38 {
                    b.events[i].kind = "mcp".into();
                    b.events[i].server = mcp.text(1).map(|s| safe_text(s, 512).0);
                }
                let output = if step.kind == 38 {
                    mcp.text(3)
                } else if step.kind == 132 {
                    payload.child(140)?.child(2)?.text(1)
                } else if step.kind == 21 {
                    payload.child(28)?.child(21)?.text(1)
                } else {
                    None
                };
                if matches!(step.status, 3 | 7) {
                    b.result(&key, &json!(output), step.status == 7, end.as_deref(), line);
                    if let Some(error) = payload.child(31)?.text(1).filter(|_| step.status == 7) {
                        b.write_text(i, "output", error);
                    }
                } else {
                    b.events[i].status = if matches!(step.status, 6 | 12) {
                        "cancelled"
                    } else {
                        "missing_result"
                    }
                    .into();
                }
                b.events[i].duration_ms = ts.as_ref().zip(end.as_ref()).and_then(|(a, z)| {
                    let ms = chrono::DateTime::parse_from_rfc3339(z)
                        .ok()?
                        .signed_duration_since(chrono::DateTime::parse_from_rfc3339(a).ok()?)
                        .num_milliseconds();
                    u64::try_from(ms).ok()
                });
            } else {
                let i = b.event(
                    &key,
                    "event",
                    &format!("Antigravity 步骤 #{}", step.kind),
                    ts.as_deref(),
                    line,
                );
                b.events[i].status = state(step.status).into();
                b.events[i].end_time = end;
            }
            Ok(())
        })();
        if result.is_err() {
            warnings.push("存在损坏或结构未支持的步骤，已保留其余可读内容".into());
        }
    }
    warnings.sort();
    warnings.dedup();
    let mut parsed = b.finish(path, h.bytes, None, warnings, window.is_some(), None)?;
    parsed.summary.disk_scope = "native_database_payload";
    parsed.fingerprint = h.fingerprint.clone();
    for e in &mut parsed.events {
        if let Some(id) = e.call_id.as_ref().and_then(|k| native_ids.get(k)) {
            e.call_id = Some(id.clone());
        }
    }
    for write in &mut parsed.writes {
        if let Some(id) = native_ids.get(&write.call_id) {
            write.call_id = id.clone();
        }
    }
    Ok(parsed)
}
