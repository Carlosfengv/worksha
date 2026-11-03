use super::*;

/// Immutable statistical preview restored into a private, disk-backed temporary
/// SQLite database. It exposes no detail/CSV reconstruction or policy permission.
pub struct PackedPreview {
    pub(super) db: Connection,
    store_id: String,
    namespace: String,
    coverage: PackedCoverage,
}
impl PackedPreview {
    pub fn coverage(&self) -> &PackedCoverage {
        &self.coverage
    }
    fn ledger(&self, kind: PackedKind) -> Result<FactLedger<'_>, LedgerError> {
        if self.coverage.kind != kind {
            return Err(LedgerError::InvalidQuery);
        }
        Ok(FactLedger {
            db: &self.db,
            store_id: self.store_id.clone(),
            namespace: self.namespace.clone(),
            immutable_snapshot: true,
            source: None,
            validation_cache: RefCell::default(),
        })
    }
    pub fn gateway_summary(&self) -> Result<Summary, LedgerError> {
        self.ledger(PackedKind::Gateway)?.staging_summary()
    }
    pub fn gateway_summary_filtered(&self, filter: &GatewayFilter) -> Result<Summary, LedgerError> {
        self.ledger(PackedKind::Gateway)?
            .staging_summary_filtered(filter)
    }
    pub fn part_summary_filtered(
        &self,
        dimension: PartDimension,
        key: &str,
        filter: &GatewayFilter,
    ) -> Result<Summary, LedgerError> {
        self.ledger(PackedKind::Gateway)?
            .staging_part_summary_filtered(dimension, key, filter)
    }
    pub fn provider_contribution_filtered(
        &self,
        provider: Option<&str>,
        filter: &GatewayFilter,
    ) -> Result<serde_json::Value, LedgerError> {
        self.ledger(PackedKind::Gateway)?
            .staging_provider_contribution_filtered(provider, filter)
    }
    pub fn model_contribution_filtered(
        &self,
        provider: Option<&str>,
        filter: &GatewayFilter,
    ) -> Result<serde_json::Value, LedgerError> {
        self.ledger(PackedKind::Gateway)?
            .staging_model_contribution_filtered(provider, filter)
    }
    pub fn attribution_coverage(&self) -> Result<AttributionCoverage, LedgerError> {
        self.ledger(PackedKind::Gateway)?.attribution_coverage()
    }
    pub fn scope_summary(
        &self,
        session: Option<&str>,
        project: Option<&str>,
    ) -> Result<Summary, LedgerError> {
        self.ledger(PackedKind::Gateway)?
            .staging_scope_summary(session, project)
    }
    pub fn part_summary(
        &self,
        dimension: PartDimension,
        key: &str,
    ) -> Result<Summary, LedgerError> {
        self.ledger(PackedKind::Gateway)?
            .staging_part_summary(dimension, key)
    }
    pub fn provider_contribution(
        &self,
        provider: Option<&str>,
    ) -> Result<serde_json::Value, LedgerError> {
        self.ledger(PackedKind::Gateway)?
            .staging_provider_contribution(provider)
    }
    pub fn model_contribution(
        &self,
        provider: Option<&str>,
    ) -> Result<serde_json::Value, LedgerError> {
        self.ledger(PackedKind::Gateway)?
            .staging_model_contribution(provider)
    }
    pub fn tool_summary(&self, filter: &ToolFilter) -> Result<ToolSummary, LedgerError> {
        let mut result = self
            .ledger(PackedKind::ConnectorTool)?
            .tool_staging_summary(filter)?;
        result.coverage = "packed_draft";
        Ok(result)
    }
}

