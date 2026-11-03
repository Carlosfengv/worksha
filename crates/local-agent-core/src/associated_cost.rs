//! Read-only fee attribution. Money comes only from stable gateway attempt IDs.
//! Session IDs establish scope; only unique response IDs establish response parity.
pub mod detail;
use crate::{
    observation::ModelObservation,
    projects::ProjectCatalog,
    provider_cost::{Cost, Totals},
    requests::{self, Request},
};
use serde::Serialize;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub cost: Totals,
    pub gateway_attempts: usize,
    pub response_linked_attempts: usize,
    pub session_linked_attempts: usize,
    pub native_responses: usize,
    pub matched_native_responses: usize,
    pub unlinked_native_responses: usize,
    pub conflicting_native_responses: usize,
    pub source_state: &'static str,
}
struct Attempt {
    id: String,
    session: Option<String>,
    family: String,
    product: Option<String>,
    response_linked: bool,
    cost: Cost,
    record: detail::Call,
}
pub struct Index {
    directory: crate::agents::AgentDirectory,
    attempts: Vec<Attempt>,
    native: Vec<Request>,
    response_costs: HashMap<String, Cost>,
    partial: bool,
}
impl Index {
    pub fn new(catalog: &ProjectCatalog, rows: Vec<ModelObservation>, partial: bool) -> Self {
        let mut gateway = requests::logical_requests(rows, &HashMap::new());
        let mut native = requests::sources::native_records(catalog.calls.clone(), &HashMap::new());
        requests::sources::correlate(&mut gateway, &mut native);
        let sessions: HashMap<_, _> = catalog
            .sessions
            .iter()
            .map(|s| (s.id.as_str(), s.agent_id.as_str()))
            .collect();
        let linked: HashMap<_, _> = native
            .iter()
            .filter_map(|n| {
                let record = n.native.as_ref()?;
                (record.association == "response_id")
                    .then_some((record.matched_attempt_id.as_deref()?, n))
            })
            .collect();
        // Contradictory response evidence invalidates a claimed Session, rather
        // than moving a charge between projects using whichever signal wins.
        let conflicts: HashSet<_> = native
            .iter()
            .filter(|n| {
                n.native
                    .as_ref()
                    .is_some_and(|r| r.association == "identity_conflict")
            })
            .filter_map(|n| {
                Some((
                    n.agent_id.as_str(),
                    n.native.as_ref()?.response_id.as_deref()?,
                ))
            })
            .collect();
        let mut attempts = vec![];
        let mut response_costs = HashMap::new();
        for request in gateway.drain(..) {
            for a in request.attempts {
                let r = a.observation;
                let paired = linked.get(r.id.as_str()).copied();
                let conflict = r
                    .native_response_id()
                    .is_some_and(|id| conflicts.contains(&(r.agent_id.as_str(), id)));
                let session = if conflict {
                    None
                } else {
                    r.session_id
                        .clone()
                        .or_else(|| paired.and_then(|n| n.session_id.clone()))
                }
                .filter(|id| sessions.get(id.as_str()) == Some(&r.agent_id.as_str()));
                if let Some(n) = paired
                    .filter(|n| !n.native.as_ref().unwrap().usage_conflict && session.is_some())
                {
                    response_costs.insert(n.id.clone(), a.cost.clone());
                }
                let record = detail::Call {
                    id: r.id.clone(),
                    request_id: request.id.clone(),
                    session_id: session.clone(),
                    provider_key: a.provider_key,
                    provider: r.provider.clone(),
                    model: r.routed_model.clone().or(r.model.clone()),
                    time: r.started_at.clone(),
                    status: r.status.clone(),
                    usage: r.usage.clone(),
                    cost: a.cost.clone(),
                    association: if paired.is_some() && !conflict {
                        "response_id"
                    } else {
                        "session_id"
                    },
                };
                attempts.push(Attempt {
                    record,
                    id: r.id,
                    session,
                    family: r.agent_id,
                    product: r
                        .agent_product_id
                        .or_else(|| paired.and_then(|n| n.agent_product_id.clone())),
                    response_linked: paired.is_some() && !conflict,
                    cost: a.cost,
                });
            }
        }
        Self {
            directory: catalog.calls.directory().clone(),
            attempts,
            native,
            response_costs,
            partial: partial || catalog.coverage != "native_history",
        }
    }
    pub fn response_cost(&self, id: &str) -> Cost {
        self.response_costs
            .get(id)
            .cloned()
            .unwrap_or_else(|| Cost::unknown("native_price_unconfirmed"))
    }
    pub fn summary(&self, session_ids: &[String], agent: Option<&str>) -> Summary {
        let ids: HashSet<_> = session_ids.iter().map(String::as_str).collect();
        let mut summary = Summary {
            source_state: if self.partial { "partial" } else { "ready" },
            ..Default::default()
        };
        let mut seen = HashSet::new();
        for a in &self.attempts {
            if !a.session.as_deref().is_some_and(|s| ids.contains(s))
                || !self
                    .directory
                    .matches(agent, &a.family, a.product.as_deref())
                || !seen.insert(&a.id)
            {
                continue;
            }
            summary.gateway_attempts += 1;
            summary.response_linked_attempts += usize::from(a.response_linked);
            summary.session_linked_attempts += usize::from(!a.response_linked);
            summary.cost.add(&a.cost);
        }
        for n in &self.native {
            if !n.session_id.as_deref().is_some_and(|s| ids.contains(s))
                || !self
                    .directory
                    .matches(agent, &n.agent_id, n.agent_product_id.as_deref())
            {
                continue;
            }
            summary.native_responses += 1;
            let record = n.native.as_ref().unwrap();
            if record.association == "response_id" && !record.usage_conflict {
                summary.matched_native_responses += 1;
            } else {
                summary.unlinked_native_responses += 1;
                summary.conflicting_native_responses += usize::from(
                    ["ambiguous", "identity_conflict"].contains(&record.association.as_str())
                        || record.usage_conflict,
                );
            }
        }
        summary
    }
}
