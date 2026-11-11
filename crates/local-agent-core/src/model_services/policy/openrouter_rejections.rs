//! Admission facts confirmed by a complete official inference rejection.
use super::{ConfigError, Store, error};
use rusqlite::{Connection, params};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenRouterBudgetScope {
    KeyLimit,
    ApiKey,
    DefaultKey,
    Member,
    DefaultMember,
    Workspace,
}

impl OpenRouterBudgetScope {
    const LAYERS: [Self; 5] = [
        Self::ApiKey,
        Self::DefaultKey,
        Self::Member,
        Self::DefaultMember,
        Self::Workspace,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::KeyLimit => "key_limit",
            Self::ApiKey => "api_key",
            Self::DefaultKey => "workspace_default_key",
            Self::Member => "member",
            Self::DefaultMember => "workspace_default_member",
            Self::Workspace => "workspace",
        }
    }
}

#[derive(Clone, Debug)]
pub struct OpenRouterBudgetRejection {
    pub credential: String,
    pub scope: OpenRouterBudgetScope,
    pub observed_at: chrono::DateTime<chrono::Utc>,
    pub reset_at: Option<i64>,
}

fn update(
    db: &Connection,
    credential: &str,
    scope: OpenRouterBudgetScope,
    observed_at: chrono::DateTime<chrono::Utc>,
    exhausted: bool,
    reset_at: Option<i64>,
) -> Result<(), ConfigError> {
    db.execute(
        "INSERT INTO service_openrouter_reply_budget(credential,scope,observed_at,exhausted,reset_at)
         VALUES(?1,?2,?3,?4,?5) ON CONFLICT(credential,scope) DO UPDATE SET
         observed_at=excluded.observed_at,exhausted=excluded.exhausted,
         reset_at=CASE WHEN service_openrouter_reply_budget.exhausted=1 AND excluded.exhausted=1
                       THEN max(service_openrouter_reply_budget.reset_at,excluded.reset_at)
                       ELSE excluded.reset_at END
         WHERE service_openrouter_reply_budget.observed_at<=excluded.observed_at",
        params![credential, scope.name(), observed_at.timestamp_micros(), exhausted, reset_at],
    ).map_err(error)?;
    Ok(())
}

pub(super) fn record(
    db: &Connection,
    rejection: &OpenRouterBudgetRejection,
) -> Result<(), ConfigError> {
    update(
        db,
        &rejection.credential,
        rejection.scope,
        rejection.observed_at,
        true,
        rejection.reset_at,
    )
}

impl Store {
    pub fn record_openrouter_budget_rejection(
        &self,
        rejection: &OpenRouterBudgetRejection,
    ) -> Result<(), ConfigError> {
        record(&self.connection, rejection)
    }

    pub fn openrouter_reply_holds(
        &self,
        credential: &str,
    ) -> Result<Vec<(String, Option<i64>)>, ConfigError> {
        let mut read = self.connection.prepare_cached(
            "SELECT scope,reset_at FROM service_openrouter_reply_budget WHERE credential=?1 AND exhausted=1",
        ).map_err(error)?;
        read.query_map([credential], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(error)?
            .collect::<Result<_, _>>()
            .map_err(error)
    }

    pub fn save_openrouter_admission_refresh(
        &self,
        value: &str,
        credential: &str,
        observed_at: chrono::DateTime<chrono::Utc>,
        key_checked: bool,
        layers_checked: bool,
    ) -> Result<(), ConfigError> {
        let tx = self.connection.unchecked_transaction().map_err(error)?;
        self.save_openrouter_admission_evidence(value)?;
        if key_checked {
            update(
                &tx,
                credential,
                OpenRouterBudgetScope::KeyLimit,
                observed_at,
                false,
                None,
            )?;
        }
        if layers_checked {
            for layer in OpenRouterBudgetScope::LAYERS {
                update(&tx, credential, layer, observed_at, false, None)?;
            }
        }
        tx.commit().map_err(error)
    }
}
