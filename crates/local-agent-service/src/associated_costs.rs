use crate::AppState;
use local_agent_core::{
    associated_cost,
    projects::{ProjectCatalog, ProjectSummary},
};
use serde_json::Value;

pub(crate) struct Index<'a> {
    history: &'a local_agent_core::observation::sql::History,
    partial: bool,
}
// Prepare and consume all fee summaries under one query admission. Publishing
// another catalog or gateway batch cannot split a page's fee generations.
pub(crate) fn with_index(
    state: &AppState,
    catalog: &ProjectCatalog,
    read: impl FnOnce(&Index<'_>) -> crate::ApiResult,
) -> crate::ApiResult {
    state
        .observation
        .sql_query(|history| {
            history.prepare_fees(catalog)?;
            Ok(read(&Index {
                history,
                partial: state.observation.persistence_error()
                    || catalog.coverage != "native_history",
            }))
        })
        .map_err(crate::error)?
}
impl Index<'_> {
    pub fn summary(&self, ids: &[String], agent: Option<&str>) -> Result<Value, String> {
        self.history.fee_summary(ids, agent, self.partial)
    }
    fn details(
        &self,
        ids: &[String],
        q: &associated_cost::detail::Query,
    ) -> Result<Value, local_agent_core::sessions::NativeError> {
        self.history
            .fee_details(ids, q, self.partial)
            .map_err(|message| local_agent_core::sessions::NativeError {
                code: if message.contains("已更新") {
                    "stale_snapshot"
                } else {
                    "invalid_input"
                },
                message,
            })
    }
}
pub(crate) fn project(
    value: &mut Value,
    data: &ProjectSummary,
    index: &Index<'_>,
    agent: Option<&str>,
) -> Result<(), String> {
    value["totals"]["associatedCosts"] = index.summary(&data.selected_session_ids, agent)?;
    for (value, row) in value["agents"]
        .as_array_mut()
        .ok_or("项目费用分组无效")?
        .iter_mut()
        .zip(&data.agents)
    {
        value["totals"]["associatedCosts"] =
            index.summary(&data.selected_session_ids, Some(&row.agent_id))?;
    }
    Ok(())
}

pub(crate) async fn details(
    axum::extract::State(state): axum::extract::State<AppState>,
    axum::extract::Query(query): axum::extract::Query<associated_cost::detail::Query>,
) -> crate::ApiResult {
    crate::blocking::query(state.query_capacity.clone(), move || {
        query
            .validate_with_directory(state.sessions.directory())
            .map_err(crate::native_error)?;
        let catalog = state
            .sessions
            .project_catalog()
            .map_err(crate::native_error)?;
        let revision = catalog.revision;
        let hidden = crate::projects::hidden(&state)?;
        // Correlation sees the entire authorized catalog, retaining cross-project conflicts.

        let ids = if query.scope == "session" {
            if !catalog.sessions.iter().any(|s| s.id == query.id) {
                return Err(crate::native_error(
                    local_agent_core::sessions::NativeError {
                        code: "not_found",
                        message: "会话不存在或来源已断开".into(),
                    },
                ));
            }
            vec![query.id.clone()]
        } else {
            local_agent_core::projects::visibility::ensure_visible(&query.id, &hidden)
                .map_err(crate::native_error)?;
            let visible =
                local_agent_core::projects::visibility::visible_catalog(catalog.clone(), &hidden);
            local_agent_core::projects::groups(
                &visible,
                &local_agent_core::projects::ProjectQuery {
                    agent: query.agent.clone(),
                    days: query.days,
                    ..Default::default()
                },
                &local_agent_core::projects::listeners::ListenerSnapshot::default(),
            )
            .into_iter()
            .find(|p| p.identity.id == query.id)
            .ok_or_else(|| {
                crate::native_error(local_agent_core::sessions::NativeError {
                    code: "not_found",
                    message: "项目不存在或来源已断开".into(),
                })
            })?
            .selected_session_ids
        };
        with_index(&state, &catalog, |index| {
            let page = index.details(&ids, &query).map_err(crate::native_error)?;
            if revision
                != state
                    .sessions
                    .project_catalog()
                    .map_err(crate::native_error)?
                    .revision
                || hidden != crate::projects::hidden(&state)?
            {
                return Err(crate::native_error(
                    local_agent_core::sessions::NativeError {
                        code: "stale_snapshot",
                        message: "会话来源或项目设置已更新，请刷新明细".into(),
                    },
                ));
            }
            crate::json_value(page)
        })
    })
    .await
}
