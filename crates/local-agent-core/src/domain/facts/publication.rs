//! Tentative facts become visible only after a complete, bounded, linked generation.
use super::*;
use std::collections::{HashMap, HashSet};

pub const MAX_FACTS: usize = 4096;
pub const MAX_FACT_BYTES: usize = 10 * 1024 * 1024;
pub const BATCH_FACTS: usize = 256;

fn text(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && !s.chars().any(char::is_control)
}
fn time(s: Option<&str>) -> bool {
    s.is_none_or(|s| s.len() <= 40 && chrono::DateTime::parse_from_rfc3339(s).is_ok())
}
fn usage(u: &Usage) -> bool {
    !matches!((&u.cached_input_tokens, &u.input_tokens), (Some(c), Some(i)) if c.value() > i.value())
        && !matches!((&u.reasoning_output_tokens, &u.output_tokens), (Some(r), Some(o)) if r.value() > o.value())
}
/// Structured identity prevents collisions between native IDs containing separators.
/// Digest changes execution identity without renaming the same source business object.
pub fn object_id(source: &str, kind: &str, native_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(&(source, kind, native_id)).expect("string tuple");
    format!("domain:{:x}", Sha256::digest(bytes))
}

pub struct FactPublication {
    identity: FactIdentity,
    kinds: Vec<FactKind>,
    products: HashSet<String>,
    revision: Option<String>,
    batches: u64,
    bytes: usize,
    records: Vec<FactCandidate>,
    keys: HashSet<(String, String)>,
    failed: bool,
}
impl FactPublication {
    /// All identity fields and permitted products come from the host's pinned lease.
    pub fn new(
        identity: FactIdentity,
        products: Vec<String>,
        kinds: Vec<FactKind>,
    ) -> Result<Self> {
        if !text(&identity.source_id, 240)
            || !text(&identity.provider_key, 512)
            || !text(&identity.implementation_identity, 256)
            || !time(Some(&identity.observed_at))
            || products.is_empty()
            || products.len() > 16
            || products.iter().any(|p| !text(p, 240))
            || products.iter().collect::<HashSet<_>>().len() != products.len()
            || kinds.is_empty()
            || kinds.len() > FactKind::ALL.len()
            || kinds.iter().collect::<HashSet<_>>().len() != kinds.len()
        {
            return Err(Failure::new(ErrorCode::InvalidInput));
        }
        Ok(Self {
            identity,
            kinds,
            products: products.into_iter().collect(),
            revision: None,
            batches: 0,
            bytes: 0,
            records: vec![],
            keys: HashSet::new(),
            failed: false,
        })
    }
    pub fn accept(&mut self, batch: FactBatch) -> Result<()> {
        let result = self.accept_inner(batch);
        if result.is_err() {
            self.failed = true;
        }
        result
    }
    fn accept_inner(&mut self, batch: FactBatch) -> Result<()> {
        if self.failed
            || batch.batch_id.value() != self.batches
            || !text(&batch.source_revision, 256)
            || self
                .revision
                .as_ref()
                .is_some_and(|r| r != &batch.source_revision)
            || batch.records.is_empty()
            || batch.records.len() > BATCH_FACTS
        {
            return Err(Failure::new(ErrorCode::InvalidInput));
        }
        let bytes = serde_json::to_vec(&batch)
            .map_err(|_| Failure::new(ErrorCode::InvalidInput))?
            .len();
        if self.bytes.saturating_add(bytes) > MAX_FACT_BYTES
            || self.records.len().saturating_add(batch.records.len()) > MAX_FACTS
        {
            return Err(Failure::new(ErrorCode::ResourceLimit));
        }
        for record in &batch.records {
            if !self.validate(record)
                || !self
                    .keys
                    .insert((record.kind().into(), record.native_id().into()))
            {
                return Err(Failure::new(ErrorCode::InvalidInput));
            }
        }
        self.bytes += bytes;
        self.revision = Some(batch.source_revision);
        self.batches += 1;
        self.records.extend(batch.records);
        Ok(())
    }
    fn validate(&self, record: &FactCandidate) -> bool {
        if !self.kinds.contains(&record.fact_kind())
            || !text(record.native_id(), 256)
            || !time(record.occurred_at())
        {
            return false;
        }
        let reference = |r: &str| text(r, 256);
        let optional = |r: &Option<String>| r.as_deref().is_none_or(reference);
        match record {
            FactCandidate::Project(v) => {
                text(&v.name, 200) && v.path_hint.as_ref().is_none_or(|p| text(p, 2048))
            }
            FactCandidate::Session(v) => {
                text(&v.title, 500)
                    && (!v.product_ids.is_empty() || v.unknown_client)
                    && v.product_ids.len() <= 16
                    && v.product_ids.iter().all(|p| self.products.contains(p))
                    && v.product_ids.iter().collect::<HashSet<_>>().len() == v.product_ids.len()
                    && time(v.created_at.as_deref())
                    && time(v.updated_at.as_deref())
                    && optional(&v.project_ref)
                    && usage(&v.usage)
            }
            FactCandidate::Response(v) => {
                reference(&v.session_ref)
                    && v.product_id
                        .as_ref()
                        .is_none_or(|p| self.products.contains(p))
                    && v.model.as_ref().is_none_or(|m| text(m, 200))
                    && optional(&v.reported_response_id)
                    && usage(&v.usage)
            }
            FactCandidate::Activity(v) => reference(&v.session_ref) && optional(&v.response_ref),
            FactCandidate::Write(v) => {
                reference(&v.session_ref)
                    && reference(&v.project_ref)
                    && optional(&v.response_ref)
                    && text(&v.relative_path, 1024)
                    && !v.relative_path.starts_with('/')
                    && !v.relative_path.contains(['\\', ':'])
                    && v.relative_path
                        .split('/')
                        .all(|c| !["", ".", ".."].contains(&c))
            }
            FactCandidate::McpCatalog(v) => {
                text(&v.server_name, 200)
                    && v.tool_names.as_ref().is_none_or(|names| {
                        names.len() <= 128
                            && names.iter().all(|t| text(t, 200))
                            && names.iter().collect::<HashSet<_>>().len() == names.len()
                    })
            }
            FactCandidate::McpExecution(v) => {
                optional(&v.server_ref)
                    && optional(&v.session_ref)
                    && optional(&v.response_ref)
                    && (v.response_ref.is_none() || v.session_ref.is_some())
                    && text(&v.tool_name, 200)
            }
        }
    }
    pub fn finish(self, checkpoint: &Checkpoint) -> Result<FactSnapshot> {
        if checkpoint.coverage == Coverage::Unavailable {
            return Err(Failure::new(ErrorCode::Unavailable));
        }
        if self.failed
            || !checkpoint.complete
            || !text(&checkpoint.source_revision, 256)
            || self
                .revision
                .as_ref()
                .is_some_and(|r| r != &checkpoint.source_revision)
            || checkpoint.batches.value() != self.batches
            || checkpoint.records.value() != self.records.len() as u64
        {
            return Err(Failure::new(ErrorCode::StaleSnapshot));
        }
        let sessions: HashMap<_, _> = self
            .records
            .iter()
            .filter_map(|r| match r {
                FactCandidate::Session(s) => Some((s.native_id.as_str(), s)),
                _ => None,
            })
            .collect();
        let responses: HashMap<_, _> = self
            .records
            .iter()
            .filter_map(|r| match r {
                FactCandidate::Response(v) => Some((v.native_id.as_str(), v.session_ref.as_str())),
                _ => None,
            })
            .collect();
        let has = |kind: &str, id: &str| self.keys.contains(&(kind.into(), id.into()));
        let linked = |session: &str, response: &Option<String>| {
            sessions.contains_key(session)
                && response
                    .as_deref()
                    .is_none_or(|id| responses.get(id) == Some(&session))
        };
        for r in &self.records {
            let valid = match r {
                FactCandidate::Session(v) => {
                    v.project_ref.as_deref().is_none_or(|p| has("project", p))
                }
                FactCandidate::Response(v) => {
                    sessions.get(v.session_ref.as_str()).is_some_and(|s| {
                        v.product_id
                            .as_ref()
                            .is_none_or(|p| s.product_ids.contains(p))
                    })
                }
                FactCandidate::Activity(v) => linked(&v.session_ref, &v.response_ref),
                FactCandidate::Write(v) => {
                    linked(&v.session_ref, &v.response_ref)
                        && has("project", &v.project_ref)
                        && sessions
                            .get(v.session_ref.as_str())
                            .is_some_and(|s| s.project_ref.as_ref() == Some(&v.project_ref))
                }
                FactCandidate::McpExecution(v) => {
                    v.server_ref
                        .as_deref()
                        .is_none_or(|s| has("mcp-catalog", s))
                        && v.session_ref
                            .as_deref()
                            .is_none_or(|s| linked(s, &v.response_ref))
                }
                _ => true,
            };
            if !valid {
                return Err(Failure::new(ErrorCode::InvalidInput));
            }
        }
        let source_revision = checkpoint.source_revision.clone();
        let records = self
            .records
            .into_iter()
            .map(|mut value| {
                let id = object_id(&self.identity.source_id, value.kind(), value.native_id());
                let map = |kind: &str, id: &str| object_id(&self.identity.source_id, kind, id);
                match &mut value {
                    FactCandidate::Session(v) => {
                        v.project_ref = v.project_ref.as_ref().map(|r| map("project", r));
                    }
                    FactCandidate::Response(v) => {
                        v.session_ref = map("session", &v.session_ref);
                    }
                    FactCandidate::Activity(v) => {
                        v.session_ref = map("session", &v.session_ref);
                        v.response_ref = v.response_ref.as_ref().map(|r| map("response", r));
                    }
                    FactCandidate::Write(v) => {
                        v.session_ref = map("session", &v.session_ref);
                        v.project_ref = map("project", &v.project_ref);
                        v.response_ref = v.response_ref.as_ref().map(|r| map("response", r));
                    }
                    FactCandidate::McpExecution(v) => {
                        v.session_ref = v.session_ref.as_ref().map(|r| map("session", r));
                        v.server_ref = v.server_ref.as_ref().map(|r| map("mcp-catalog", r));
                        v.response_ref = v.response_ref.as_ref().map(|r| map("response", r));
                    }
                    _ => {}
                }
                FactRecord {
                    id,
                    identity: self.identity.clone(),
                    source_revision: source_revision.clone(),
                    value,
                }
            })
            .collect();
        Ok(FactSnapshot {
            identity: self.identity,
            available_kinds: self.kinds,
            source_revision,
            coverage: checkpoint.coverage.clone(),
            records,
        })
    }
}

#[cfg(test)]
mod tests;
