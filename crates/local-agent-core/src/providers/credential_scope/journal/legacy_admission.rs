//! Negative admission evidence, not an ownership claim on historical credentials.
use super::*;
pub(super) struct Admission {
    allow_new: bool,
    pub(super) ids: Vec<String>,
}
pub(super) fn initialize(
    tx: &Transaction<'_>,
    path: &Path,
    store_id: &str,
    fresh: bool,
) -> Result<(), ConfigError> {
    let parent = path.parent().ok_or_else(corrupt)?;
    let absent = |name: &str| {
        fs::symlink_metadata(parent.join("provider-configs").join(name))
            .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
    };
    let allow_new =
        fresh && absent("credential-operations-v1") && absent("credential-operation-key-v1.json");
    tx.execute_batch("CREATE TABLE credential_legacy_scope(id INTEGER PRIMARY KEY CHECK(id=1),format INTEGER NOT NULL,store_id TEXT NOT NULL,allow_new INTEGER NOT NULL CHECK(allow_new IN(0,1)),operation_count INTEGER NOT NULL CHECK(operation_count BETWEEN 0 AND 4096));CREATE TABLE credential_legacy_operations(operation_id TEXT NOT NULL PRIMARY KEY);")
        .map_err(unavailable)?;
    if tx
        .execute(
            "INSERT INTO credential_legacy_scope VALUES(1,1,?1,?2,0)",
            params![store_id, allow_new],
        )
        .map_err(unavailable)?
        != 1
    {
        return Err(corrupt());
    }
    validate(tx, store_id)?;
    Ok(())
}
pub(super) fn validate(db: &Connection, store_id: &str) -> Result<Admission, ConfigError> {
    let (format, actual, allow, count): (u32, String, i64, i64) = db.query_row("SELECT format,store_id,allow_new,operation_count FROM credential_legacy_scope WHERE id=1", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).map_err(|_| corrupt())?;
    let scope_count: i64 = db
        .query_row(
            "SELECT count(*) FROM (SELECT 1 FROM credential_legacy_scope LIMIT 2)",
            [],
            |r| r.get(0),
        )
        .map_err(|_| corrupt())?;
    if scope_count != 1
        || format != 1
        || actual != store_id
        || !matches!(allow, 0 | 1)
        || !(0..=4096).contains(&count)
    {
        return Err(corrupt());
    }
    let mut query = db.prepare("SELECT substr(operation_id,1,37) FROM credential_legacy_operations ORDER BY operation_id LIMIT 4097").map_err(|_| corrupt())?;
    let ids = query
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|_| corrupt())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| corrupt())?;
    if ids.len() != count as usize || ids.iter().any(|id| !valid_id(id)) {
        return Err(corrupt());
    }
    Ok(Admission {
        allow_new: allow == 1,
        ids,
    })
}
fn unknown() -> ConfigError {
    ConfigError::new(
        "credential_result_unknown",
        "此旧编号的原恢复材料无法确认，未重新保存或标记取消；请保留编号并恢复原材料，新保存请使用当前恢复空间",
    )
}
impl Journal {
    pub(crate) fn validate_legacy_admission(&self) -> Result<(), ConfigError> {
        let mut db = self.connect()?;
        let tx = db.transaction().map_err(unavailable)?;
        let meta = self.validate(&tx)?;
        validate(&tx, &meta.store_id)?;
        tx.commit().map_err(unavailable)?;
        self.check_file()
    }
    /// The caller holds the root recovery lock and then the Provider file lock.
    pub(crate) fn admit_legacy_operation(
        &self,
        id: &str,
        material_exists: bool,
        create_or_cancel: bool,
    ) -> Result<(), ConfigError> {
        if !valid_id(id) {
            return Err(corrupt());
        }
        let mut db = self.connect()?;
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(unavailable)?;
        let meta = self.validate(&tx)?;
        let admission = validate(&tx, &meta.store_id)?;
        let known = admission.ids.iter().any(|saved| saved == id);
        if !material_exists && (known || !admission.allow_new) {
            return Err(unknown());
        }
        if admission.allow_new && !known && (material_exists || create_or_cancel) {
            if admission.ids.len() == 4096 {
                return fail(
                    "credential_operation_capacity",
                    "旧凭据准入记录已达上限；原记录与凭据已保留，未分配新的引用",
                );
            }
            if meta.revision == MAX_REVISION {
                return Err(corrupt());
            }
            let inserted = tx
                .execute("INSERT INTO credential_legacy_operations VALUES(?1)", [id])
                .map_err(unavailable)?;
            let updated = tx.execute("UPDATE credential_legacy_scope SET operation_count=operation_count+1 WHERE id=1 AND operation_count=?1", [admission.ids.len() as i64]).map_err(unavailable)?;
            let advanced = tx.execute("UPDATE credential_recovery_scope SET revision=revision+1 WHERE id=1 AND revision=?1", [meta.revision]).map_err(unavailable)?;
            if inserted != 1 || updated != 1 || advanced != 1 {
                return Err(corrupt());
            }
            let after = validate(&tx, &meta.store_id)?;
            let next = self.validate(&tx)?;
            if after.ids.len() != admission.ids.len() + 1
                || !after.ids.iter().any(|saved| saved == id)
                || next.revision != meta.revision + 1
            {
                return Err(corrupt());
            }
        }
        tx.commit().map_err(unavailable)?;
        self.check_file()
    }
}
