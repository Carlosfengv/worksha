//! Application accounts and resource grants. Process and gateway credentials are separate.
use crate::{Store, plugin::PluginError};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

pub const PLUGIN_ID: &str = "builtin.iam";
pub const PROTECTED_SCHEMA_VERSION: u32 = 17;
pub const DEFAULT_TENANT: &str = "local";
const MAX_ACCOUNTS: i64 = 128;
type Result<T> = std::result::Result<T, PluginError>;

fn unavailable(_: impl std::fmt::Display) -> PluginError {
    PluginError::new("iam_unavailable", "账号管理数据暂不可用")
}
fn invalid() -> PluginError {
    PluginError::new("invalid_input", "账号名称、角色或操作参数无效")
}
fn missing() -> PluginError {
    PluginError::new("not_found", "账号或资源不存在")
}
fn conflict() -> PluginError {
    PluginError::new("stale_snapshot", "账号已变化，请重新读取")
}
fn locked() -> PluginError {
    PluginError::new("iam_locked", "账号管理承担登录保护，无法关闭或卸载")
}

pub(crate) fn initialize(db: &Connection, original_version: u32) -> Result<()> {
    let count: i64 = db.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('iam_mode','iam_accounts','iam_memberships','iam_model_grants','iam_model_creators','iam_audit')",
        [], |row| row.get(0),
    ).map_err(unavailable)?;
    if count == 0 && original_version < PROTECTED_SCHEMA_VERSION {
        db.execute_batch(
            "CREATE TABLE iam_mode (
                id INTEGER PRIMARY KEY CHECK(id=1), tenant_id TEXT NOT NULL,
                enabled INTEGER NOT NULL CHECK(enabled IN(0,1)),
                recovery_hash TEXT, recovery_owner TEXT REFERENCES iam_accounts(id),
                CHECK((enabled=1) = (recovery_hash IS NOT NULL)),
                CHECK((recovery_hash IS NULL) = (recovery_owner IS NULL)));
             CREATE TABLE iam_accounts (
                id TEXT PRIMARY KEY, username TEXT NOT NULL UNIQUE,
                display_name TEXT NOT NULL, password_hash TEXT NOT NULL,
                status TEXT NOT NULL CHECK(status IN('active','disabled','deleted')),
                revision INTEGER NOT NULL, auth_revision INTEGER NOT NULL);
             CREATE TABLE iam_memberships (
                tenant_id TEXT NOT NULL, account_id TEXT NOT NULL REFERENCES iam_accounts(id),
                role TEXT NOT NULL CHECK(role IN('owner','member')),
                PRIMARY KEY(tenant_id,account_id));
             CREATE TABLE iam_model_grants (
                tenant_id TEXT NOT NULL, account_id TEXT NOT NULL,
                service_id TEXT NOT NULL REFERENCES model_services(id),
                PRIMARY KEY(tenant_id,account_id,service_id),
                FOREIGN KEY(tenant_id,account_id) REFERENCES iam_memberships(tenant_id,account_id));
             CREATE TABLE iam_model_creators (
                service_id TEXT PRIMARY KEY REFERENCES model_services(id),
                account_id TEXT REFERENCES iam_accounts(id));
             CREATE TABLE iam_audit (
                id INTEGER PRIMARY KEY, actor TEXT REFERENCES iam_accounts(id),
                action TEXT NOT NULL, target TEXT NOT NULL,
                time TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP);
             INSERT INTO iam_mode VALUES(1,'local',0,NULL,NULL);",
        )
        .map_err(unavailable)?;
    }
    db.prepare("SELECT id,username,display_name,password_hash,status,revision,auth_revision FROM iam_accounts LIMIT 0").map_err(unavailable)?;
    db.prepare("SELECT tenant_id,account_id,role FROM iam_memberships LIMIT 0")
        .map_err(unavailable)?;
    db.prepare("SELECT tenant_id,account_id,service_id FROM iam_model_grants LIMIT 0")
        .map_err(unavailable)?;
    db.prepare("SELECT account_id,service_id FROM iam_model_creators LIMIT 0")
        .map_err(unavailable)?;
    db.prepare("SELECT actor,action,target,time FROM iam_audit LIMIT 0")
        .map_err(unavailable)?;
    if enabled(db)? != (original_version == PROTECTED_SCHEMA_VERSION) {
        return Err(unavailable("IAM schema marker"));
    }
    Ok(())
}

