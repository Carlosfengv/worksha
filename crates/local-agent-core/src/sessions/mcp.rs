//! Configuration inventory + authorized native-history metadata. This is not a
//! live MCP client, and never executes config commands or resolves credentials.
pub(super) mod config_sources;
use super::mcp_metrics::{self, McpFacets, McpGroup};
use super::{
    CallSummary, NativeError, NativeResult, NativeSessions, SourceStatus, now,
    opencode_mcp::{self, ConfigSource},
    sanitize_label, unique_files,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpQuery {
    pub agent: Option<String>,
    pub days: Option<u16>,
    pub service: Option<String>,
    pub model: Option<String>,
    pub project: Option<String>,
    /// Locate a native call without narrowing its service's aggregate samples.
    pub call: Option<String>,
    pub cursor: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpService {
    pub id: String,
    pub name: String,
    pub agent_id: String,
    pub applicable_agent_ids: Vec<String>,
    pub project_path: Option<String>,
    pub sources: Vec<ConfigSource>,
    pub transport: Option<String>,
    pub configured_enabled: Option<bool>,
    pub configuration_status: &'static str,
    pub connection_status: &'static str,
    pub observation_status: &'static str,
    pub call_count: Option<usize>,
    pub returned_count: usize,
    pub error_count: usize,
    pub unknown_count: usize,
    pub duration_samples: usize,
    pub p95_ms: Option<u64>,
    pub error_rate: Option<f64>,
    pub recorded_tool_count: usize,
    pub tool_catalog_count: Option<usize>,
    pub session_count: usize,
    pub last_call_at: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCall {
    pub service_id: String,
    pub group_id: String,
    pub call: CallSummary,
    pub related_usage: Option<super::Usage>,
    pub related_agent_product_id: Option<String>,
    pub related_usage_state: &'static str,
    pub project_path: Option<String>,
    pub association: &'static str,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSnapshot {
    pub services: Vec<McpService>,
    pub groups: Vec<McpGroup>,
    pub usage_summary: mcp_metrics::McpUsage,
    pub facets: McpFacets,
    pub calls: Vec<McpCall>,
    pub total_calls: usize,
    pub next_cursor: Option<String>,
    pub selected_service_id: Option<String>,
    pub selected_group_id: Option<String>,
    pub sources: Vec<SourceStatus>,
    pub as_of: String,
    pub history_as_of: String,
    pub coverage: &'static str,
    pub warnings: Vec<String>,
    pub applied_filter: McpQuery,
}
#[derive(Clone)]
pub(super) struct Inventory {
    directory: std::sync::Arc<crate::agents::AgentDirectory>,
    services: BTreeMap<String, McpService>,
    warnings: BTreeSet<String>,
    blocked_agents: BTreeSet<String>,
    deadline: Option<std::time::Instant>,
    versions: BTreeMap<PathBuf, String>,
}
fn id(agent: &str, name: &str, project: Option<&str>, sources: &[ConfigSource]) -> String {
    crate::projects::hash(
        &serde_json::json!([
            agent,
            name,
            project,
            sources.iter().map(|s| &s.path).collect::<Vec<_>>()
        ])
        .to_string(),
    )
}
impl Inventory {
    pub(super) fn catalog(
        self,
        sources: &[String],
        project_ids: &BTreeMap<String, String>,
    ) -> crate::domain::mcp::McpCatalogSnapshot {
        use crate::domain::mcp::{McpCatalogSnapshot, McpConfiguration};
        let expired = !self.within_budget();
        McpCatalogSnapshot {
            entries: self
                .services
                .into_values()
                .map(|s| McpConfiguration {
                    id: s.id,
                    source_id: s.agent_id,
                    name: s.name,
                    applicable_product_ids: s.applicable_agent_ids,
                    project_ref: s
                        .project_path
                        .as_ref()
                        .and_then(|p| project_ids.get(p))
                        .cloned(),
                    transport: s.transport,
                    configured_enabled: s.configured_enabled,
                })
                .collect(),
            partial_sources: sources
                .iter()
                .filter(|s| expired || self.blocked_agents.contains(*s))
                .cloned()
                .collect(),
            observed_at: now(),
            configuration_revision: crate::projects::hash(
                &serde_json::json!(
                    self.versions
                        .iter()
                        .map(|(p, v)| (p.to_string_lossy(), v))
                        .collect::<Vec<_>>()
                )
                .to_string(),
            ),
        }
    }
    pub(super) fn within_budget(&self) -> bool {
        self.deadline
            .is_none_or(|d| std::time::Instant::now() < d && self.services.len() < 4096)
    }
    pub(super) fn codex_global(&mut self, home: &Path) {
        self.codex(&home.join(".codex/config.toml"), None);
    }
    fn add(
        &mut self,
        agent: &str,
        name: &str,
        project: Option<&str>,
        sources: Vec<ConfigSource>,
        transport: Option<&str>,
        enabled: Option<bool>,
    ) -> String {
        let key = id(agent, name, project, &sources);
        if !self.within_budget() {
            self.blocked_agents.insert(agent.into());
            return key;
        }
        self.services
            .entry(key.clone())
            .or_insert_with(|| McpService {
                id: key.clone(),
                name: sanitize_label(name, 512),
                agent_id: agent.into(),
                applicable_agent_ids: self
                    .directory
                    .products()
                    .iter()
                    .filter(|p| p.family == agent)
                    .map(|p| p.id.clone())
                    .collect(),
                project_path: project.map(|p| sanitize_label(p, 2048)),
                sources,
                transport: transport.map(str::to_owned),
                configured_enabled: enabled,
                configuration_status: "present",
                connection_status: "unknown",
                observation_status: "unknown",
                call_count: None,
                returned_count: 0,
                error_count: 0,
                unknown_count: 0,
                duration_samples: 0,
                p95_ms: None,
                error_rate: None,
                recorded_tool_count: 0,
                tool_catalog_count: None,
                session_count: 0,
                last_call_at: None,
            });
        key
    }
    fn unknown(&mut self, agent: &str, server: Option<&str>, project: Option<&str>) -> String {
        let key = self.add(
            agent,
            server.unwrap_or("来源未知"),
            project,
            vec![],
            None,
            None,
        );
        let service = self.services.get_mut(&key).unwrap();
        service.configuration_status = "unknown";
        service.applicable_agent_ids.clear();
        key
    }
    fn issue(&mut self, agent: &str, path: &Path) {
        self.blocked_agents.insert(agent.into());
        self.warnings.insert(format!(
            "{agent} 配置未完整读取或结构不受支持：{}",
            sanitize_label(&path.to_string_lossy(), 2048)
        ));
    }
    fn json(&mut self, agent: &str, path: &Path) -> Option<(Value, Option<String>)> {
        if !self.within_budget() {
            self.issue(agent, path);
            return None;
        }
        let read = if self.deadline.is_some() {
            opencode_mcp::read_observed(path).map(|value| {
                self.versions.insert(
                    path.into(),
                    value
                        .as_ref()
                        .map(|(_, _, v)| v.clone())
                        .unwrap_or_else(|| "missing".into()),
                );
                value.map(|(data, modified, _)| (data, modified))
            })
        } else {
            opencode_mcp::read(path)
        };
        match read {
            Ok(value) => value,
            Err(()) => {
                self.issue(agent, path);
                None
            }
        }
    }
    fn json_entries(
        &mut self,
        agent: &str,
        value: &Value,
        path: &Path,
        modified: Option<String>,
        scope: &'static str,
        project: Option<&str>,
    ) {
        if value.is_null() {
            return;
        }
        let Some(entries) = value.as_object().filter(|m| m.len() <= 256) else {
            self.issue(agent, path);
            return;
        };
        for (name, entry) in entries {
            if !self.within_budget() {
                self.issue(agent, path);
                break;
            }
            if name.is_empty() || name.len() > 512 || !entry.is_object() {
                self.issue(agent, path);
                continue;
            }
            let transport = match entry["type"].as_str() {
                Some("stdio") => Some("stdio"),
                Some("http" | "streamable-http") => Some("streamable_http"),
                Some("sse") => Some("sse"),
                Some("ws") => Some("websocket"),
                None if entry["command"].as_str().is_some() => Some("stdio"),
                _ => None,
            };
            self.add(
                agent,
                name,
                project,
                vec![ConfigSource {
                    path: sanitize_label(&path.to_string_lossy(), 2048),
                    scope,
                    modified_at: modified.clone(),
                }],
                transport,
                entry["enabled"].as_bool(),
            );
        }
    }
    fn codex(&mut self, path: &Path, project: Option<&str>) {
        if !self.within_budget() {
            self.issue("codex", path);
            return;
        }
        let read = if self.deadline.is_some() {
            opencode_mcp::read_text_observed(path).map(|value| {
                self.versions.insert(
                    path.into(),
                    value
                        .as_ref()
                        .map(|(_, _, v)| v.clone())
                        .unwrap_or_else(|| "missing".into()),
                );
                value.map(|(data, modified, _)| (data, modified))
            })
        } else {
            opencode_mcp::read_text(path)
        };
        let (raw, modified) = match read {
            Ok(Some(v)) => v,
            Ok(None) => return,
            Err(()) => {
                self.issue("codex", path);
                return;
            }
        };
        let Ok(doc) = raw.parse::<toml_edit::DocumentMut>() else {
            self.issue("codex", path);
            return;
        };
        let Some(entries) = doc.get("mcp_servers") else {
            return;
        };
        let Some(table) = entries.as_table_like().filter(|m| m.len() <= 256) else {
            self.issue("codex", path);
            return;
        };
        for (name, entry) in table.iter() {
            if !self.within_budget() {
                self.issue("codex", path);
                break;
            }
            if name.is_empty() || name.len() > 512 || entry.as_table_like().is_none() {
                self.issue("codex", path);
                continue;
            }
            let command = entry
                .get("command")
                .and_then(toml_edit::Item::as_str)
                .is_some();
            let url = entry.get("url").and_then(toml_edit::Item::as_str).is_some();
            let transport = match (command, url) {
                (true, false) => Some("stdio"),
                (false, true) => Some("streamable_http"),
                _ => None,
            };
            self.add(
                "codex",
                name,
                project,
                vec![ConfigSource {
                    path: sanitize_label(&path.to_string_lossy(), 2048),
                    scope: if project.is_some() {
                        "project"
                    } else {
                        "local"
                    },
                    modified_at: modified.clone(),
                }],
                transport,
                entry.get("enabled").and_then(toml_edit::Item::as_bool),
            );
        }
    }
}
fn valid_project(path: &str) -> bool {
    let p = Path::new(path);
    p.is_absolute()
        && path.len() <= 2048
        && !p.components().any(|c| matches!(c, Component::ParentDir))
}
// Known filenames only; no recursive project search or plugin discovery.
fn project_dirs(home: &Path, cwd: &str) -> Vec<PathBuf> {
    if !valid_project(cwd) {
        return vec![];
    }
    let mut dirs = vec![];
    let mut git = false;
    for p in Path::new(cwd).ancestors().take(32) {
        dirs.push(p.to_path_buf());
        git = p.join(".git").symlink_metadata().is_ok();
        if git || p == home {
            break;
        }
    }
    if !git && !Path::new(cwd).starts_with(home) {
        dirs.truncate(1);
    }
    dirs
}

impl NativeSessions {
    pub fn mcp(&self, query: McpQuery) -> NativeResult<McpSnapshot> {
        self.mcp_with_hidden_projects(query, &Default::default())
    }

    pub fn mcp_with_hidden_projects(
        &self,
        query: McpQuery,
        hidden_paths: &std::collections::HashSet<String>,
    ) -> NativeResult<McpSnapshot> {
        self.domain_mcp_with_hidden_projects(query, hidden_paths)
    }

    pub(super) fn read_native_mcp(
        &self,
        query: McpQuery,
        hidden_paths: &std::collections::HashSet<String>,
    ) -> NativeResult<McpSnapshot> {
        let hidden_facets: BTreeSet<_> = hidden_paths
            .iter()
            .map(|p| mcp_metrics::facet_id("project", Some(p)))
            .collect();
        if query
            .project
            .as_ref()
            .is_some_and(|p| hidden_facets.contains(p))
        {
            return Err(NativeError::new(
                "project_hidden",
                "所选项目已隐藏，请清除项目筛选或在项目页恢复。",
            ));
        }
        self.validate_agent(query.agent.as_deref())?;
        if query.days.is_some_and(|d| ![7, 30].contains(&d))
            || [&query.model, &query.project]
                .into_iter()
                .flatten()
                .any(|v| {
                    v != "unknown" && (v.len() != 64 || !v.bytes().all(|c| c.is_ascii_hexdigit()))
                })
            || query.call.as_ref().is_some_and(|id| {
                id.is_empty() || id.len() > 4096 || id.chars().any(char::is_control)
            })
            || query
                .service
                .as_ref()
                .is_some_and(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(NativeError::new("invalid_input", "MCP 筛选无效"));
        }
        let (files, sources, revision, history_as_of) = {
            let state = self.lock()?;
            (
                unique_files(&state)
                    .into_iter()
                    .filter(|f| {
                        state
                            .sources
                            .iter()
                            .any(|s| s.agent_id == f.agent && s.enabled)
                    })
                    .cloned()
                    .collect::<Vec<_>>(),
                state.sources.clone(),
                state.revision,
                state.as_of.clone(),
            )
        };
        let metadata = files.iter().map(ConfigFile::from).collect::<Vec<_>>();
        let (mut inventory, opencode_contexts) =
            self.mcp_configuration(&sources, &metadata, None)?;
        let start = query
            .days
            .map(|d| Utc::now().timestamp() - i64::from(d) * 86400);
        let mut calls = vec![];
        let mut unknown_dates = 0;
        for file in &files {
            if let Some(start) = start {
                let created = file
                    .summary
                    .created_at
                    .as_deref()
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok());
                if created.is_none() {
                    unknown_dates += 1;
                }
                if created.is_none_or(|d| d.timestamp() < start) {
                    continue;
                }
            }
            for original in file.calls.iter() {
                let unknown = original
                    .tool_source
                    .as_ref()
                    .is_some_and(|s| s.status == "unknown");
                if original.kind != "mcp" && !unknown {
                    continue;
                }
                let mut call = original.clone();
                let project = file.summary.project_path.as_deref();
                let mut association = "unknown";
                let mut matched = None;
                if file.agent == "opencode"
                    && let Some(context) = project.and_then(|p| opencode_contexts.get(p))
                {
                    let (server, source) = context.resolve(&call.name, &Value::Null);
                    // Explicit native evidence in the index remains authoritative.
                    if call
                        .tool_source
                        .as_ref()
                        .is_none_or(|s| s.status != "native")
                    {
                        call.server = server;
                        call.tool_name = source.tool_name.clone();
                        call.tool_source = Some(source);
                        call.kind = if call.server.is_some() { "mcp" } else { "tool" }.into();
                    }
                }
                if let Some(server) = call.server.as_deref()
                    && !inventory.blocked_agents.contains(&file.agent)
                    && (file.agent != "opencode"
                        || project
                            .and_then(|p| opencode_contexts.get(p))
                            .is_some_and(|c| c.warnings.is_empty()))
                {
                    let candidates: Vec<_> = inventory
                        .services
                        .values()
                        .filter(|s| {
                            s.agent_id == file.agent
                                && s.name == server
                                && s.configuration_status == "present"
                        })
                        .filter(|s| {
                            s.project_path.as_deref().is_none_or(|p| {
                                project.is_some_and(|cwd| Path::new(cwd).starts_with(p))
                            })
                        })
                        .filter(|s| {
                            file.agent != "opencode"
                                || call.tool_source.as_ref().is_none_or(|t| {
                                    t.status == "native"
                                        || t.sources
                                            .iter()
                                            .map(|x| &x.path)
                                            .eq(s.sources.iter().map(|x| &x.path))
                                })
                        })
                        .collect();
                    // For native history, several current configurations can share
                    // a server name. Do not manufacture a historical config link.
                    let candidates = if file.agent == "claude" {
                        let rank = |s: &&McpService| {
                            (
                                s.sources
                                    .first()
                                    .is_some_and(|x| x.scope == "project_local")
                                    as usize,
                                s.project_path.as_ref().map_or(0, String::len),
                            )
                        };
                        let max = candidates.iter().map(rank).max();
                        candidates
                            .into_iter()
                            .filter(|s| Some(rank(s)) == max)
                            .collect::<Vec<_>>()
                    } else {
                        candidates
                    };
                    if candidates.len() == 1 {
                        matched = Some(candidates[0].id.clone());
                        association = "current_configuration";
                    }
                }
                let service_id = matched.unwrap_or_else(|| {
                    inventory.unknown(&file.agent, call.server.as_deref(), project)
                });
                let service = &inventory.services[&service_id];
                let group_id = mcp_metrics::group_id(service);
                let response = call.model_response_id.as_ref().and_then(|response| {
                    file.calls
                        .iter()
                        .find(|c| &c.id == response && c.kind == "model")
                });
                let response_matches = response.is_some_and(|c| {
                    self.directory().matches(
                        query.agent.as_deref(),
                        &c.agent_id,
                        c.agent_product_id.as_deref(),
                    )
                });
                let related_usage = response
                    .filter(|_| response_matches)
                    .and_then(|c| c.usage.clone());
                let related_usage_state = if response.is_some() && !response_matches {
                    "outside_agent_scope"
                } else if related_usage
                    .as_ref()
                    .is_none_or(|u| u.total_tokens.is_none())
                {
                    "unknown"
                } else {
                    "recorded"
                };
                calls.push(McpCall {
                    service_id,
                    group_id,
                    related_usage,
                    related_agent_product_id: response.and_then(|c| c.agent_product_id.clone()),
                    related_usage_state,
                    call,
                    project_path: project.map(str::to_owned),
                    association,
                });
            }
        }
        if unknown_dates > 0 {
            inventory.warnings.insert(format!(
                "当前时间范围跳过 {unknown_dates} 个开始时间未知的会话，调用量覆盖不完整。"
            ));
        }
        // Select the logical MCP name across Agent / project configurations.
        // Old configuration IDs remain valid bookmarks, scoped to that source.
        let selected_group_id = query
            .service
            .as_ref()
            .map(|selector| {
                inventory
                    .services
                    .values()
                    .find_map(|s| {
                        let group = mcp_metrics::group_id(s);
                        (selector == &s.id || selector == &group).then_some(group)
                    })
                    .ok_or_else(|| {
                        NativeError::new("not_found", "MCP 服务已变化，请返回列表重新选择")
                    })
            })
            .transpose()?;
        calls.retain(|row| {
            self.directory().matches(
                query.agent.as_deref(),
                &row.call.agent_id,
                row.call.agent_product_id.as_deref(),
            )
        });
        let mut facets = mcp_metrics::facets(&calls, selected_group_id.as_deref());
        facets.projects.retain(|p| !hidden_facets.contains(&p.id));
        calls.retain(|row| {
            query.model.as_ref().is_none_or(|model| {
                model == &mcp_metrics::facet_id("model", row.call.model.as_deref())
            }) && query.project.as_ref().is_none_or(|project| {
                project == &mcp_metrics::facet_id("project", row.project_path.as_deref())
            }) && query
                .service
                .as_ref()
                .is_none_or(|selector| selector == &row.service_id || selector == &row.group_id)
        });
        for service in inventory.services.values_mut() {
            let source = sources
                .iter()
                .find(|s| s.agent_id == service.agent_id)
                .unwrap();
            if source.enabled && matches!(source.status.as_str(), "ready" | "partial") {
                service.call_count = Some(0);
                service.observation_status = "native_history";
            } else if !source.enabled {
                service.observation_status = "not_enabled";
            }
            let rows: Vec<_> = calls
                .iter()
                .filter(|c| c.service_id == service.id)
                .collect();
            if !rows.is_empty() {
                service.call_count = Some(rows.len());
                service.observation_status = "native_history";
            }
            service.returned_count = rows.iter().filter(|c| c.call.status == "returned").count();
            service.error_count = rows.iter().filter(|c| c.call.status == "error").count();
            service.unknown_count = rows.len() - service.returned_count - service.error_count;
            let mut durations: Vec<_> = rows
                .iter()
                .filter(|c| matches!(c.call.status.as_str(), "returned" | "error"))
                .filter_map(|c| c.call.duration_ms)
                .collect();
            durations.sort_unstable();
            service.duration_samples = durations.len();
            service.p95_ms = durations
                .get((durations.len() * 95).div_ceil(100).saturating_sub(1))
                .copied();
            let ended = service.returned_count + service.error_count;
            service.error_rate = (ended > 0).then(|| service.error_count as f64 / ended as f64);
            service.recorded_tool_count = rows
                .iter()
                .map(|c| &c.call.name)
                .collect::<BTreeSet<_>>()
                .len();
            service.session_count = rows
                .iter()
                .map(|c| &c.call.session_id)
                .collect::<BTreeSet<_>>()
                .len();
            service.last_call_at = rows.iter().filter_map(|c| c.call.timestamp.clone()).max();
        }
        let mut services: Vec<_> = inventory
            .services
            .into_values()
            .filter(|s| {
                query.agent.as_deref().is_none_or(|a| {
                    (matches!(a, "claude" | "codex" | "opencode") && a == s.agent_id)
                        || s.applicable_agent_ids.iter().any(|id| id == a)
                        || calls.iter().any(|c| c.service_id == s.id)
                })
            })
            .collect();
        let groups = mcp_metrics::groups(&services, &calls);
        let usage_summary = mcp_metrics::usage(
            calls.iter(),
            calls.is_empty() && services.iter().any(|s| s.call_count.is_some()),
        );
        services.sort_by(|a, b| {
            (
                a.configuration_status != "present",
                &a.name,
                &a.agent_id,
                &a.project_path,
            )
                .cmp(&(
                    b.configuration_status != "present",
                    &b.name,
                    &b.agent_id,
                    &b.project_path,
                ))
        });
        let selected_service_id = if let Some(call_id) = &query.call {
            let service = calls
                .iter()
                .find(|c| &c.call.id == call_id)
                .map(|c| c.service_id.clone())
                .ok_or_else(|| {
                    NativeError::new("not_found", "调用不在当前已授权会话范围，请刷新后重试")
                })?;
            calls.retain(|c| &c.call.id == call_id);
            Some(service)
        } else {
            query
                .service
                .clone()
                .filter(|id| services.iter().any(|s| &s.id == id))
        };
        let selected_group_id = selected_group_id.or_else(|| {
            calls
                .first()
                .filter(|_| query.call.is_some())
                .map(|row| row.group_id.clone())
        });
        calls.sort_by(|a, b| {
            b.call
                .timestamp
                .cmp(&a.call.timestamp)
                .then(a.call.id.cmp(&b.call.id))
        });
        // Config inventory and all call metadata participate, even without a
        // native index revision change. This prevents cross-filter/stale pages.
        let fingerprint = crate::projects::hash(
            &serde_json::json!([
                revision,
                query.agent,
                query.days,
                query.service,
                query.model,
                query.project,
                query.call,
                hidden_facets,
                services,
                groups,
                calls
            ])
            .to_string(),
        );
        let current_sources = self.sources()?;
        if self.revision()? != revision
            || sources.iter().any(|source| {
                current_sources
                    .iter()
                    .find(|s| s.agent_id == source.agent_id)
                    .is_none_or(|s| {
                        s.enabled != source.enabled
                            || s.authorization_revision != source.authorization_revision
                    })
            })
        {
            return Err(NativeError::new(
                "stale_snapshot",
                "会话授权已改变，请重新读取 MCP 数据",
            ));
        }
        let offset = super::cursor(query.cursor.as_deref(), &fingerprint)?;
        let total = calls.len();
        inventory.warnings.insert("只读当前标准配置及已授权的原生历史。当前配置关联不代表历史配置、当前连接或实时 MCP 观测；系统托管、插件、远程和运行时配置未读取，项目批准 / 信任状态未知。".into());
        Ok(McpSnapshot {
            services,
            groups,
            usage_summary,
            facets,
            calls: calls.into_iter().skip(offset).take(40).collect(),
            total_calls: total,
            next_cursor: (offset + 40 < total).then(|| format!("{fingerprint}:{}", offset + 40)),
            selected_service_id,
            selected_group_id,
            sources,
            as_of: now(),
            history_as_of,
            coverage: "partial",
            warnings: inventory.warnings.into_iter().collect(),
            applied_filter: query,
        })
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum ConfigPhase {
    Global,
    Projects,
}
pub(super) struct ConfigScope<'a> {
    pub projects: &'a mut BTreeSet<String>,
    pub scanned_projects: &'a [String],
    pub opencode_contexts: &'a mut HashMap<String, opencode_mcp::Context>,
    pub consented: bool,
}
/// Configuration discovery needs project metadata, never transcripts or call vectors.
pub(super) struct ConfigFile {
    pub project_path: Option<String>,
    pub project_root: Option<String>,
}
impl From<&super::IndexedFile> for ConfigFile {
    fn from(file: &super::IndexedFile) -> Self {
        Self {
            project_path: file.summary.project_path.clone(),
            project_root: file.summary.project_root.clone(),
        }
    }
}
impl NativeSessions {
    pub(super) fn mcp_configuration(
        &self,
        sources: &[SourceStatus],
        files: &[ConfigFile],
        deadline: Option<std::time::Instant>,
    ) -> NativeResult<(Inventory, HashMap<String, opencode_mcp::Context>)> {
        let mut inventory = Inventory {
            directory: std::sync::Arc::new(self.directory().clone()),
            services: BTreeMap::new(),
            warnings: BTreeSet::new(),
            blocked_agents: BTreeSet::new(),
            deadline,
            versions: BTreeMap::new(),
        };
        let mut projects: BTreeSet<String> = files
            .iter()
            .flat_map(|f| [f.project_path.clone(), f.project_root.clone()])
            .flatten()
            .filter(|p| valid_project(p))
            .collect();
        let mut contexts = HashMap::new();
        self.collect_mcp_configuration(
            ConfigPhase::Global,
            sources,
            &mut ConfigScope {
                projects: &mut projects,
                scanned_projects: &[],
                opencode_contexts: &mut contexts,
                consented: false,
            },
            &mut inventory,
        )?;
        if projects.len() > 500 {
            inventory
                .blocked_agents
                .extend(self.directory().sources().iter().map(|s| s.id.clone()));
            inventory
                .warnings
                .insert("已知项目位置超过 500 项，本次配置扫描不完整。".into());
        }
        let scanned: Vec<_> = projects.iter().take(500).cloned().collect();
        self.collect_mcp_configuration(
            ConfigPhase::Projects,
            sources,
            &mut ConfigScope {
                projects: &mut projects,
                scanned_projects: &scanned,
                opencode_contexts: &mut contexts,
                consented: false,
            },
            &mut inventory,
        )?;
        Ok((inventory, contexts))
    }
    fn collect_mcp_configuration(
        &self,
        phase: ConfigPhase,
        sources: &[SourceStatus],
        scope: &mut ConfigScope<'_>,
        inventory: &mut Inventory,
    ) -> NativeResult<()> {
        for source in sources {
            let saved = inventory.clone();
            let saved_projects = scope.projects.clone();
            let saved_contexts = scope.opencode_contexts.clone();
            scope.consented = source.enabled;
            if self
                .with_source_context(
                    &source.agent_id,
                    Some(source.authorization_revision),
                    false,
                    |plugin, context| plugin.mcp_config(context, phase, scope, inventory),
                )
                .is_err()
            {
                *inventory = saved;
                *scope.projects = saved_projects;
                *scope.opencode_contexts = saved_contexts;
                inventory.blocked_agents.insert(source.agent_id.clone());
                inventory
                    .warnings
                    .insert(format!("{} 配置读取插件不可用。", source.agent_id));
            }
        }
        Ok(())
    }
}
