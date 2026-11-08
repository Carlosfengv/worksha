//! Legacy common execution bridge. It consumes only complete, host-validated snapshots.
use super::{Cached, Checked, Manager, Result, domain_store::Consent, fail};
use local_agent_core::{domain as d, plugin::external as common};
use serde::Serialize;

#[derive(Clone)]
pub(crate) struct SourceView {
    pub plugin_id: String,
    pub digest: String,
    pub source: d::Source,
    pub product: d::Product,
    pub provider: d::Provider,
    pub cache: Option<std::sync::Arc<Cached>>,
    pub consent_identity: Option<String>,
    pub persist_metadata: bool,
    pub problem: Option<String>,
}
impl SourceView {
    pub fn vector(&self) -> String {
        // Opaque host identities, not file paths or guest-provided revision authority.
        serde_json::json!([
            self.digest,
            self.source.status,
            self.consent_identity,
            self.cache.as_ref().map(|c| &c.snapshot),
            self.problem
        ])
        .to_string()
    }
    pub fn state(&self) -> d::SourceState {
        let authorized = self.source.status.authorization == d::Authorization::Authorized;
        d::SourceState {
            source_id: self.source.id.clone(),
            authorization: self.source.status.authorization.clone(),
            freshness: self.source.status.freshness.clone(),
            coverage: if !authorized || self.cache.is_none() {
                d::Coverage::Unavailable
            } else if self.source.status.freshness != d::Freshness::Current
                || self.problem.is_some()
            {
                d::Coverage::Partial
            } else {
                d::Coverage::Complete
            },
            problem: self.problem.clone().or_else(|| {
                if !authorized {
                    Some("permission_denied".into())
                } else if self.cache.is_none() {
                    Some("needs_refresh".into())
                } else {
                    None
                }
            }),
        }
    }
    pub fn sessions(
        &self,
        filter: &d::SessionFilter,
        after: Option<d::SessionOrder>,
        cutoff: Option<i64>,
    ) -> Result<(Vec<d::SessionItem>, d::SessionScopeSummary)> {
        let mut window = d::SessionWindow::new(&self.source.id, after);
        if self.source.status.authorization != d::Authorization::Authorized {
            return Ok(window.finish());
        }
        if let Some(cache) = &self.cache {
            let mut publication = d::source::ScanPublication::with_limit(&self.source.id, 10240);
            let mut count = 0u64;
            for (n, batch) in cache.sessions.chunks(d::source::BATCH_ITEMS).enumerate() {
                let records: Vec<_> = batch
                    .iter()
                    .map(|s| legacy_record(&self.plugin_id, &self.product.id, s))
                    .collect::<Result<_>>()?;
                count += records.len() as u64;
                publication
                    .accept(d::SourceBatch {
                        source_id: self.source.id.clone(),
                        source_revision: cache.snapshot.clone(),
                        batch_id: (n as u64).into(),
                        records: records.clone(),
                    })
                    .map_err(|_| fail("invalid_record"))?;
                for record in records {
                    if filter.matches(&record, cutoff) {
                        window.observe(record).map_err(|_| fail("resource_limit"))?;
                    }
                }
            }
            publication
                .finish(&d::Checkpoint {
                    source_revision: cache.snapshot.clone(),
                    batches: (cache.sessions.len().div_ceil(d::source::BATCH_ITEMS) as u64).into(),
                    records: count.into(),
                    complete: true,
                    coverage: d::Coverage::Complete,
                })
                .map_err(|_| fail("invalid_record"))?;
        }
        Ok(window.finish())
    }
}
pub(crate) fn legacy_record(
    plugin: &str,
    product: &str,
    s: &common::SessionCandidate,
) -> Result<d::SessionRecord> {
    super::broker::validate_session(s).map_err(|_| fail("invalid_record"))?;
    let input = s
        .usage
        .as_ref()
        .map(|u| super::broker::decimal_u64(&u.input_tokens))
        .transpose()
        .map_err(|_| fail("invalid_record"))?;
    let output = s
        .usage
        .as_ref()
        .map(|u| super::broker::decimal_u64(&u.output_tokens))
        .transpose()
        .map_err(|_| fail("invalid_record"))?;
    let total = input
        .zip(output)
        .map(|(i, o)| i.checked_add(o).ok_or(fail("resource_limit")))
        .transpose()?;
    Ok(d::SessionRecord {
        id: format!("{plugin}:{}", s.native_id),
        native_id: s.native_id.clone(),
        product_ids: vec![product.into()],
        unknown_client: true,
        title: s.title.clone(),
        created_at: s.created_at.clone(),
        updated_at: s.created_at.clone(),
        project_ref: None,
        usage: d::Usage {
            input_tokens: input.map(Into::into),
            output_tokens: output.map(Into::into),
            total_tokens: total.map(Into::into),
            cached_input_tokens: s
                .usage
                .as_ref()
                .and_then(|u| u.cache_read_tokens.as_ref())
                .map(|s| super::broker::decimal_u64(s))
                .transpose()?
                .map(Into::into),
            cache_write_input_tokens: None,
            reasoning_output_tokens: s
                .usage
                .as_ref()
                .and_then(|u| u.reasoning_tokens.as_ref())
                .map(|s| super::broker::decimal_u64(s))
                .transpose()?
                .map(Into::into),
        },
        partial: false,
    })
}
impl Manager {
    pub(crate) fn domain_grant_source(
        &self,
        q: &d::SourceConsentQuery,
        path: &std::path::Path,
    ) -> Result<()> {
        if self.full_query(&q.plugin_id, &q.digest)? {
            self.full_grant_source(
                &local_agent_core::plugin::full::PackageQuery {
                    plugin_id: q.plugin_id.clone(),
                    digest: q.digest.clone(),
                },
                path,
            )?;
        } else {
            self.grant_source(&q.plugin_id, &q.digest, path)?;
        }
        Ok(())
    }
    pub(crate) async fn domain_refresh_source(
        self: &std::sync::Arc<Self>,
        q: d::SourceConsentQuery,
    ) -> Result<()> {
        let manager = self.clone();
        let query = q.clone();
        let full =
            super::run_sync(move || manager.full_query(&query.plugin_id, &query.digest)).await?;
        if full {
            self.full_refresh(local_agent_core::plugin::full::PackageQuery {
                plugin_id: q.plugin_id,
                digest: q.digest,
            })
            .await?;
        } else {
            self.refresh_external(&q.plugin_id, &q.digest, None).await?;
        }
        Ok(())
    }
    fn domain_restore_projection(&self, id: &str, saved: Cached) -> Result<std::sync::Arc<Cached>> {
        // A manual refresh can publish while the saved projection is being read.
        // Restoration may fill an empty cache, never replace a published generation.
        Ok(self
            .cache
            .lock()
            .checked("unavailable")?
            .data
            .entry(id.into())
            .or_insert_with(|| std::sync::Arc::new(saved))
            .clone())
    }
    pub(crate) fn domain_freeze(
        &self,
        query: &d::UnifiedSessionQuery,
        signature: &str,
    ) -> Result<(
        String,
        super::domain_store::FrozenQuery,
        Option<d::SessionOrder>,
    )> {
        let mut host = self.domain.lock().checked("unavailable")?;
        host.prune_queries();
        if let Some(token) = &query.snapshot_token {
            let frozen = host
                .queries
                .get(token)
                .filter(|q| q.created.elapsed() < std::time::Duration::from_secs(300))
                .ok_or(fail("stale_snapshot"))?;
            if frozen.signature != signature
                || frozen.query.scope != query.scope
                || frozen.query.filter != query.filter
            {
                return Err(fail("stale_snapshot"));
            }
            let after = if let Some(cursor) = &query.cursor {
                if cursor.len() > 4096 {
                    return Err(fail("stale_snapshot"));
                }
                let (prefix, data) = cursor.split_once('.').ok_or(fail("stale_snapshot"))?;
                if prefix != token || data.len() % 2 != 0 {
                    return Err(fail("stale_snapshot"));
                }
                let bytes = (0..data.len())
                    .step_by(2)
                    .map(|i| {
                        u8::from_str_radix(data.get(i..i + 2).ok_or(fail("stale_snapshot"))?, 16)
                            .checked("stale_snapshot")
                    })
                    .collect::<Result<Vec<_>>>()?;
                Some(serde_json::from_slice(&bytes).checked("stale_snapshot")?)
            } else {
                None
            };
            return Ok((token.clone(), frozen.clone(), after));
        }
        if query.cursor.is_some() {
            return Err(fail("stale_snapshot"));
        }
        let now = chrono::Utc::now();
        let cutoff = query.filter.days.map(|days| {
            if days == 1 {
                now.with_timezone(&chrono::FixedOffset::east_opt(8 * 3600).expect("fixed offset"))
                    .date_naive()
                    .and_hms_opt(0, 0, 0)
                    .expect("midnight")
                    .and_local_timezone(
                        chrono::FixedOffset::east_opt(8 * 3600).expect("fixed offset"),
                    )
                    .unwrap()
                    .timestamp_millis()
            } else {
                now.timestamp_millis() - i64::from(days) * 86_400_000
            }
        });
        let frozen = super::domain_store::FrozenQuery {
            query: query.clone(),
            signature: signature.into(),
            as_of: now.to_rfc3339(),
            cutoff,
            created: std::time::Instant::now(),
        };
        let token = uuid::Uuid::new_v4().to_string();
        host.queries.insert(token.clone(), frozen.clone());
        Ok((token, frozen, None))
    }
    pub(crate) fn domain_frozen(&self, token: &str) -> Result<super::domain_store::FrozenQuery> {
        let host = self.domain.lock().checked("unavailable")?;
        host.queries
            .get(token)
            .filter(|q| q.created.elapsed() < std::time::Duration::from_secs(300))
            .cloned()
            .ok_or(fail("stale_snapshot"))
    }
    pub(crate) fn domain_check_frozen(&self, token: &str, signature: &str) -> Result<()> {
        if self.domain_frozen(token)?.signature != signature {
            return Err(fail("stale_snapshot"));
        }
        Ok(())
    }
    pub(crate) async fn domain_read_legacy(
        self: &std::sync::Arc<Self>,
        view: &SourceView,
        request: &d::UnifiedConversationRequest,
    ) -> Result<d::ConversationPage> {
        let cache = view.cache.as_ref().ok_or(fail("stale_snapshot"))?;
        if request.anchor.is_some() {
            return Err(fail("unsupported"));
        }
        let identity = super::package::hash(request.session_id.as_bytes());
        let prefix = format!("{}.{}.", request.snapshot_token, identity);
        let offset = if let Some(cursor) = &request.cursor {
            let s = cursor.strip_prefix(&prefix).ok_or(fail("stale_snapshot"))?;
            super::broker::decimal_u64(s).map_err(|_| fail("stale_snapshot"))? as usize
        } else {
            0
        };
        let mut observed = self.observe(
            &view.plugin_id,
            &view.digest,
            super::diagnostics::Operation::ReadSession,
        );
        let result = self
            .read_session(
                &view.plugin_id,
                &view.digest,
                &request.session_id,
                &cache.snapshot,
            )
            .await;
        observed.finish(&result);
        let body: common::Conversation =
            serde_json::from_value(result?).checked("invalid_record")?;
        if offset > body.messages.len() || offset % d::PAGE_SIZE != 0 {
            return Err(fail("stale_snapshot"));
        }
        let events = body
            .messages
            .iter()
            .enumerate()
            .skip(offset)
            .take(d::PAGE_SIZE)
            .map(|(n, m)| d::ConversationEvent {
                id: format!("{}:message:{n}", request.session_id),
                native_id: n.to_string(),
                turn_id: n.to_string(),
                kind: "message".into(),
                role: Some(m.role.clone()),
                status: "recorded".into(),
                text: Some(m.text.clone()),
                input: None,
                output: None,
                model: None,
                response_id: None,
                usage: None,
            })
            .collect();
        let end = (offset + d::PAGE_SIZE).min(body.messages.len());
        Ok(d::ConversationPage {
            session_id: request.session_id.clone(),
            events,
            next_cursor: (end < body.messages.len()).then(|| format!("{prefix}{end}")),
            complete: end == body.messages.len(),
        })
    }
    pub(crate) fn domain_sources(&self) -> Result<Vec<SourceView>> {
        self.ensure_running()?;
        let mut sources = Vec::new();
        for item in self.list()?.into_iter().filter(|p| {
            p.manifest
                .capabilities
                .iter()
                .any(|c| c == "session.source@1")
        }) {
            let id = &item.manifest.id;
            let s = self.selection(id)?;
            if s.digest != item.digest {
                continue;
            }
            // Installation, activation and startup verify executable bytes. Metadata
            // reads use that installed description and the last complete snapshot;
            // read_session verifies the package again before any guest execution.
            let p = self.installed(id, &s.digest)?;
            let trusted = match p.signing_key.as_ref() {
                Some(key) => self.full_publisher_key(&p.manifest.publisher)?.as_ref() == Some(key),
                None => self.development,
            };
            let conflict = super::full::legacy_conflict(&self.root, id)?;
            let runtime_active = s.enabled && trusted && !conflict;
            let agent = p.manifest.agent.as_ref().ok_or(fail("invalid_record"))?;
            let consent = self.domain.lock().checked("unavailable")?.consent(id)?;
            let valid = consent.as_ref().is_some_and(|c| {
                c.enabled && c.digest == s.digest && c.epoch == s.epoch && c.revision == s.revision
            }) && runtime_active
                && s.source.is_some();
            let mut cached = self
                .cache
                .lock()
                .checked("unavailable")?
                .data
                .get(id)
                .cloned()
                .filter(|c| {
                    c.digest == s.digest
                        && c.epoch == s.epoch.to_string()
                        && c.revision == s.revision.to_string()
                });
            if valid && cached.is_none() && consent.as_ref().is_some_and(|c| c.persist) {
                let saved = self.domain.lock().checked("unavailable")?.projection(id)?;
                if let Some(saved) = saved.filter(|c| {
                    c.digest == s.digest
                        && c.epoch == s.epoch.to_string()
                        && c.revision == s.revision.to_string()
                }) {
                    // Restore only opted-in metadata. No source or guest execution occurs here.
                    cached = Some(self.domain_restore_projection(id, saved)?).filter(|c| {
                        c.digest == s.digest
                            && c.epoch == s.epoch.to_string()
                            && c.revision == s.revision.to_string()
                    });
                }
            }
            let source_id = format!("{id}:source");
            let provider_key = format!("{id}:domain.session-source@1:source");
            let freshness = if !valid {
                d::Freshness::Unread
            } else if s.error.is_some() {
                d::Freshness::Failed
            } else if let Some(c) = &cached {
                if c.expired() {
                    d::Freshness::Stale
                } else {
                    d::Freshness::Current
                }
            } else {
                d::Freshness::Unread
            };
            sources.push(SourceView {
                plugin_id: id.clone(),
                digest: s.digest.clone(),
                provider: d::Provider {
                    provider_key: provider_key.clone(),
                    plugin_id: id.clone(),
                    implementation_version: p.manifest.version.clone(),
                    implementation_identity: s.digest.clone(),
                    capability: d::Capability {
                        name: "domain.session-source".into(),
                        major: 1,
                        instance_id: source_id.clone(),
                    },
                    execution_kind: match p.manifest.runtime {
                        common::Runtime::Declarative => d::ExecutionKind::Declarative,
                        _ => d::ExecutionKind::WasmComponent,
                    },
                },
                product: d::Product {
                    id: agent.product_id.clone(),
                    display_name: agent.display_name.clone(),
                    source_id: source_id.clone(),
                    config_id: None,
                },
                source: d::Source {
                    id: source_id,
                    provider_key,
                    product_ids: vec![agent.product_id.clone()],
                    status: d::ProviderStatus {
                        enabled: s.enabled,
                        authorization: if valid {
                            d::Authorization::Authorized
                        } else if consent.is_some() {
                            d::Authorization::Revoked
                        } else {
                            d::Authorization::Required
                        },
                        grant_revision: s.revision.into(),
                        runtime_active,
                        freshness,
                    },
                },
                cache: if valid { cached } else { None },
                consent_identity: consent.as_ref().map(|c| c.identity.clone()),
                persist_metadata: consent.as_ref().is_some_and(|c| valid && c.persist),
                problem: (!trusted)
                    .then(|| "publisher_untrusted".into())
                    .or_else(|| conflict.then(|| "capability_conflict".into()))
                    .or(s.error)
                    .or(self
                        .domain
                        .lock()
                        .checked("unavailable")?
                        .persistence_errors
                        .get(id)
                        .cloned()),
            });
        }
        Ok(sources)
    }
    pub(crate) fn domain_prepare_consent(
        &self,
        input: &d::SourceConsentQuery,
    ) -> Result<d::SourceConsentReview> {
        if self.full_query(&input.plugin_id, &input.digest)? {
            return self.full_prepare_consent(input);
        }
        let s = self.selection(&input.plugin_id)?;
        if s.digest != input.digest {
            return Err(fail("grant_revoked"));
        }
        let p = self.installed(&input.plugin_id, &input.digest)?;
        if !p
            .manifest
            .capabilities
            .iter()
            .any(|c| c == "session.source@1")
        {
            return Err(fail("unsupported"));
        }
        let mut host = self.domain.lock().checked("unavailable")?;
        let consent = host.consent(&input.plugin_id)?;
        host.reviews
            .retain(|_, r| r.created.elapsed() < std::time::Duration::from_secs(300));
        if host.reviews.len() >= 64
            && let Some(id) = host
                .reviews
                .iter()
                .min_by_key(|(_, r)| r.created)
                .map(|(id, _)| id.clone())
        {
            host.reviews.remove(&id);
        }
        let review_ref = uuid::Uuid::new_v4().to_string();
        host.reviews.insert(
            review_ref.clone(),
            super::domain_store::ConsentReview {
                plugin_id: input.plugin_id.clone(),
                digest: s.digest.clone(),
                epoch: s.epoch,
                revision: s.revision,
                consent_identity: consent.as_ref().map(|c| c.identity.clone()),
                created: std::time::Instant::now(),
                applied: None,
            },
        );
        let valid = consent.as_ref().is_some_and(|c| {
            c.enabled && c.digest == s.digest && c.epoch == s.epoch && c.revision == s.revision
        }) && s.enabled
            && s.source.is_some();
        Ok(d::SourceConsentReview {
            review_ref,
            plugin_id: input.plugin_id.clone(),
            digest: s.digest,
            resource_label: s
                .source
                .as_ref()
                .map(|source| source.path.display().to_string()),
            source_authorized: s.enabled && s.source.is_some(),
            unified_authorized: valid,
            persist_metadata: valid && consent.is_some_and(|c| c.persist),
        })
    }
    pub(crate) fn domain_consent(&self, input: &d::SourceConsent) -> Result<()> {
        if self.full_query(&input.plugin_id, &input.digest)? {
            return self.full_consent(input);
        }
        let mut host = self.domain.lock().checked("unavailable")?;
        let review = host
            .reviews
            .get(&input.review_ref)
            .filter(|r| {
                r.plugin_id == input.plugin_id
                    && r.digest == input.digest
                    && r.created.elapsed() < std::time::Duration::from_secs(300)
            })
            .cloned()
            .ok_or(fail("stale_snapshot"))?;
        drop(host);
        let s = self.selection(&input.plugin_id)?;
        if s.digest != input.digest {
            return Err(fail("grant_revoked"));
        }
        if s.epoch != review.epoch || s.revision != review.revision {
            return Err(fail("grant_revoked"));
        }
        let p = if input.enabled {
            self.verified(&input.plugin_id, &input.digest)?
        } else {
            self.installed(&input.plugin_id, &input.digest)?
        };
        if !p
            .manifest
            .capabilities
            .iter()
            .any(|c| c == "session.source@1")
        {
            return Err(fail("unsupported"));
        }
        if input.enabled && (!s.enabled || s.source.is_none()) {
            return Err(fail("permission_denied"));
        }
        host = self.domain.lock().checked("unavailable")?;
        let prepared = host
            .reviews
            .get(&input.review_ref)
            .filter(|r| r.created.elapsed() < std::time::Duration::from_secs(300))
            .cloned()
            .ok_or(fail("stale_snapshot"))?;
        let current_identity = host.consent(&input.plugin_id)?.map(|c| c.identity);
        if let Some(applied) = prepared.applied {
            if current_identity.as_deref() != Some(input.review_ref.as_str()) {
                return Err(fail("stale_snapshot"));
            }
            return if applied == (input.enabled, input.persist_metadata) {
                Ok(())
            } else {
                Err(fail("invalid_input"))
            };
        }
        if current_identity != prepared.consent_identity {
            return Err(fail("stale_snapshot"));
        }
        host.grant(
            &input.plugin_id,
            &Consent {
                digest: s.digest.clone(),
                epoch: s.epoch,
                revision: s.revision,
                enabled: input.enabled,
                persist: input.enabled && input.persist_metadata,
                identity: input.review_ref.clone(),
            },
        )?;
        host.reviews
            .get_mut(&input.review_ref)
            .ok_or(fail("stale_snapshot"))?
            .applied = Some((input.enabled, input.persist_metadata));
        drop(host);
        self.domain_cache_published(&input.plugin_id)?;
        let now = self.selection(&input.plugin_id)?;
        if now.digest != s.digest || now.epoch != s.epoch || now.revision != s.revision {
            return Err(fail("grant_revoked"));
        }
        Ok(())
    }
    pub(super) fn domain_cache_published(&self, id: &str) -> Result<()> {
        let mut host = self.domain.lock().checked("unavailable")?;
        let Some(consent) = host.consent(id)? else {
            return Ok(());
        };
        if !consent.enabled || !consent.persist {
            return Ok(());
        }
        let cached = self
            .cache
            .lock()
            .checked("unavailable")?
            .data
            .get(id)
            .cloned();
        if let Some(c) = cached.filter(|c| {
            c.digest == consent.digest
                && c.epoch == consent.epoch.to_string()
                && c.revision == consent.revision.to_string()
        }) {
            if let Err(error) = host.persist(id, &c) {
                host.persistence_errors
                    .insert(id.into(), "persistence_unavailable".into());
                return Err(error);
            }
            host.persistence_errors.remove(id);
        }
        Ok(())
    }
    pub(crate) fn domain_catalog_revision(&self, value: &impl Serialize) -> Result<d::Decimal> {
        let signature = super::package::hash(&serde_json::to_vec(value).checked("invalid_record")?);
        let mut host = self.domain.lock().checked("unavailable")?;
        if host.catalog_signature != signature {
            host.catalog_revision = host
                .catalog_revision
                .checked_add(1)
                .ok_or(fail("resource_limit"))?;
            host.catalog_signature = signature;
        }
        Ok(host.catalog_revision.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    fn snapshot(name: &str, revision: &str) -> Cached {
        Cached {
            observed_at: "2026-10-31T00:00:00Z".into(),
            report: None,
            collected_at: std::time::Instant::now() - std::time::Duration::from_secs(301),
            digest: "a".repeat(64),
            epoch: "1".into(),
            revision: revision.into(),
            snapshot: name.into(),
            sessions: vec![],
            source_revision: Some(name.into()),
            quota: None,
        }
    }
    #[test]
    fn late_projection_restore_cannot_overwrite_a_refresh_or_a_replaced_grant() {
        let root = tempfile::tempdir().unwrap();
        let host = super::super::tests::new_host(root.path());
        let id = "org.example.aurora-agent";
        let restored = host
            .domain_restore_projection(id, snapshot("saved", "1"))
            .unwrap();
        assert_eq!(restored.snapshot, "saved");
        assert!(restored.expired());
        for (name, revision) in [("new-refresh", "1"), ("new-resource-grant", "2")] {
            // Deterministic interleaving: old projection was fetched first; a new
            // generation wins publication before that old restoration completes.
            let current = Arc::new(snapshot(name, revision));
            host.cache
                .lock()
                .unwrap()
                .data
                .insert(id.into(), current.clone());
            let chosen = host
                .domain_restore_projection(id, snapshot("saved", "1"))
                .unwrap();
            assert!(Arc::ptr_eq(&chosen, &current));
            assert!(Arc::ptr_eq(
                host.cache.lock().unwrap().data.get(id).unwrap(),
                &current
            ));
        }
    }
}
