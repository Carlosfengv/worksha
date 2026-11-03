//! In-memory, bounded references to one authorized object snapshot per slot.
use super::super::{Result, fail};
use local_agent_core::{
    domain::object_summary::{ObjectSummaryRead, SummaryField},
    plugin::object_contributions as c,
};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};

type Key = (String, String, String);
fn key(slot: &c::ObjectSlotQuery) -> Key {
    (
        slot.plugin_id.clone(),
        slot.digest.clone(),
        slot.slot_id.clone(),
    )
}
const TTL: Duration = Duration::from_millis(c::OBJECT_REF_TTL_MS);

#[derive(Clone)]
pub(super) struct IssueTicket {
    slot: c::ObjectSlotQuery,
    object: local_agent_core::domain::object_summary::ObjectSelector,
    fields: Vec<SummaryField>,
    authority: String,
    nonce: String,
    started: Instant,
}
struct Latest {
    nonce: String,
    started: Instant,
}
#[derive(Clone)]
pub(super) struct Captured {
    pub reference: c::ObjectReference,
    pub read: ObjectSummaryRead,
    authority: String,
    nonce: String,
    started: Instant,
}
#[derive(Default)]
pub(super) struct Registry {
    latest: BTreeMap<Key, Latest>,
    references: BTreeMap<String, Captured>,
}
impl Registry {
    fn collect(&mut self, now: Instant) {
        self.latest
            .retain(|_, v| now.saturating_duration_since(v.started) < TTL);
        self.references.retain(|_, v| {
            now.saturating_duration_since(v.started) < TTL
                && self
                    .latest
                    .get(&key(&v.reference.slot))
                    .is_some_and(|latest| latest.nonce == v.nonce)
        });
    }
    /// Retire the previous object immediately, before any source read can wait.
    pub(super) fn begin(
        &mut self,
        request: &c::ObjectRefRequest,
        declaration: &c::ObjectSlot,
        authority: &str,
    ) -> Result<IssueTicket> {
        use super::super::Checked;
        request.validate().checked("invalid_input")?;
        if request.slot.slot_id != declaration.id || request.object.target() != declaration.target {
            return Err(fail("stale_snapshot"));
        }
        local_agent_core::domain::object_summary::validate_fields(
            declaration.target,
            &declaration.fields,
        )
        .checked("invalid_input")?;
        super::super::package::unhex::<32>(authority)?;
        if request.expected_context_identity != authority {
            return Err(fail("stale_snapshot"));
        }
        let now = Instant::now();
        self.collect(now);
        let k = key(&request.slot);
        if !self.latest.contains_key(&k) && self.latest.len() >= c::MAX_OBJECT_REFS {
            return Err(fail("resource_limit"));
        }
        let ticket = IssueTicket {
            slot: request.slot.clone(),
            object: request.object.clone(),
            fields: declaration.fields.clone(),
            authority: authority.into(),
            nonce: uuid::Uuid::new_v4().to_string(),
            started: now,
        };
        self.references.retain(|_, v| key(&v.reference.slot) != k);
        self.latest.insert(
            k,
            Latest {
                nonce: ticket.nonce.clone(),
                started: now,
            },
        );
        Ok(ticket)
    }
    pub(super) fn abandon(&mut self, ticket: &IssueTicket) {
        let k = key(&ticket.slot);
        if self.latest.get(&k).is_some_and(|v| v.nonce == ticket.nonce) {
            self.latest.remove(&k);
            self.references.retain(|_, v| v.nonce != ticket.nonce);
        }
    }
    /// Call only after independently rechecking the current grant and source.
    pub(super) fn finish(
        &mut self,
        ticket: &IssueTicket,
        read: ObjectSummaryRead,
        authority: &str,
    ) -> Result<c::ObjectReference> {
        use super::super::Checked;
        let now = Instant::now();
        self.collect(now);
        if self
            .latest
            .get(&key(&ticket.slot))
            .is_none_or(|v| v.nonce != ticket.nonce)
        {
            return Err(fail("stale_snapshot"));
        }
        if authority != ticket.authority {
            return Err(fail("grant_revoked"));
        }
        read.validate(&ticket.object, &ticket.fields)
            .checked("stale_snapshot")?;
        let elapsed = now.saturating_duration_since(ticket.started);
        let remaining = TTL.checked_sub(elapsed).ok_or(fail("stale_snapshot"))?;
        let reference = c::ObjectReference {
            slot: ticket.slot.clone(),
            context_identity: authority.into(),
            object_ref: uuid::Uuid::new_v4().to_string(),
            target: ticket.object.target(),
            expires_after_ms: u64::try_from(remaining.as_millis())
                .checked("resource_limit")?
                .into(),
        };
        self.references
            .retain(|_, v| key(&v.reference.slot) != key(&ticket.slot));
        self.references.insert(
            reference.object_ref.clone(),
            Captured {
                reference: reference.clone(),
                read,
                authority: authority.into(),
                nonce: ticket.nonce.clone(),
                started: ticket.started,
            },
        );
        Ok(reference)
    }
    /// The caller must check source authorization and call this again before delivery.
    pub(super) fn lookup(
        &mut self,
        query: &c::ObjectRefQuery,
        authority: &str,
    ) -> Result<Captured> {
        use super::super::Checked;
        query.validate().checked("invalid_input")?;
        self.collect(Instant::now());
        let captured = self
            .references
            .get(&query.object_ref)
            .ok_or(fail("stale_snapshot"))?;
        if captured.reference.slot != query.slot {
            return Err(fail("stale_snapshot"));
        }
        if captured.authority != authority {
            return Err(fail("grant_revoked"));
        }
        Ok(captured.clone())
    }
    /// A late release for an old object cannot retire a newer issue or reference.
    pub(super) fn release(&mut self, query: &c::ObjectRefQuery) -> Result<()> {
        use super::super::Checked;
        query.validate().checked("invalid_input")?;
        if let Some(captured) = self.references.get(&query.object_ref) {
            if captured.reference.slot != query.slot {
                return Err(fail("stale_snapshot"));
            }
            let k = key(&query.slot);
            if self
                .latest
                .get(&k)
                .is_some_and(|v| v.nonce == captured.nonce)
            {
                self.latest.remove(&k);
            }
        }
        self.references.remove(&query.object_ref);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use local_agent_core::domain::{Coverage, Freshness, object_summary::*};
    fn declaration() -> c::ObjectSlot {
        c::ObjectSlot {
            id: "usage".into(),
            target: ObjectTarget::Session,
            placement: local_agent_core::plugin::contributions::DetailPlacement::Card,
            view_id: "usage-view".into(),
            title: "用量".into(),
            query_kind: c::ObjectQueryKind::Summary,
            fields: vec![SummaryField::InputTokens],
            purpose: "当前会话已有用量".into(),
        }
    }
    fn request(id: &str) -> c::ObjectRefRequest {
        c::ObjectRefRequest {
            slot: c::ObjectSlotQuery {
                plugin_id: "org.example.details".into(),
                digest: "a".repeat(64),
                slot_id: "usage".into(),
            },
            expected_context_identity: "a".repeat(64),
            object: ObjectSelector::Session {
                id: id.into(),
                origin: ObjectOrigin::Builtin {},
            },
        }
    }
    fn read(request: &c::ObjectRefRequest) -> ObjectSummaryRead {
        ObjectSummaryRead {
            selector: request.object.clone(),
            read_identity: format!("snapshot-{}", request.object.id()),
            summary: ObjectSummary {
                api_version: API_VERSION.into(),
                target: ObjectTarget::Session,
                coverage: Coverage::Complete,
                freshness: Freshness::Current,
                captured_at: "2026-11-01T00:00:00Z".into(),
                metrics: vec![SummaryMetric {
                    field: SummaryField::InputTokens,
                    value: Some(SummaryValue::Tokens {
                        value: 0_u64.into(),
                    }),
                    missing_reason: None,
                }],
            },
        }
    }
    fn query(reference: &c::ObjectReference) -> c::ObjectRefQuery {
        c::ObjectRefQuery {
            slot: reference.slot.clone(),
            object_ref: reference.object_ref.clone(),
        }
    }
    #[test]
    fn switching_objects_retires_prior_reference_and_late_issue_cannot_replace_newer_object() {
        let mut r = Registry::default();
        let authority = "a".repeat(64);
        let a = request("a");
        let b = request("b");
        let ta = r.begin(&a, &declaration(), &authority).unwrap();
        let tb = r.begin(&b, &declaration(), &authority).unwrap();
        assert_eq!(
            r.finish(&ta, read(&a), &authority).unwrap_err().code,
            "stale_snapshot"
        );
        r.abandon(&ta);
        let rb = r.finish(&tb, read(&b), &authority).unwrap();
        assert_eq!(
            r.lookup(&query(&rb), &authority).unwrap().read.selector,
            b.object
        );
        let tc = r.begin(&a, &declaration(), &authority).unwrap();
        assert_eq!(
            r.lookup(&query(&rb), &authority).err().unwrap().code,
            "stale_snapshot"
        );
        let ra = r.finish(&tc, read(&a), &authority).unwrap();
        r.release(&query(&rb)).unwrap();
        assert_eq!(
            r.lookup(&query(&ra), &authority).unwrap().read.selector,
            a.object
        );
    }
    #[test]
    fn foreign_slot_authority_and_object_data_are_rejected_and_release_does_not_cross_scope() {
        let mut r = Registry::default();
        let authority = "a".repeat(64);
        let a = request("a");
        let ticket = r.begin(&a, &declaration(), &authority).unwrap();
        assert_eq!(
            r.finish(&ticket, read(&request("b")), &authority)
                .unwrap_err()
                .code,
            "stale_snapshot"
        );
        assert_eq!(
            r.finish(&ticket, read(&a), &"b".repeat(64))
                .unwrap_err()
                .code,
            "grant_revoked"
        );
        let reference = r.finish(&ticket, read(&a), &authority).unwrap();
        let mut foreign = query(&reference);
        foreign.slot.plugin_id = "org.other.details".into();
        assert!(r.lookup(&foreign, &authority).is_err());
        assert!(r.release(&foreign).is_err());
        assert_eq!(
            r.lookup(&query(&reference), &"b".repeat(64))
                .err()
                .unwrap()
                .code,
            "grant_revoked"
        );
        r.lookup(&query(&reference), &authority).unwrap();
    }
    #[test]
    fn stale_issue_context_cannot_retire_a_newer_authorized_reference() {
        let mut r = Registry::default();
        let authority = "b".repeat(64);
        let mut current = request("current");
        current.expected_context_identity = authority.clone();
        let ticket = r.begin(&current, &declaration(), &authority).unwrap();
        let reference = r.finish(&ticket, read(&current), &authority).unwrap();
        assert_eq!(
            r.begin(&request("old"), &declaration(), &authority)
                .err()
                .unwrap()
                .code,
            "stale_snapshot"
        );
        r.lookup(&query(&reference), &authority).unwrap();
    }
    #[test]
    fn capacity_is_bounded_existing_slots_survive_and_expiry_reclaims_without_a_timer() {
        let mut r = Registry::default();
        let authority = "a".repeat(64);
        let mut tickets = Vec::new();
        for i in 0..c::MAX_OBJECT_REFS {
            let mut d = declaration();
            d.id = format!("slot{i}");
            let mut q = request("a");
            q.slot.slot_id = d.id.clone();
            tickets.push((q.clone(), r.begin(&q, &d, &authority).unwrap()));
        }
        assert_eq!(
            r.begin(&request("extra"), &declaration(), &authority)
                .err()
                .unwrap()
                .code,
            "resource_limit"
        );
        let reference = r
            .finish(&tickets[0].1, read(&tickets[0].0), &authority)
            .unwrap();
        assert!(reference.expires_after_ms.value() <= c::OBJECT_REF_TTL_MS);
        r.lookup(&query(&reference), &authority).unwrap();
        let newer = r
            .finish(&tickets[0].1, read(&tickets[0].0), &authority)
            .unwrap();
        assert_eq!(r.references.len(), 1);
        assert!(r.lookup(&query(&reference), &authority).is_err());
        r.lookup(&query(&newer), &authority).unwrap();
        let expired = Instant::now() - Duration::from_millis(c::OBJECT_REF_TTL_MS + 1);
        for entry in r.latest.values_mut() {
            entry.started = expired;
        }
        assert_eq!(
            r.lookup(&query(&newer), &authority).err().unwrap().code,
            "stale_snapshot"
        );
        r.begin(&request("new"), &declaration(), &authority)
            .unwrap();
        assert_eq!(r.latest.len(), 1);
        assert!(r.references.is_empty());
    }
}
