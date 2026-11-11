use super::{ConfigError, Verification, fail, fingerprint, select_model};
use crate::{Provider, Store};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountRef {
    pub id: String,
    pub revision: u32,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountEdit {
    pub id: String,
    pub expected_revision: u32,
    pub provider_revision: u64,
    pub label: String,
    pub credential_ref: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AccountAction {
    List,
    Save {
        store_id: String,
        account: AccountEdit,
    },
    SetState {
        store_id: String,
        account: AccountRef,
        expected_state: String,
        state: String,
    },
    Verify {
        store_id: String,
        expected_revision: u64,
        account: AccountRef,
        model: String,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub provider_id: String,
    pub label: String,
    pub revision: u32,
    pub state: String,
    #[serde(skip)]
    credential_ref: String,
    #[serde(skip)]
    endpoint: String,
    #[serde(skip)]
    protocol: String,
}

impl Account {
    /// Resolve the credential for existing quota evidence, including paused accounts.
    /// Model requests and fresh reads still require Store::upstream_account_target.
    pub fn quota_source(&self, provider: Provider) -> Option<Provider> {
        self.matches_source(&provider)
            .then(|| self.with_credential(provider))
    }

    pub fn reference(&self) -> AccountRef {
        AccountRef {
            id: self.id.clone(),
            revision: self.revision,
        }
    }

    pub fn matches_verification(&self, provider: &Provider, record: &Verification) -> bool {
        provider.configured
            && self.matches_source(provider)
            && self.state != "revoked"
            && self.id == record.provider_id
            && record.account_revision == Some(self.revision)
            && record
                .model
                .as_deref()
                .and_then(|model| select_model(provider, Some(model)).ok())
                .is_some_and(|target| {
                    fingerprint(&self.with_credential(target)) == record.fingerprint
                })
    }

    fn matches_source(&self, provider: &Provider) -> bool {
        self.provider_id == provider.id
            && self.endpoint == provider.endpoint
            && self.protocol == provider.protocol
    }

    fn with_credential(&self, mut provider: Provider) -> Provider {
        provider.credential_ref = Some(self.credential_ref.clone());
        provider.credential_label = Some(self.label.clone());
        provider
    }
}

fn unavailable(_: impl std::fmt::Display) -> ConfigError {
    ConfigError::new("service_unavailable", "上游账号存储暂不可用")
}

pub(crate) fn initialize(db: &Connection) -> Result<(), ConfigError> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS provider_upstream_accounts (
            id TEXT PRIMARY KEY, provider TEXT NOT NULL REFERENCES providers(id),
            label TEXT NOT NULL, revision INTEGER NOT NULL CHECK(revision>0),
            state TEXT NOT NULL CHECK(state IN ('enabled','paused','revoked')),
            credential TEXT NOT NULL, endpoint TEXT NOT NULL, protocol TEXT NOT NULL,
            UNIQUE(provider,credential)
        );",
    )
    .map_err(unavailable)
}

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Account> {
    Ok(Account {
        id: r.get(0)?,
        provider_id: r.get(1)?,
        label: r.get(2)?,
        revision: r.get(3)?,
        state: r.get(4)?,
        credential_ref: r.get(5)?,
        endpoint: r.get(6)?,
        protocol: r.get(7)?,
    })
}

fn read(db: &Connection, id: &str) -> Result<Option<Account>, ConfigError> {
    db.query_row(
        "SELECT id,provider,label,revision,state,credential,endpoint,protocol
         FROM provider_upstream_accounts WHERE id=?1",
        [id],
        row,
    )
    .optional()
    .map_err(unavailable)
}

impl Store {
    pub fn upstream_account(&self, id: &str) -> Result<Option<Account>, ConfigError> {
        read(&self.connection, id)
    }

    pub fn upstream_accounts(&self, provider: &str) -> Result<Vec<Account>, ConfigError> {
        self.connection
            .prepare(
                "SELECT id,provider,label,revision,state,credential,endpoint,protocol
                 FROM provider_upstream_accounts WHERE provider=?1 ORDER BY rowid",
            )
            .map_err(unavailable)?
            .query_map([provider], row)
            .map_err(unavailable)?
            .collect::<Result<_, _>>()
            .map_err(unavailable)
    }

    pub fn save_upstream_account(
        &self,
        provider: &Provider,
        input: &AccountEdit,
    ) -> Result<Account, ConfigError> {
        let label = input.label.trim();
        if !super::operations::valid_id(&input.id)
            || !super::operations::valid_id(&input.credential_ref)
            || label.is_empty()
            || label.chars().count() > 80
            || label.chars().any(char::is_control)
        {
            return fail("invalid_input", "请填写有效的账号名称与凭据");
        }
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(unavailable)?;
        let provider_is_current = self.providers().map_err(unavailable)?.iter().any(|p| {
            p.id == provider.id
                && p.configured
                && p.revision == input.provider_revision
                && p.endpoint == provider.endpoint
                && p.protocol == provider.protocol
        });
        if !provider_is_current {
            return fail("conflict", "Provider 已改变，请重新读取后保存账号");
        }
        super::validate_endpoint(&provider.endpoint)?;
        let old = read(&tx, &input.id)?;
        let revision = input
            .expected_revision
            .checked_add(1)
            .ok_or_else(|| ConfigError::new("invalid_input", "上游账号版本无效"))?;
        if let Some(old) = &old {
            if old.provider_id != provider.id || old.state == "revoked" {
                return fail("conflict", "原账号不可编辑，请重新读取");
            }
            if old.revision == revision
                && old.label == label
                && old.credential_ref == input.credential_ref
                && old.endpoint == provider.endpoint
                && old.protocol == provider.protocol
            {
                return Ok(old.clone());
            }
        }
        if old.as_ref().map_or(0, |a| a.revision) != input.expected_revision {
            return fail("conflict", "上游账号已改变，请重新读取");
        }
        let count: i64 = tx
            .query_row(
                "SELECT count(*) FROM provider_upstream_accounts WHERE provider=?1",
                [&provider.id],
                |r| r.get(0),
            )
            .map_err(unavailable)?;
        if old.is_none() && count >= 32 {
            return fail("capacity", "此 Provider 的上游账号已达上限");
        }
        let duplicate = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM provider_upstream_accounts
             WHERE provider=?1 AND credential=?2 AND id<>?3)",
                params![provider.id, input.credential_ref, input.id],
                |r| r.get::<_, bool>(0),
            )
            .map_err(unavailable)?;
        if duplicate {
            return fail("conflict", "该凭据已登记为此 Provider 的账号");
        }
        tx.execute(
            "INSERT INTO provider_upstream_accounts VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(id) DO UPDATE SET label=excluded.label,revision=excluded.revision,
             credential=excluded.credential,endpoint=excluded.endpoint,protocol=excluded.protocol",
            params![
                input.id,
                provider.id,
                label,
                revision,
                old.as_ref().map_or("enabled", |a| a.state.as_str()),
                input.credential_ref,
                provider.endpoint,
                provider.protocol
            ],
        )
        .map_err(unavailable)?;
        let saved = read(&tx, &input.id)?.ok_or_else(|| unavailable("missing account"))?;
        tx.commit().map_err(unavailable)?;
        Ok(saved)
    }

    pub fn set_upstream_account_state(
        &self,
        provider: &str,
        reference: &AccountRef,
        expected_state: &str,
        state: &str,
    ) -> Result<Account, ConfigError> {
        if !matches!(state, "enabled" | "paused" | "revoked") {
            return fail("invalid_input", "未知的上游账号状态");
        }
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(unavailable)?;
        let mut account = read(&tx, &reference.id)?
            .filter(|a| a.provider_id == provider && a.revision == reference.revision)
            .ok_or_else(|| ConfigError::new("conflict", "上游账号已改变，请重新读取"))?;
        if account.state == state {
            return Ok(account);
        }
        if account.state == "revoked" || account.state != expected_state {
            return fail("conflict", "上游账号状态已改变，请重新读取");
        }
        tx.execute(
            "UPDATE provider_upstream_accounts SET state=?2 WHERE id=?1",
            params![account.id, state],
        )
        .map_err(unavailable)?;
        tx.commit().map_err(unavailable)?;
        account.state = state.into();
        Ok(account)
    }

    /// Resolve an authorized account against the exact published origin and protocol.
    pub fn upstream_account_target(
        &self,
        provider: &Provider,
        reference: &AccountRef,
    ) -> Result<(Provider, Account), ConfigError> {
        let account = read(&self.connection, &reference.id)?
            .filter(|a| {
                a.matches_source(provider)
                    && a.revision == reference.revision
                    && a.state == "enabled"
            })
            .ok_or_else(|| ConfigError::new("account_unavailable", "上游账号已暂停、撤销或改变"))?;
        let target = account.with_credential(provider.clone());
        super::validate_source(&target)?;
        Ok((target, account))
    }
}
