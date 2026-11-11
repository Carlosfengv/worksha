use super::{Capabilities, ModelService, ServiceModel, current, error, version};
use crate::providers::{ConfigError, fail};
use rusqlite::Connection;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const MAX_SERVICE_LEVELS: usize = 3;
const MAX_LEAF_CANDIDATES: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ChildServiceRef {
    pub service_id: String,
    pub version_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NestedHop {
    pub service_id: String,
    pub version_id: String,
    pub service_name: String,
    pub model_id: String,
    pub model_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing_rule: Option<super::policy::MatchedRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing_intent: Option<super::policy::IntentOutcome>,
}

pub(super) fn load(db: &Connection, child: &ChildServiceRef) -> Result<ModelService, ConfigError> {
    let published = version(db, &child.version_id)?;
    if published.id != child.service_id {
        return fail("invalid_child_service", "子服务标识和发布版本不匹配");
    }
    Ok(published)
}

pub(super) fn member_source(
    db: &Connection,
    child: &ChildServiceRef,
    required: &Capabilities,
) -> Result<crate::Provider, ConfigError> {
    let published = load(db, child)?;
    if !Capabilities::common(&published.models).covers(required) {
        return fail(
            "invalid_child_service",
            "子服务默认入口的能力低于当前条目的声明",
        );
    }
    let model = published
        .models
        .iter()
        .find(|model| model.id == published.default_model_id)
        .ok_or_else(|| ConfigError::new("invalid_child_service", "子服务的默认条目不可读取"))?;
    Ok(model.provider.clone())
}

pub(super) fn validate_graph(
    db: &Connection,
    service_id: &str,
    models: &[ServiceModel],
    policy: &super::RoutingPolicy,
) -> Result<(), ConfigError> {
    if models.iter().all(|model| model.child.is_none()) {
        return Ok(());
    }
    if !policy.enabled {
        return fail("invalid_policy", "包含子服务时须启用路由策略");
    }
    for model in models.iter().filter(|model| model.child.is_some()) {
        if policy.pool(&model.id).is_some()
            || policy
                .intent_classifier
                .as_ref()
                .is_some_and(|classifier| classifier.model_id == model.id)
        {
            return fail(
                "invalid_policy",
                "请在子服务中配置账号池；分类模型须选择直接来源",
            );
        }
    }
    walk(db, models, policy, &mut vec![service_id.into()], &mut 0)
}

fn walk(
    db: &Connection,
    models: &[ServiceModel],
    policy: &super::RoutingPolicy,
    path: &mut Vec<String>,
    leaves: &mut usize,
) -> Result<(), ConfigError> {
    for model in models {
        if let Some(child) = &model.child {
            if path.contains(&child.service_id) {
                return fail("nested_service_cycle", "服务不能直接或间接引用自身");
            }
            if path.len() >= MAX_SERVICE_LEVELS {
                return fail("nested_service_depth", "最多支持三层模型服务");
            }
            current(db, &child.service_id).map_err(|error| {
                if error.code == "not_found" {
                    ConfigError::new("invalid_child_service", "子服务已删除")
                } else {
                    error
                }
            })?;
            let published = load(db, child)?;
            path.push(child.service_id.clone());
            walk(
                db,
                &published.models,
                &published.routing_policy,
                path,
                leaves,
            )?;
            path.pop();
        } else {
            *leaves += policy.accounts(&model.id).len().max(1);
            if *leaves > MAX_LEAF_CANDIDATES {
                return fail(
                    "nested_service_capacity",
                    "嵌套服务展开后最多支持 128 个模型和账号候选",
                );
            }
        }
    }
    Ok(())
}

pub(super) fn referenced(db: &Connection, service_id: &str) -> Result<bool, ConfigError> {
    db.query_row(
        "WITH RECURSIVE reachable(id) AS (
           SELECT current FROM model_services WHERE deleted=0 AND id<>?1
           UNION
           SELECT json_extract(m.value,'$.child.versionId')
           FROM reachable r JOIN model_service_versions v ON v.id=r.id,
                json_each(v.value,'$.models') m
           WHERE json_type(m.value,'$.child')='object'
         )
         SELECT EXISTS(SELECT 1 FROM reachable r
           JOIN model_service_versions v ON v.id=r.id WHERE v.service=?1)",
        [service_id],
        |row| row.get(0),
    )
    .map_err(error)
}
