use super::text::safe_text;
use super::{
    Builder, MAX_TEXT, NativeError, NativeResult, PAGE, Parsed, ProjectCache, Usage, Window,
};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, path::Path};

pub(in crate::sessions) fn opencode(
    path: &Path,
    history: crate::sessions::opencode::History,
    window: Option<Window<'_>>,
) -> NativeResult<Parsed> {
    opencode_inner(path, history, window, None)
}

pub(in crate::sessions) fn opencode_index(
    path: &Path,
    history: crate::sessions::opencode::History,
    projects: &mut ProjectCache,
) -> NativeResult<Parsed> {
    opencode_inner(path, history, None, Some(projects))
}

fn opencode_inner(
    path: &Path,
    history: crate::sessions::opencode::History,
    window: Option<Window<'_>>,
    projects: Option<&mut ProjectCache>,
) -> NativeResult<Parsed> {
    let id = history.info["id"]
        .as_str()
        .ok_or_else(|| NativeError::new("unsupported", "OpenCode 会话缺少身份"))?;
    let mut b = Builder::new("opencode", id, window.as_ref().map(|w| w.offset));
    b.native_id = Some(id.into());
    b.version = history.info["version"]
        .as_str()
        .map(|s| safe_text(s, 100).0);
    if let Some(title) = crate::sessions::opencode::native_title(&history.info) {
        b.title = Some(safe_text(title, 200).0);
        b.title_source = "native_title".into();
    } else if let Some(title) = &history.first_user_title {
        b.title_from(title);
    }
    if b.title.is_none() {
        b.title = Some("未命名会话".into());
    }
    b.project = history.info["directory"]
        .as_str()
        .filter(|s| Path::new(s).is_absolute())
        .map(|s| safe_text(s, 2048).0);
    let time = |ms: Option<i64>| {
        ms.and_then(DateTime::<Utc>::from_timestamp_millis)
            .map(|t| t.to_rfc3339())
    };
    b.created = time(history.info["created"].as_i64());
    b.updated = time(history.info["updated"].as_i64());
    let mut parts: HashMap<&str, Vec<&crate::sessions::opencode::Record>> = HashMap::new();
    for part in &history.parts {
        parts.entry(&part.parent).or_default().push(part);
    }
    let mut warnings = history.warnings;
    let mut line = 0;
    for message in &history.messages {
        let role = message.data["role"].as_str().unwrap_or("");
        if !["user", "assistant", "event"].contains(&role) {
            warnings.push("存在未识别角色的消息，未计入模型调用。".into());
        }
        b.turn = if role == "user" {
            message.id.clone()
        } else {
            message.data["parentID"]
                .as_str()
                .unwrap_or("unknown")
                .into()
        };
        b.model = message.data["modelID"]
            .as_str()
            .or_else(|| message.data["model"]["modelID"].as_str())
            .map(|s| safe_text(s, 300).0);
        b.provider = message.data["providerID"]
            .as_str()
            .or_else(|| message.data["model"]["providerID"].as_str())
            .map(|s| safe_text(s, 300).0);
        let timestamp = time(Some(message.created));
        let inherited = message.data["inherited"] == true;
        if !inherited {
            b.context
                .opencode_message(&message.data, timestamp.as_deref(), line);
        }
        for part in parts.remove(message.id.as_str()).unwrap_or_default() {
            line += 1;
            let v = &part.data;
            let ts = time(v["time"]["start"].as_i64().or(Some(part.created)));
            if !inherited {
                b.context.opencode_part(v, ts.as_deref(), line);
            }
            if message.data["summary"] == true
                && v["type"] == "text"
                && let Some(text) = v["text"].as_str()
            {
                b.context.summary_text(text);
            }
            match v["type"].as_str().unwrap_or("") {
                // These delimiters repeat the assistant usage snapshot. They
                // are not independent user actions or additional model calls.
                "step-start" | "step-finish" => {}
                "reasoning" if v["text"].as_str().is_some_and(|s| s.trim().is_empty()) => {}
                "text" | "reasoning" => {
                    let context = v["type"] == "reasoning" || v["synthetic"] == true;
                    let index = b.event(
                        &format!("part:{}", part.id),
                        if context { "event" } else { "message" },
                        if context {
                            if v["type"] == "reasoning" {
                                "公开推理记录"
                            } else {
                                "补充上下文"
                            }
                        } else if role == "user" {
                            "用户"
                        } else {
                            "OpenCode 回复"
                        },
                        ts.as_deref(),
                        line,
                    );
                    b.events[index].role = (!context).then(|| role.into());
                    b.events[index].model = b.model.clone();
                    if let Some(text) = v["text"].as_str() {
                        b.write_text(index, "text", text);
                    }
                }
                "tool" if inherited => {
                    let index = b.event(
                        &format!("part:{}", part.id),
                        "event",
                        "继承的工具记录",
                        ts.as_deref(),
                        line,
                    );
                    if let Some(name) = v["tool"].as_str() {
                        b.write_text(index, "text", name);
                    }
                }
                "tool" => {
                    let name = v["tool"].as_str().unwrap_or("未知工具");
                    let call_id = part.id.as_str();
                    let state = &v["state"];
                    let mut input = state["input"].clone();
                    if name == "skill" && input.is_object() && input["skill"].is_null() {
                        input["skill"] = input["name"].clone();
                    }
                    let index = b.tool(
                        call_id,
                        if name == "skill" { "Skill" } else { name },
                        &input,
                        ts.as_deref(),
                        line,
                    );
                    // Part IDs remain stable even if an upstream reuses a call ID.
                    b.events[index].native_id = part.id.clone();
                    if role == "assistant" {
                        b.events[index].model_response_id = Some(format!("model:{}", message.id));
                    }
                    let (server, source) = history.mcp.resolve(name, &v["mcp"]);
                    if let Some(server) = server {
                        b.events[index].kind = "mcp".into();
                        b.events[index].server = Some(server);
                    }
                    b.events[index].tool_source = Some(source);
                    if let Some(start) = time(state["time"]["start"].as_i64()) {
                        b.events[index].timestamp = Some(start);
                    }
                    let canonical = match name {
                        "write" => Some("Write"),
                        "edit" => Some("Edit"),
                        "apply_patch" => Some("apply_patch"),
                        _ => None,
                    };
                    if let (Some(tool), Some(cwd)) = (canonical, b.project.as_deref()) {
                        let mut structured = input.clone();
                        if let Some(p) = input["filePath"].as_str() {
                            structured["file_path"] = p.into();
                        }
                        if let Some(p) = input["patchText"].as_str() {
                            structured["patch"] = p.into();
                        }
                        let proposed = crate::projects::writes::proposed(tool, &structured, cwd);
                        if !proposed.is_empty() {
                            b.proposed_writes.insert(call_id.into(), proposed);
                        }
                    }
                    let end = time(state["time"]["end"].as_i64());
                    match state["status"].as_str() {
                        Some("completed" | "error") => {
                            let error = state["status"] == "error";
                            let output = if error {
                                &state["error"]
                            } else {
                                &state["output"]
                            };
                            b.result(call_id, output, error, end.as_deref(), line);
                            if !error && canonical.is_some() {
                                b.confirmed_writes.insert(call_id.into());
                            }
                            b.events[index].duration_ms = state["time"]["end"]
                                .as_u64()
                                .zip(state["time"]["start"].as_u64())
                                .and_then(|(end, start)| end.checked_sub(start));
                        }
                        Some("pending" | "running") => {}
                        _ => warnings.push("存在未识别的工具状态，未推断调用成功。".into()),
                    }
                }
                kind => {
                    let index = b.event(
                        &format!("part:{}", part.id),
                        "event",
                        &match kind {
                            "patch" => "项目文件快照变化".into(),
                            "compaction" => "对话压缩记录".into(),
                            "file" => "附件记录".into(),
                            _ => format!("原生事件 · {kind}"),
                        },
                        ts.as_deref(),
                        line,
                    );
                    if b.content(index) {
                        b.write_text(index, "text", &serde_json::to_string(v).unwrap_or_default());
                    }
                }
            }
        }
        if role == "assistant" && !inherited {
            line += 1;
            let index = b.event(
                &format!("model:{}", message.id),
                "model",
                "模型响应用量",
                timestamp.as_deref(),
                line,
            );
            let event = &mut b.events[index];
            event.model = b.model.clone();
            event.provider = b.provider.clone();
            event.end_time = time(message.data["time"]["completed"].as_i64());
            let failed = message.data.get("error").is_some_and(|v| !v.is_null());
            event.status = if failed {
                let mut failure =
                    crate::sessions::failure::native(&message.data["error"], "opencode_message");
                if !b
                    .window
                    .is_some_and(|start| (start..start + PAGE).contains(&index))
                {
                    failure.message = None;
                }
                let cancelled = failure.category == "cancelled";
                event.failure = Some(failure);
                event.name = if cancelled {
                    "模型调用已中止"
                } else {
                    "模型调用失败"
                }
                .into();
                if cancelled { "cancelled" } else { "error" }
            } else if event.end_time.is_none() && message.data["finish"].is_null() {
                "unknown"
            } else {
                "recorded"
            }
            .into();
            if let Some(tokens) = message
                .data
                .get("tokens")
                .filter(|v| v.is_object())
                .filter(|v| {
                    // Native clients initialize pending / failed responses with zeros.
                    // Such placeholders do not establish measured zero usage.
                    !failed && (!message.data["finish"].is_null() || event.end_time.is_some())
                        || [
                            v["input"].as_u64(),
                            v["output"].as_u64(),
                            v["reasoning"].as_u64(),
                            v["cache"]["read"].as_u64(),
                            v["cache"]["write"].as_u64(),
                            v["total"].as_u64(),
                        ]
                        .into_iter()
                        .flatten()
                        .any(|n| n > 0)
                })
            {
                let cache_read = tokens["cache"]["read"].as_u64();
                let cache_write = tokens["cache"]["write"].as_u64();
                let reasoning = tokens["reasoning"].as_u64();
                let input = tokens["input"].as_u64().map(|v| {
                    v.saturating_add(cache_read.unwrap_or(0))
                        .saturating_add(cache_write.unwrap_or(0))
                });
                let output = tokens["output"]
                    .as_u64()
                    .map(|v| v.saturating_add(reasoning.unwrap_or(0)));
                event.usage = Some(Usage {
                    input_tokens: input,
                    output_tokens: output,
                    cached_input_tokens: cache_read,
                    cache_write_input_tokens: cache_write,
                    reasoning_output_tokens: reasoning,
                    total_tokens: tokens["total"]
                        .as_u64()
                        .or_else(|| input.zip(output).map(|(a, b)| a.saturating_add(b))),
                });
                if b.content(index) {
                    b.events[index].raw_usage = Some(safe_text(&tokens.to_string(), MAX_TEXT).0);
                }
            }
        }
    }
    // Logs may carry a failed attempt while OpenCode is still retrying and the
    // assistant row has no final error. Keep these as separate diagnostic groups,
    // never as extra model responses or token/cost samples.
    for diagnostic in &history.diagnostics {
        // The log confirms the session, not a turn or response. Concurrent
        // prompts / retries cannot be assigned by timestamp proximity alone.
        b.turn = "unknown".into();
        b.model = Some(diagnostic.model.clone());
        b.provider = Some(diagnostic.provider.clone());
        let key = format!(
            "diagnostic:{:x}",
            Sha256::digest(
                serde_json::to_vec(&(
                    &b.turn,
                    &diagnostic.model,
                    &diagnostic.provider,
                    &diagnostic.failure.message
                ))
                .unwrap_or_default()
            )
        );
        if let Some(index) = b.lookup.get(&key).copied() {
            if let Some(failure) = &mut b.events[index].failure {
                failure.occurrences += 1;
                failure.last_observed_at = Some(diagnostic.timestamp.clone());
            }
            continue;
        }
        let index = b.event(
            &key,
            "diagnostic",
            "模型调用异常 · OpenCode 日志",
            Some(&diagnostic.timestamp),
            0,
        );
        let mut failure = diagnostic.failure.clone();
        failure.last_observed_at = Some(diagnostic.timestamp.clone());
        if !b.content(index) {
            failure.message = None;
        }
        b.events[index].failure = Some(failure);
        b.events[index].status = "error".into();
    }
    if !parts.is_empty() {
        warnings.push("存在未关联到消息的片段，未并入其他对话。".into());
    }
    // An anchor can point outside the first page; content is parsed again only
    // for that bounded page, retaining the same in-memory native read snapshot.
    let anchor_offset = window
        .as_ref()
        .and_then(|w| w.anchor)
        .map(|anchor| {
            b.events
                .iter()
                .position(|e| format!("opencode:{id}:{}", e.id) == anchor)
                .map(|n| n / PAGE * PAGE)
                .ok_or_else(|| NativeError::new("not_found", "调用在此 OpenCode 会话中不存在"))
        })
        .transpose()?;
    if let Some(offset) = anchor_offset.filter(|offset| Some(*offset) != b.window) {
        return opencode(
            path,
            crate::sessions::opencode::History {
                first_user_title: history.first_user_title,
                info: history.info,
                messages: history.messages,
                parts: history.parts,
                bytes: history.bytes,
                fingerprint: history.fingerprint,
                warnings,
                coverage_warnings: history.coverage_warnings,
                mcp: history.mcp,
                diagnostics: history.diagnostics,
            },
            Some(Window {
                offset,
                anchor: None,
            }),
        );
    }
    warnings.sort();
    warnings.dedup();
    let mut parsed = b.finish(
        path,
        history.bytes,
        None,
        warnings,
        window.is_some(),
        projects,
    )?;
    parsed.warnings.extend(history.coverage_warnings);
    parsed.warnings.sort();
    parsed.warnings.dedup();
    parsed.summary.message_count = history.messages.len();
    parsed.summary.disk_scope = "native_database_payload";
    let native_calls: HashMap<_, _> = history
        .parts
        .iter()
        .filter_map(|p| p.data["callID"].as_str().map(|call| (p.id.as_str(), call)))
        .collect();
    for event in &mut parsed.events {
        if let Some(native) = event.call_id.as_deref().and_then(|id| native_calls.get(id)) {
            event.call_id = Some((*native).into());
        }
    }
    for write in &mut parsed.writes {
        if let Some(native) = native_calls.get(write.call_id.as_str()) {
            write.call_id = (*native).into();
        }
    }
    parsed.fingerprint = history.fingerprint;
    Ok(parsed)
}