fn enabled(db: &Connection) -> Result<bool> {
    db.query_row(
        "SELECT enabled FROM iam_mode WHERE id=1 AND tenant_id=?1",
        [DEFAULT_TENANT],
        |r| r.get(0),
    )
    .map_err(unavailable)
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Member,
}
impl Role {
    fn name(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Member => "member",
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub role: Role,
    pub enabled: bool,
    pub revision: u32,
    #[serde(skip)]
    pub auth_revision: u32,
}

pub struct Bootstrap {
    pub username: String,
    pub display_name: String,
    pub password_hash: String,
    pub recovery_hash: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountEdit {
    pub revision: u32,
    pub display_name: String,
    pub role: Role,
    pub enabled: bool,
}

fn account(row: &rusqlite::Row<'_>) -> rusqlite::Result<Account> {
    Ok(Account {
        id: row.get(0)?,
        username: row.get(1)?,
        display_name: row.get(2)?,
        role: if row.get_ref(3)?.as_str()? == "owner" {
            Role::Owner
        } else {
            Role::Member
        },
        enabled: row.get(4)?,
        revision: row.get(5)?,
        auth_revision: row.get(6)?,
    })
}
const ACCOUNT_COLUMNS: &str =
    "a.id,a.username,a.display_name,m.role,a.status='active',a.revision,a.auth_revision";
const MEMBERS: &str = "iam_accounts a JOIN iam_memberships m ON a.id=m.account_id";

fn read_account(db: &Connection, id: &str) -> Result<Account> {
    db.query_row(
        &format!("SELECT {ACCOUNT_COLUMNS} FROM {MEMBERS} WHERE a.id=?1 AND m.tenant_id=?2 AND a.status!='deleted'"),
        params![id, DEFAULT_TENANT], account,
    ).optional().map_err(unavailable)?.ok_or_else(missing)
}

pub fn normalize_username(username: &str) -> Result<String> {
    let value = username.trim().to_ascii_lowercase();
    if !(3..=64).contains(&value.len())
        || !value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    {
        return Err(invalid());
    }
    Ok(value)
}

fn display_name(value: &str) -> Result<&str> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 64 || value.chars().any(char::is_control) {
        return Err(invalid());
    }
    Ok(value)
}

fn audit(db: &Connection, actor: Option<&str>, action: &str, target: &str) -> Result<()> {
    db.execute(
        "INSERT INTO iam_audit(actor,action,target) VALUES(?1,?2,?3)",
        params![actor, action, target],
    )
    .map_err(unavailable)?;
    Ok(())
}

fn insert_account(
    db: &Connection,
    username: &str,
    name: &str,
    hash: &str,
    role: Role,
) -> Result<String> {
    let username = normalize_username(username)?;
    let name = display_name(name)?;
    let (count, taken): (i64, bool) = db
        .query_row(
            "SELECT count(*),coalesce(max(username=?1),0) FROM iam_accounts",
            [&username],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map_err(unavailable)?;
    if taken {
        return Err(PluginError::new("username_taken", "此登录名已使用"));
    }
    if count >= MAX_ACCOUNTS {
        return Err(PluginError::new("resource_limit", "账号数量已达到上限"));
    }
    let id = uuid::Uuid::new_v4().to_string();
    db.execute(
        "INSERT INTO iam_accounts VALUES(?1,?2,?3,?4,'active',1,1)",
        params![id, username, name, hash],
    )
    .map_err(unavailable)?;
    db.execute(
        "INSERT INTO iam_memberships VALUES(?1,?2,?3)",
        params![DEFAULT_TENANT, id, role.name()],
    )
    .map_err(unavailable)?;
    Ok(id)
}

pub(crate) fn composition_change(
    db: &Connection,
    composition: &crate::plugin::builtin::Composition,
    bootstrap: Option<&Bootstrap>,
) -> Result<()> {
    use crate::plugin::builtin::Profile;
    let selected = composition
        .opted_in_plugin_ids
        .iter()
        .any(|id| id == PLUGIN_ID);
    let disabled = composition
        .disabled_plugin_ids
        .iter()
        .chain(&composition.removed_plugin_ids)
        .any(|id| id == PLUGIN_ID);
    if enabled(db)? {
        if !selected || disabled || composition.profile == Profile::Recovery || bootstrap.is_some()
        {
            return Err(locked());
        }
    } else if let Some(bootstrap) = bootstrap {
        if !selected || disabled || composition.profile == Profile::Recovery {
            return Err(invalid());
        }
        let id = insert_account(
            db,
            &bootstrap.username,
            &bootstrap.display_name,
            &bootstrap.password_hash,
            Role::Owner,
        )?;
        db.execute(
            "UPDATE iam_mode SET enabled=1,recovery_hash=?1,recovery_owner=?2 WHERE id=1",
            params![bootstrap.recovery_hash, id],
        )
        .map_err(unavailable)?;
        audit(db, Some(&id), "iam.enable", DEFAULT_TENANT)?;
        db.pragma_update(None, "user_version", PROTECTED_SCHEMA_VERSION)
            .map_err(unavailable)?;
    } else if selected {
        return Err(PluginError::new(
            "iam_setup_required",
            "请在插件中心完成账号管理首次设置",
        ));
    }
    Ok(())
}

impl Store {
    pub fn iam_enabled(&self) -> Result<bool> {
        enabled(&self.connection)
    }

    pub fn iam_account(&self, id: &str) -> Result<Account> {
        read_account(&self.connection, id)
    }

    pub fn iam_accounts(&self) -> Result<Vec<Account>> {
        let mut statement = self.connection.prepare(
            &format!("SELECT {ACCOUNT_COLUMNS} FROM {MEMBERS} WHERE m.tenant_id=?1 AND a.status!='deleted' ORDER BY a.username"),
        ).map_err(unavailable)?;
        statement
            .query_map([DEFAULT_TENANT], account)
            .map_err(unavailable)?
            .collect::<rusqlite::Result<_>>()
            .map_err(unavailable)
    }

    pub fn iam_login(&self, username: &str) -> Result<Option<(String, String, u32)>> {
        self.connection.query_row(
            &format!("SELECT a.id,a.password_hash,a.auth_revision FROM {MEMBERS} WHERE a.username=?1 AND m.tenant_id=?2 AND a.status='active'"),
            params![normalize_username(username)?, DEFAULT_TENANT],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
        ).optional().map_err(unavailable)
    }

    pub fn iam_create(
        &self,
        actor: &str,
        username: &str,
        name: &str,
        hash: &str,
        role: Role,
    ) -> Result<Account> {
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(unavailable)?;
        let id = insert_account(&tx, username, name, hash, role)?;
        audit(&tx, Some(actor), "account.create", &id)?;
        let saved = read_account(&tx, &id)?;
        tx.commit().map_err(unavailable)?;
        Ok(saved)
    }

    pub fn iam_edit(&self, actor: &str, id: &str, input: AccountEdit) -> Result<Account> {
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(unavailable)?;
        let current = read_account(&tx, id)?;
        if current.revision != input.revision {
            return Err(conflict());
        }
        let auth_changed = current.role != input.role || current.enabled != input.enabled;
        if current.enabled
            && current.role == Role::Owner
            && (input.role != Role::Owner || !input.enabled)
        {
            require_other_owner(&tx, id)?;
        }
        tx.execute(
            "UPDATE iam_accounts SET display_name=?2,status=?3,revision=revision+1,auth_revision=auth_revision+?4 WHERE id=?1",
            params![id,display_name(&input.display_name)?,if input.enabled {"active"} else {"disabled"}, auth_changed],
        ).map_err(unavailable)?;
        tx.execute(
            "UPDATE iam_memberships SET role=?3 WHERE tenant_id=?1 AND account_id=?2",
            params![DEFAULT_TENANT, id, input.role.name()],
        )
        .map_err(unavailable)?;
        audit(&tx, Some(actor), "account.edit", id)?;
        let saved = read_account(&tx, id)?;
        tx.commit().map_err(unavailable)?;
        Ok(saved)
    }

    pub fn iam_delete(&self, actor: &str, id: &str, revision: u32) -> Result<()> {
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(unavailable)?;
        let current = read_account(&tx, id)?;
        if current.revision != revision {
            return Err(conflict());
        }
        if current.enabled && current.role == Role::Owner {
            require_other_owner(&tx, id)?;
        }
        tx.execute("DELETE FROM iam_model_grants WHERE account_id=?1", [id])
            .map_err(unavailable)?;
        tx.execute("UPDATE iam_accounts SET status='deleted',password_hash='',revision=revision+1,auth_revision=auth_revision+1 WHERE id=?1",[id]).map_err(unavailable)?;
        audit(&tx, Some(actor), "account.delete", id)?;
        tx.commit().map_err(unavailable)
    }

    pub fn iam_set_password(&self, actor: &str, id: &str, revision: u32, hash: &str) -> Result<()> {
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(unavailable)?;
        if read_account(&tx, id)?.revision != revision {
            return Err(conflict());
        }
        tx.execute("UPDATE iam_accounts SET password_hash=?2,revision=revision+1,auth_revision=auth_revision+1 WHERE id=?1",params![id,hash]).map_err(unavailable)?;
        audit(&tx, Some(actor), "account.password", id)?;
        tx.commit().map_err(unavailable)
    }

    pub fn iam_recovery_hash(&self) -> Result<Option<String>> {
        self.connection
            .query_row("SELECT recovery_hash FROM iam_mode WHERE id=1", [], |r| {
                r.get(0)
            })
            .map_err(unavailable)
    }

    pub fn iam_recover(&self, prior: &str, new_hash: &str, new_recovery: &str) -> Result<()> {
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(unavailable)?;
        let id: String = tx
            .query_row(
                "SELECT recovery_owner FROM iam_mode WHERE id=1 AND recovery_hash=?1",
                [prior],
                |r| r.get(0),
            )
            .optional()
            .map_err(unavailable)?
            .ok_or_else(conflict)?;
        let owner = read_account(&tx, &id)?;
        if !owner.enabled || owner.role != Role::Owner {
            return Err(unavailable("recovery owner"));
        }
        tx.execute("UPDATE iam_accounts SET auth_revision=auth_revision+1", [])
            .map_err(unavailable)?;
        tx.execute(
            "UPDATE iam_accounts SET password_hash=?2,revision=revision+1 WHERE id=?1",
            params![id, new_hash],
        )
        .map_err(unavailable)?;
        tx.execute(
            "UPDATE iam_mode SET recovery_hash=?1 WHERE id=1",
            [new_recovery],
        )
        .map_err(unavailable)?;
        audit(&tx, Some(&id), "iam.recover", &id)?;
        tx.commit().map_err(unavailable)
    }

    pub fn iam_grants(&self, id: &str) -> Result<Vec<String>> {
        self.iam_account(id)?;
        let mut query = self
            .connection
            .prepare(
                "SELECT service_id FROM iam_model_grants WHERE tenant_id=?1 AND account_id=?2 ORDER BY service_id",
            )
            .map_err(unavailable)?;
        query
            .query_map(params![DEFAULT_TENANT, id], |r| r.get(0))
            .map_err(unavailable)?
            .collect::<rusqlite::Result<_>>()
            .map_err(unavailable)
    }

    pub fn iam_grant(&self, actor: &str, id: &str, service: &str, allow: bool) -> Result<()> {
        let tx = Transaction::new_unchecked(&self.connection, TransactionBehavior::Immediate)
            .map_err(unavailable)?;
        read_account(&tx, id)?;
        let changed = if allow {
            self.model_service(service).map_err(|_| missing())?;
            tx.execute(
                "INSERT OR IGNORE INTO iam_model_grants VALUES(?1,?2,?3)",
                params![DEFAULT_TENANT, id, service],
            )
        } else {
            tx.execute(
                "DELETE FROM iam_model_grants WHERE tenant_id=?1 AND account_id=?2 AND service_id=?3",
                params![DEFAULT_TENANT, id, service],
            )
        }
        .map_err(unavailable)?;
        if changed != 0 {
            audit(
                &tx,
                Some(actor),
                if allow {
                    "resource.grant"
                } else {
                    "resource.revoke"
                },
                &format!("{id}/{service}"),
            )?;
        }
        tx.commit().map_err(unavailable)
    }

    pub fn iam_model_creator(&self, service: &str) -> Result<String> {
        self.connection.query_row(
            "SELECT CASE WHEN c.account_id IS NULL THEN '系统' WHEN a.status='deleted' THEN '已删除账号' ELSE a.display_name END
             FROM iam_model_creators c LEFT JOIN iam_accounts a ON a.id=c.account_id WHERE c.service_id=?1",
            [service], |r| r.get(0),
        ).optional().map_err(unavailable).map(|v| v.unwrap_or_else(|| "未记录".into()))
    }
}

fn require_other_owner(db: &Connection, id: &str) -> Result<()> {
    let (other, recovery): (bool, bool) = db.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM {MEMBERS} WHERE m.tenant_id=?1 AND a.id!=?2 AND m.role='owner' AND a.status='active'),
          EXISTS(SELECT 1 FROM iam_mode WHERE id=1 AND recovery_owner=?2)"),
        params![DEFAULT_TENANT,id],|r|Ok((r.get(0)?,r.get(1)?)),
    ).map_err(unavailable)?;
    if !other {
        return Err(PluginError::new(
            "last_owner",
            "至少需要保留一个启用的 Owner",
        ));
    }
    if recovery {
        return Err(PluginError::new(
            "recovery_owner",
            "此 Owner 关联实例恢复码，需保留启用状态",
        ));
    }
    Ok(())
}

pub(crate) fn model_created(
    db: &Connection,
    id: &str,
    creator: Option<&str>,
) -> rusqlite::Result<()> {
    if db.query_row("SELECT enabled FROM iam_mode WHERE id=1", [], |r| {
        r.get::<_, bool>(0)
    })? {
        db.execute(
            "INSERT INTO iam_model_creators VALUES(?1,?2)",
            params![id, creator],
        )?;
    }
    Ok(())
}

pub(crate) fn model_deleted(db: &Connection, id: &str) -> rusqlite::Result<()> {
    db.execute("DELETE FROM iam_model_grants WHERE service_id=?1", [id])?;
    Ok(())
}
