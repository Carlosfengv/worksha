use crate::{ApiResult, AppState, error, json_value, native_error};
use axum::Json;
use axum::extract::{Path, Query, State};
use local_agent_core::projects::{self, ProjectQuery};
use projects::visibility::{self, HiddenProject};
use serde::Deserialize;

pub(crate) fn hidden(
    state: &AppState,
) -> Result<Vec<HiddenProject>, (axum::http::StatusCode, Json<serde_json::Value>)> {
    state
        .store
        .lock()
        .map_err(|_| error("数据服务暂不可用".into()))?
        .hidden_projects()
        .map_err(error)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct VisibilityChange {
    project_id: String,
    hidden: bool,
}

pub(crate) async fn set_hidden(
    State(state): State<AppState>,
    Json(change): Json<VisibilityChange>,
) -> ApiResult {
    crate::blocking::management(move || {
        if !visibility::valid_id(&change.project_id) {
            return Err(error("项目标识无效".into()));
        }
        let project = if change.hidden {
            Some(
                state
                    .sessions
                    .project_catalog()
                    .map_err(native_error)?
                    .sessions
                    .iter()
                    .filter_map(projects::session_identity)
                    .find(|p| p.id == change.project_id)
                    .ok_or_else(|| {
                        native_error(local_agent_core::sessions::NativeError {
                            code: "not_found",
                            message: "项目不存在或会话来源已断开".into(),
                        })
                    })?,
            )
        } else {
            None
        };
        let store = state
            .store
            .lock()
            .map_err(|_| error("数据服务暂不可用".into()))?;
        if let Some(project) = project {
            store.hide_project(&project.into()).map_err(error)?;
        } else {
            store.restore_project(&change.project_id).map_err(error)?;
        }
        json_value(serde_json::json!({"hiddenProjects":store.hidden_projects().map_err(error)?}))
    })
    .await
}

async fn blocking(
    state: AppState,
    id: Option<String>,
    query: ProjectQuery,
    file: Option<String>,
) -> ApiResult {
    crate::blocking::query(state.query_capacity.clone(), move || {
        let authorization = crate::sessions::authorizations(&state)?;
        let catalog = state
            .readonly
            .projects
            .catalog(|| state.sessions.project_catalog())
            .map_err(native_error)?;
        let hidden = hidden(&state)?;
        if let Some(id) = &id {
            visibility::ensure_visible(id, &hidden).map_err(native_error)?;
        }
        let visible = visibility::visible_catalog(catalog.clone(), &hidden);
        let listeners = state
            .listeners
            .lock()
            .map_err(|_| error("端口采集暂不可用".into()))?
            .clone();
        let value = if let Some(id) = id {
            if let Some(file) = file {
                projects::validate_with_directory(&query, state.sessions.directory())
                    .map_err(native_error)?;
                let project = projects::groups(&visible, &query, &listeners)
                    .into_iter()
                    .find(|p| p.identity.id == id)
                    .ok_or_else(|| {
                        native_error(local_agent_core::sessions::NativeError {
                            code: "not_found",
                            message: "项目不存在或会话来源已断开".into(),
                        })
                    })?;
                json_value(
                    state
                        .readonly
                        .projects
                        .diff(&project.identity, &file)
                        .map_err(native_error)?,
                )?
            } else {
                let activities =
                    state
                        .observation
                        .project_activities(&visible, &id, query.agent.as_deref());
                let detail = state
                    .readonly
                    .projects
                    .detail(&visible, &id, query, &listeners)
                    .map_err(native_error)?;
                crate::associated_costs::with_index(&state, &catalog, |costs| {
                    let mut project = json_value(&detail.project)?.0;
                    crate::associated_costs::project(
                        &mut project,
                        &detail.project,
                        costs,
                        detail.applied_filter.agent.as_deref(),
                    )
                    .map_err(error)?;
                    let mut value = json_value(detail)?;
                    value.0["project"] = project;
                    value.0["activities"] = activities;
                    Ok(value)
                })?
            }
        } else {
            let page = state
                .readonly
                .projects
                .query(&visible, query, &listeners)
                .map_err(native_error)?;
            crate::associated_costs::with_index(&state, &catalog, |costs| {
                let mut value = json_value(&page)?;
                value.0["summary"]["associatedCosts"] = costs
                    .summary(
                        &page.selected_session_ids,
                        page.applied_filter.agent.as_deref(),
                    )
                    .map_err(error)?;
                for (item, data) in value.0["items"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .zip(&page.items)
                {
                    crate::associated_costs::project(
                        item,
                        data,
                        costs,
                        page.applied_filter.agent.as_deref(),
                    )
                    .map_err(error)?;
                }
                Ok(value)
            })?
        };
        if authorization != crate::sessions::authorizations(&state)?
            || hidden != self::hidden(&state)?
        {
            return Err(native_error(local_agent_core::sessions::NativeError {
                code: "stale_snapshot",
                message: "会话来源或项目设置已更新，请刷新项目后继续".into(),
            }));
        }
        Ok(value)
    })
    .await
}
pub async fn query(State(state): State<AppState>, Query(query): Query<ProjectQuery>) -> ApiResult {
    blocking(state, None, query, None).await
}
pub async fn detail(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<ProjectQuery>,
) -> ApiResult {
    blocking(state, Some(id), query, None).await
}
pub async fn diff(
    State(state): State<AppState>,
    Path((id, file)): Path<(String, String)>,
) -> ApiResult {
    blocking(state, Some(id), ProjectQuery::default(), Some(file)).await
}