impl FactLedger<'_> {
    /// Verify all referenced envelopes and current staging fingerprints under one
    /// source snapshot, then restore only their whitelisted statistical JSON.
    /// This is a draft snapshot, never proof of complete original-source coverage.
    pub fn packed_preview(&self, kind: PackedKind) -> Result<PackedPreview, LedgerError> {
        let tx = self.db.unchecked_transaction().map_err(unavailable)?;
        self.check_identity()?;
        schema(&tx)?;
        let unknown: bool = tx
            .query_row(
                &format!(
                    "SELECT EXISTS(SELECT 1 FROM {} WHERE state NOT IN ('staged','sealed'))",
                    kind.table()
                ),
                [],
                |r| r.get(0),
            )
            .map_err(invalid)?;
        if unknown {
            return Err(LedgerError::InvalidRecord);
        }
        let work = Connection::open("").map_err(unavailable)?;
        {
            let _prepared = FactLedger::prepare(&work, &self.store_id, None)?;
        }
        work.execute(
            "UPDATE retention_fact_scope SET namespace=?1,identity_digest=?2 WHERE id=1",
            params![
                self.namespace,
                scope_digest(&self.store_id, &self.namespace)
            ],
        )
        .map_err(unavailable)?;
        tools::create_staging(&work)?;
        let work_tx = work.unchecked_transaction().map_err(unavailable)?;
        let mut packed = 0usize;
        let mut blocks=tx.prepare("SELECT slot FROM retention_packed_blocks WHERE EXISTS(SELECT 1 FROM retention_packed_members m WHERE m.block_slot=retention_packed_blocks.slot AND m.kind=?1) ORDER BY slot").map_err(unavailable)?;
        let mut block_rows = blocks.query([kind.tag()]).map_err(unavailable)?;
        while let Some(row) = block_rows.next().map_err(unavailable)? {
            let slot: i64 = row.get(0).map_err(invalid)?;
            let block = read_slot(&tx, slot, &self.store_id, &self.namespace, kind)?;
            let members=tx.prepare(&format!("SELECT {MEMBER_SELECT} FROM retention_packed_members WHERE block_slot=?1 ORDER BY ordinal LIMIT 65")).map_err(unavailable)?
                .query_map([slot],member).map_err(invalid)?.collect::<Result<Vec<_>,_>>().map_err(invalid)?;
            if members.is_empty() || members.len() > MAX_BATCH {
                return Err(LedgerError::InvalidRecord);
            }
            for member in members {
                let entry = block
                    .frame
                    .entries
                    .get(member.ordinal)
                    .ok_or(LedgerError::InvalidRecord)?;
                matches_member(&member, entry, kind)?;
                let current = source_entry(&tx, kind, &entry.key, MAX_RAW)?;
                if current.order != entry.order
                    || current.source != entry.source
                    || current.projection != entry.projection
                    || current.value != entry.value
                {
                    return Err(LedgerError::SourceChanged);
                }
                let inserted = match kind {
                    PackedKind::Gateway => work_tx.execute(
                        "INSERT INTO retention_fact_staging(rowid,key,source_fingerprint,projection_digest,state,value) VALUES(?1,?2,?3,?4,'staged',?5)",
                        params![entry.order,entry.key, entry.source, entry.projection, entry.value],
                    ),
                    PackedKind::ConnectorTool => work_tx.execute(
                        "INSERT INTO retention_tool_staging(rowid,key,source_locator,source_fingerprint,projection_digest,state,value) VALUES(?1,?2,?3,?4,?5,'staged',?6)",
                        params![
                            entry.order,
                            entry.key,
                            current.locator,
                            entry.source,
                            entry.projection,
                            entry.value
                        ],
                    ),
                }
                .map_err(invalid)?;
                if inserted != 1 {
                    return Err(LedgerError::InvalidRecord);
                }
                packed += 1;
            }
        }
        // Orphan references must not be silently omitted by the block iteration.
        let members: i64 = tx
            .query_row(
                "SELECT count(*) FROM retention_packed_members WHERE kind=?1",
                [kind.tag()],
                |r| r.get(0),
            )
            .map_err(invalid)?;
        if usize::try_from(members).ok() != Some(packed) {
            return Err(LedgerError::InvalidRecord);
        }
        let staged: i64 = tx
            .query_row(
                &format!("SELECT count(*) FROM {} WHERE state='staged'", kind.table()),
                [],
                |r| r.get(0),
            )
            .map_err(invalid)?;
        let staged = usize::try_from(staged).map_err(invalid)?;
        let unpacked = staged
            .checked_sub(packed)
            .ok_or(LedgerError::InvalidRecord)?;
        drop(block_rows);
        drop(blocks);
        work_tx.commit().map_err(unavailable)?;
        tx.commit().map_err(unavailable)?;
        work.execute_batch("PRAGMA query_only=ON;")
            .map_err(unavailable)?;
        Ok(PackedPreview {
            db: work,
            store_id: self.store_id.clone(),
            namespace: self.namespace.clone(),
            coverage: PackedCoverage {
                kind,
                packed_facts: packed,
                staged_facts: staged,
                unpacked_facts: unpacked,
                coverage: "packed_draft",
            },
        })
    }
}
