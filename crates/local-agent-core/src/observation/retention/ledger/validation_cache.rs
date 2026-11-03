//! Memoize only semantic validation of the exact current projection bytes.
//! Every read still scans the current set, hashes every value and checks its
//! key, source digest and state. This is not a result or authorization cache.
use super::*;
use crate::observation::retention::AttributionCoverage;
use std::collections::HashMap;

const MAX_ENTRIES: usize = 16_384;

struct Verified {
    key: [u8; 64],
    namespace: [u8; 36],
    attributed: bool,
    visited: bool,
}

#[derive(Default)]
pub(super) struct Cache {
    entries: HashMap<[u8; 32], Verified>,
    #[cfg(test)]
    hash_checks: u64,
    #[cfg(test)]
    full_checks: u64,
}

impl Cache {
    pub fn clear(&mut self) {
        // Release the allocation as well as the entries.
        self.entries = HashMap::new();
    }

    fn check(
        &mut self,
        key: &str,
        namespace: &str,
        projection: &str,
        value: &str,
    ) -> Result<bool, LedgerError> {
        let key_bytes: [u8; 64] = key.as_bytes().try_into().map_err(invalid)?;
        let namespace_bytes: [u8; 36] = namespace.as_bytes().try_into().map_err(invalid)?;
        let actual = Sha256::digest(value.as_bytes());
        #[cfg(test)]
        {
            self.hash_checks += 1;
        }
        if format!("{actual:x}") != projection {
            return Err(LedgerError::InvalidRecord);
        }
        let fingerprint: [u8; 32] = actual.into();
        if let Some(entry) = self.entries.get_mut(&fingerprint)
            && entry.key == key_bytes
            && entry.namespace == namespace_bytes
        {
            entry.visited = true;
            return Ok(entry.attributed);
        }
        #[cfg(test)]
        {
            self.full_checks += 1;
        }
        let fact = validate(key, namespace, projection, value)?;
        let attributed = fact.attribution.is_some();
        if self.entries.len() < MAX_ENTRIES {
            self.entries.insert(
                fingerprint,
                Verified {
                    key: key_bytes,
                    namespace: namespace_bytes,
                    attributed,
                    visited: true,
                },
            );
        }
        Ok(attributed)
    }

    pub fn coverage(
        &mut self,
        db: &Connection,
        namespace: &str,
    ) -> Result<AttributionCoverage, LedgerError> {
        let result = self.scan(db, namespace);
        if result.is_ok() {
            self.entries.retain(|_, entry| entry.visited);
        }
        if result.is_err() || self.entries.is_empty() {
            self.clear();
        }
        result
    }

    fn scan(
        &mut self,
        db: &Connection,
        namespace: &str,
    ) -> Result<AttributionCoverage, LedgerError> {
        for entry in self.entries.values_mut() {
            entry.visited = false;
        }
        let mut result = AttributionCoverage::default();
        let mut statement = db.prepare("SELECT key,source_fingerprint,projection_digest,value,state FROM retention_fact_staging").map_err(unavailable)?;
        let mut rows = statement.query([]).map_err(unavailable)?;
        while let Some(row) = rows.next().map_err(unavailable)? {
            let key: String = row.get(0).map_err(invalid)?;
            let source: String = row.get(1).map_err(invalid)?;
            let projection: String = row.get(2).map_err(invalid)?;
            let value: String = row.get(3).map_err(invalid)?;
            let state: String = row.get(4).map_err(invalid)?;
            if !valid_digest(&source) || !["staged", "sealed"].contains(&state.as_str()) {
                return Err(LedgerError::InvalidRecord);
            }
            let attributed = self.check(&key, namespace, &projection, &value)?;
            if state == "staged" {
                if attributed {
                    result.complete_facts += 1;
                } else {
                    result.summary_only_facts += 1;
                }
            }
        }
        Ok(result)
    }

    #[cfg(test)]
    pub(super) fn counts(&self) -> (usize, u64, u64) {
        (self.entries.len(), self.hash_checks, self.full_checks)
    }
}

#[cfg(test)]
mod tests;
