use crate::{ApiResult, AppState, json_value};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier, password_hash::SaltString};
use axum::{
    Json, Router,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use local_agent_core::{
    Store,
    iam::{Account, AccountEdit, Bootstrap, DEFAULT_TENANT, PLUGIN_ID, Role},
    plugin::PluginError,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tokio::sync::{OwnedRwLockWriteGuard, RwLock, Semaphore};
use zeroize::Zeroizing;

const SESSION_HEADER: &str = "x-local-agent-session";
const SESSION_LIFETIME: Duration = Duration::from_secs(8 * 3600);
const SETUP_LIFETIME: Duration = Duration::from_secs(15 * 60);
const GATE_WAIT: Duration = Duration::from_secs(4);
type Failure = (StatusCode, Json<Value>);

#[derive(Clone)]
pub(crate) struct Actor(pub String);

struct Session {
    account: String,
    auth_revision: u32,
    until: Instant,
}

struct Challenge {
    id: String,
    code: Zeroizing<String>,
    recovery: bool,
    until: Instant,
}

pub(crate) struct Iam {
    enabled: AtomicBool,
    gate: Arc<RwLock<()>>,
    sessions: Mutex<HashMap<[u8; 32], Session>>,
    challenge: Mutex<Option<Challenge>>,
    attempts: Mutex<(Instant, u32)>,
    crypto: Arc<Semaphore>,
}
impl Default for Iam {
    fn default() -> Self {
        Self {
            enabled: AtomicBool::new(false),
            gate: Arc::new(RwLock::new(())),
            sessions: Mutex::new(HashMap::new()),
            challenge: Mutex::new(None),
            attempts: Mutex::new((Instant::now(), 0)),
            crypto: Arc::new(Semaphore::new(2)),
        }
    }
}

fn failure(status: StatusCode, code: &str, message: &str) -> Failure {
    (status, Json(json!({ "code":code, "message":message })))
}
fn unavailable() -> Failure {
    failure(
        StatusCode::SERVICE_UNAVAILABLE,
        "iam_unavailable",
        "账号管理暂不可用",
    )
}
fn busy() -> Failure {
    failure(
        StatusCode::SERVICE_UNAVAILABLE,
        "busy",
        "账号管理有操作正在进行，请稍后重试",
    )
}
fn login_required() -> Failure {
    failure(StatusCode::UNAUTHORIZED, "login_required", "请先登录账号")
}
fn denied() -> Failure {
    failure(
        StatusCode::FORBIDDEN,
        "permission_denied",
        "当前账号没有此操作权限",
    )
}
fn invalid_credentials() -> Failure {
    failure(
        StatusCode::UNAUTHORIZED,
        "invalid_credentials",
        "登录信息或恢复码不正确",
    )
}
fn plugin_error(error: PluginError) -> Failure {
    let status = match error.code {
        "not_found" => StatusCode::NOT_FOUND,
        "unavailable" | "iam_unavailable" | "busy" => StatusCode::SERVICE_UNAVAILABLE,
        "invalid_input" => StatusCode::BAD_REQUEST,
        _ => StatusCode::CONFLICT,
    };
    failure(status, error.code, &error.message)
}
fn secret() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
fn session_digest(headers: &HeaderMap) -> Option<[u8; 32]> {
    let value = headers.get(SESSION_HEADER)?.to_str().ok()?;
    (value.len() == 64).then(|| Sha256::digest(value.as_bytes()).into())
}

impl Iam {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            enabled: AtomicBool::new(enabled),
            ..Self::default()
        }
    }

    pub(crate) fn mark_enabled(&self) {
        self.enabled.store(true, Ordering::Release);
    }
    async fn write_gate(&self) -> Result<OwnedRwLockWriteGuard<()>, Failure> {
        tokio::time::timeout(GATE_WAIT, self.gate.clone().write_owned())
            .await
            .map_err(|_| busy())
    }

    fn authenticate(&self, store: &Store, digest: Option<[u8; 32]>) -> Result<Account, Failure> {
        let digest = digest.ok_or_else(login_required)?;
        let mut sessions = self.sessions.lock().map_err(|_| unavailable())?;
        let session = sessions.get(&digest).ok_or_else(login_required)?;
        let account = store.iam_account(&session.account).map_err(|error| {
            if error.code == "not_found" {
                login_required()
            } else {
                plugin_error(error)
            }
        })?;
        if session.until <= Instant::now()
            || account.auth_revision != session.auth_revision
            || !account.enabled
        {
            sessions.remove(&digest);
            return Err(login_required());
        }
        Ok(account)
    }

    fn create_session(&self, account: &Account) -> Result<Zeroizing<String>, Failure> {
        let mut sessions = self.sessions.lock().map_err(|_| unavailable())?;
        sessions
            .retain(|_, session| session.until > Instant::now() && session.account != account.id);
        if sessions.len() >= 128 {
            return Err(busy());
        }
        let token = Zeroizing::new(secret());
        sessions.insert(
            Sha256::digest(token.as_bytes()).into(),
            Session {
                account: account.id.clone(),
                auth_revision: account.auth_revision,
                until: Instant::now() + SESSION_LIFETIME,
            },
        );
        Ok(token)
    }

    fn attempt(&self) -> Result<(), Failure> {
        let mut attempts = self.attempts.lock().map_err(|_| unavailable())?;
        if attempts.0.elapsed() >= Duration::from_secs(60) {
            *attempts = (Instant::now(), 0);
        }
        if attempts.1 >= 12 {
            return Err(failure(
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "验证尝试过多，请稍后再试",
            ));
        }
        attempts.1 += 1;
        Ok(())
    }

    async fn crypto<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> Result<T, Failure> + Send + 'static,
    ) -> Result<T, Failure> {
        let permit = self
            .crypto
            .clone()
            .try_acquire_owned()
            .map_err(|_| busy())?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            work()
        })
        .await
        .map_err(|_| unavailable())?
    }

    fn prepare(&self, recovery: bool) -> ApiResult {
        let mut pending = self.challenge.lock().map_err(|_| unavailable())?;
        let next = Challenge {
            id: uuid::Uuid::new_v4().to_string(),
            code: Zeroizing::new(secret()),
            recovery,
            until: Instant::now() + SETUP_LIFETIME,
        };
        let reply = json!({"operationId":next.id,"recoveryCode":next.code.as_str()});
        *pending = Some(next);
        Ok(Json(reply))
    }

    fn code(&self, id: &str, recovery: bool) -> Result<Zeroizing<String>, Failure> {
        self.challenge
            .lock()
            .map_err(|_| unavailable())?
            .as_ref()
            .filter(|pending| {
                pending.id == id && pending.recovery == recovery && pending.until > Instant::now()
            })
            .map(|pending| pending.code.clone())
            .ok_or_else(|| {
                failure(
                    StatusCode::CONFLICT,
                    "setup_expired",
                    "设置许可已过期，请重新开始",
                )
            })
    }
}

fn password_valid(password: &str) -> Result<(), Failure> {
    if password.chars().count() < 12 || password.len() > 256 {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "invalid_password",
            "密码至少需要 12 个字符，最多 256 字节",
        ));
    }
    Ok(())
}
fn hash(password: &str) -> Result<String, Failure> {
    let salt =
        SaltString::encode_b64(uuid::Uuid::new_v4().as_bytes()).map_err(|_| unavailable())?;
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| unavailable())
}
fn verify(password: &str, hash: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|parsed| {
        Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    })
}
async fn with_store<T: Send + 'static>(
    state: &AppState,
    work: impl FnOnce(&Store) -> Result<T, Failure> + Send + 'static,
) -> Result<T, Failure> {
    let store = state.store.clone();
    crate::blocking::run(move || {
        let store = store.lock().map_err(|_| unavailable())?;
        work(&store)
    })
    .await
    .map_err(|error| match error {
        crate::blocking::Error::Busy => busy(),
        crate::blocking::Error::Unavailable => unavailable(),
    })?
}
fn enabled(store: &Store) -> Result<bool, Failure> {
    store.iam_enabled().map_err(plugin_error)
}
fn ready(state: &AppState) -> bool {
    crate::plugin_host::features::active(state, PLUGIN_ID)
}
fn require_plugin(state: &AppState) -> Result<(), Failure> {
    if ready(state) {
        Ok(())
    } else {
        Err(unavailable())
    }
}
async fn admit_owner(
    state: &AppState,
    digest: Option<[u8; 32]>,
) -> Result<Option<Account>, Failure> {
    let iam = state.iam.clone();
    if !iam.enabled.load(Ordering::Acquire) {
        return Ok(None);
    }
    let account = with_store(state, move |store| {
        let account = iam.authenticate(store, digest)?;
        if account.role != Role::Owner {
            return Err(denied());
        }
        Ok(Some(account))
    })
    .await?;
    // In recovery the verified owner may reach the existing plugin recovery
    // controls. The ordinary plugin guards still close affected business I/O.
    Ok(account)
}

fn hold_response<P: Send + Unpin + 'static>(reply: impl IntoResponse, permit: P) -> Response {
    let (parts, body) = reply.into_response().into_parts();
    Response::from_parts(
        parts,
        axum::body::Body::new(crate::plugin_host::intake::ScopedBody::new(body, permit)),
    )
}

#[cfg(test)]
#[tokio::test]
async fn resource_reply_holds_account_and_plugin_admission_until_body_drops() {
    use crate::plugin_host::intake::{Intake, Scope};
    use std::collections::BTreeSet;

    let gate = Arc::new(RwLock::new(()));
    let intake = Intake::new(true);
    let permit = intake.acquire_for(Some("builtin.model-services")).unwrap();
    let reply = hold_response(
        Json(json!({"resource":"fixture"})),
        (gate.clone().read_owned().await, permit),
    );
    let scope = || Scope::Plugins(BTreeSet::from(["builtin.model-services".into()]));
    assert!(gate.try_write().is_err());
    assert!(
        intake
            .transition_scope(Duration::from_millis(1), scope())
            .is_err()
    );

    drop(reply);
    assert!(gate.try_write().is_ok());
    assert!(
        intake
            .transition_scope(Duration::from_millis(1), scope())
            .is_ok()
    );
}

pub(crate) async fn guard(
    State(state): State<AppState>,
    mut request: Request,
    next: Next,
) -> Response {
    let Ok(permit) = state.iam.gate.clone().try_read_owned() else {
        return busy().into_response();
    };
    match admit_owner(&state, session_digest(request.headers())).await {
        Ok(account) => {
            if account.is_some()
                && !ready(&state)
                && !matches!(
                    request.uri().path(),
                    "/api/builtin-plugins"
                        | "/api/builtin-plugins/changes"
                        | "/api/builtin-plugins/operations"
                )
            {
                return unavailable().into_response();
            }
            if let Some(account) = account {
                request.extensions_mut().insert(Actor(account.id));
            }
            hold_response(next.run(request).await, permit)
        }
        Err(error) => error.into_response(),
    }
}

async fn admission(State(state): State<AppState>, headers: HeaderMap) -> ApiResult {
    let _permit = state
        .iam
        .gate
        .clone()
        .try_read_owned()
        .map_err(|_| busy())?;
    if admit_owner(&state, session_digest(&headers))
        .await?
        .is_some()
    {
        require_plugin(&state)?;
    }
    Ok(Json(json!({"allowed":true})))
}
async fn status(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, Failure> {
    let permit = tokio::time::timeout(GATE_WAIT, state.iam.gate.clone().read_owned())
        .await
        .map_err(|_| busy())?;
    let digest = session_digest(&headers);
    let iam = state.iam.clone();
    let (enabled, account) = with_store(&state, move |store| {
        let enabled = enabled(store)?;
        let account = if enabled {
            match iam.authenticate(store, digest) {
                Ok(account) => Some(account),
                Err((StatusCode::UNAUTHORIZED, _)) => None,
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        Ok((enabled, account))
    })
    .await?;
    Ok(hold_response(
        Json(json!({"enabled":enabled,"account":account,"available":!enabled || ready(&state)})),
        permit,
    ))
}

pub(crate) fn routes(state: AppState) -> Router<AppState> {
    Router::new()
        .route("/api/iam/status", get(status))
        .route("/api/iam/admission", get(admission))
        .route(
            "/api/iam",
            post(action).layer(axum::extract::DefaultBodyLimit::max(8192)),
        )
        .layer(middleware::from_fn_with_state(state, crate::api::authorize))
}

#[derive(Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum Action {
    Prepare {
        recovery: bool,
    },
    Initialize {
        operation_id: String,
        username: String,
        display_name: String,
        password: String,
    },
    Recover {
        operation_id: String,
        recovery_code: String,
        password: String,
    },
    Login {
        username: String,
        password: String,
    },
    Logout,
    Accounts,
    Create {
        username: String,
        display_name: String,
        role: Role,
        password: String,
    },
    Edit {
        account_id: String,
        input: AccountEdit,
    },
    Delete {
        account_id: String,
        revision: u32,
    },
    Password {
        account_id: String,
        revision: u32,
        password: String,
        current_password: Option<String>,
    },
    Grants {
        account_id: String,
    },
    Grant {
        account_id: String,
        service_id: String,
        allow: bool,
    },
    Resources,
    Resource {
        service_id: String,
    },
}

async fn action(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<Action>,
) -> Result<Response, Failure> {
    let digest = session_digest(&headers);
    match input {
        Action::Login { username, password } => login(&state, digest, username, password)
            .await
            .map(IntoResponse::into_response),
        Action::Accounts | Action::Grants { .. } | Action::Resources | Action::Resource { .. } => {
            let permit = state
                .iam
                .gate
                .clone()
                .try_read_owned()
                .map_err(|_| busy())?;
            let model_scope = if matches!(input, Action::Resources | Action::Resource { .. }) {
                Some(
                    state
                        .readonly
                        .intake
                        .acquire_for(Some("builtin.model-services"))
                        .ok_or_else(model_unavailable)?,
                )
            } else {
                None
            };
            Ok(hold_response(
                account_action(state, digest, input).await?,
                (permit, model_scope),
            ))
        }
        _ => write_action(state, digest, input)
            .await
            .map(IntoResponse::into_response),
    }
}

fn model_unavailable() -> Failure {
    failure(
        StatusCode::SERVICE_UNAVAILABLE,
        "plugin_disabled",
        "模型服务插件当前不可用",
    )
}

async fn login(
    state: &AppState,
    digest: Option<[u8; 32]>,
    username: String,
    password: String,
) -> ApiResult {
    if password.len() > 256 || !state.iam.enabled.load(Ordering::Acquire) {
        return Err(invalid_credentials());
    }
    state.iam.attempt()?;
    let record = with_store(state, move |store| {
        if local_agent_core::iam::normalize_username(&username).is_err() {
            return Ok(None);
        }
        store.iam_login(&username).map_err(plugin_error)
    })
    .await?;
    let password = Zeroizing::new(password);
    let (id, revision) = state
        .iam
        .crypto(move || {
            if let Some((id, hash, revision)) = record {
                if verify(&password, &hash) {
                    return Ok((id, revision));
                }
            } else {
                let _ = hash(&password)?;
            }
            Err(invalid_credentials())
        })
        .await?;

    // Password verification can run alongside normal business I/O. Revalidate
    // the account before publishing its session under the mutation gate.
    let _permit = state.iam.write_gate().await?;
    let iam = state.iam.clone();
    with_store(state, move |store| {
        match iam.authenticate(store, digest) {
            Ok(_) => {
                return Err(failure(
                    StatusCode::CONFLICT,
                    "already_signed_in",
                    "请先退出当前账号再登录",
                ));
            }
            Err((StatusCode::UNAUTHORIZED, _)) => {}
            Err(error) => return Err(error),
        }
        let account = store.iam_account(&id).map_err(|error| {
            if error.code == "not_found" {
                invalid_credentials()
            } else {
                plugin_error(error)
            }
        })?;
        if !account.enabled || account.auth_revision != revision {
            return Err(invalid_credentials());
        }
        let token = iam.create_session(&account)?;
        Ok(Json(json!({"account":account,"session":token.as_str()})))
    })
    .await
}

async fn write_action(state: AppState, digest: Option<[u8; 32]>, input: Action) -> ApiResult {
    let _permit = state.iam.write_gate().await?;
    match input {
        Action::Prepare { recovery } => {
            if with_store(&state, enabled).await? != recovery {
                return Err(failure(
                    StatusCode::CONFLICT,
                    "invalid_state",
                    "当前账号管理状态已变化",
                ));
            }
            state.iam.prepare(recovery)
        }
        Action::Initialize {
            operation_id,
            username,
            display_name,
            password,
        } => {
            if with_store(&state, enabled).await? {
                return Err(plugin_error(PluginError::new(
                    "iam_locked",
                    "账号管理已开启",
                )));
            }
            password_valid(&password)?;
            let code = state.iam.code(&operation_id, false)?;
            let password = Zeroizing::new(password);
            let (password_hash, recovery_hash) = state
                .iam
                .crypto(move || Ok((hash(&password)?, hash(&code)?)))
                .await?;
            let bootstrap = Bootstrap {
                username,
                display_name,
                password_hash,
                recovery_hash,
            };
            let own = state.clone();
            crate::blocking::run(move || {
                crate::plugin_host::builtin::enable_iam(&own, operation_id, bootstrap)
            })
            .await
            .map_err(|_| unavailable())?
            .map_err(plugin_error)?;
            *state.iam.challenge.lock().map_err(|_| unavailable())? = None;
            Ok(Json(json!({"enabled":true})))
        }
        Action::Recover {
            operation_id,
            recovery_code,
            password,
        } => {
            state.iam.attempt()?;
            password_valid(&password)?;
            let code = state.iam.code(&operation_id, true)?;
            let recovery_code = Zeroizing::new(recovery_code);
            let password = Zeroizing::new(password);
            let prior = with_store(&state, |store| {
                store.iam_recovery_hash().map_err(plugin_error)
            })
            .await?
            .ok_or_else(invalid_credentials)?;
            let (prior, password_hash, recovery_hash) = state
                .iam
                .crypto(move || {
                    if recovery_code.len() != 64 || !verify(&recovery_code, &prior) {
                        return Err(invalid_credentials());
                    }
                    Ok((prior, hash(&password)?, hash(&code)?))
                })
                .await?;
            with_store(&state, move |store| {
                store
                    .iam_recover(&prior, &password_hash, &recovery_hash)
                    .map_err(plugin_error)
            })
            .await?;
            state
                .iam
                .sessions
                .lock()
                .map_err(|_| unavailable())?
                .clear();
            *state.iam.challenge.lock().map_err(|_| unavailable())? = None;
            Ok(Json(json!({"recovered":true})))
        }
        Action::Logout => {
            if let Some(digest) = digest {
                state
                    .iam
                    .sessions
                    .lock()
                    .map_err(|_| unavailable())?
                    .remove(&digest);
            }
            Ok(Json(json!({"loggedOut":true})))
        }
        _ => account_action(state, digest, input).await,
    }
}

async fn account_action(state: AppState, digest: Option<[u8; 32]>, input: Action) -> ApiResult {
    let iam = state.iam.clone();
    let account = with_store(&state, move |store| iam.authenticate(store, digest)).await?;
    require_plugin(&state)?;
    if matches!(
        &input,
        Action::Resources | Action::Resource { .. } | Action::Grant { allow: true, .. }
    ) && !crate::plugin_host::features::active(&state, "builtin.model-services")
    {
        return Err(model_unavailable());
    }
    if !matches!(
        &input,
        Action::Resources | Action::Resource { .. } | Action::Password { .. }
    ) && account.role != Role::Owner
    {
        return Err(denied());
    }
    let actor = account.id.clone();
    match input {
        Action::Accounts => json_value(
            with_store(&state, |store| store.iam_accounts().map_err(plugin_error)).await?,
        ),
        Action::Create {
            username,
            display_name,
            role,
            password,
        } => {
            password_valid(&password)?;
            let password = Zeroizing::new(password);
            let hash = state.iam.crypto(move || hash(&password)).await?;
            json_value(
                with_store(&state, move |store| {
                    store
                        .iam_create(&actor, &username, &display_name, &hash, role)
                        .map_err(plugin_error)
                })
                .await?,
            )
        }
        Action::Edit { account_id, input } => json_value(
            with_store(&state, move |store| {
                store
                    .iam_edit(&actor, &account_id, input)
                    .map_err(plugin_error)
            })
            .await?,
        ),
        Action::Delete {
            account_id,
            revision,
        } => {
            with_store(&state, move |store| {
                store
                    .iam_delete(&actor, &account_id, revision)
                    .map_err(plugin_error)
            })
            .await?;
            Ok(Json(json!({"deleted":true})))
        }
        Action::Password {
            account_id,
            revision,
            password,
            current_password,
        } => {
            if account_id != actor && account.role != Role::Owner {
                return Err(denied());
            }
            password_valid(&password)?;
            let password = Zeroizing::new(password);
            let prior = if account_id == actor {
                state.iam.attempt()?;
                let current_password =
                    Zeroizing::new(current_password.ok_or_else(invalid_credentials)?);
                if current_password.len() > 256 {
                    return Err(invalid_credentials());
                }
                let username = account.username;
                let (_, hash, _) = with_store(&state, move |store| {
                    store.iam_login(&username).map_err(plugin_error)
                })
                .await?
                .ok_or_else(invalid_credentials)?;
                Some((current_password, hash))
            } else {
                None
            };
            let hash = state
                .iam
                .crypto(move || {
                    if prior.is_some_and(|(password, hash)| !verify(&password, &hash)) {
                        return Err(invalid_credentials());
                    }
                    hash(&password)
                })
                .await?;
            with_store(&state, move |store| {
                store
                    .iam_set_password(&actor, &account_id, revision, &hash)
                    .map_err(plugin_error)
            })
            .await?;
            Ok(Json(json!({"updated":true})))
        }
        Action::Grants { account_id } => json_value(
            with_store(&state, move |store| {
                store.iam_grants(&account_id).map_err(plugin_error)
            })
            .await?,
        ),
        Action::Grant {
            account_id,
            service_id,
            allow,
        } => {
            with_store(&state, move |store| {
                store
                    .iam_grant(&actor, &account_id, &service_id, allow)
                    .map_err(plugin_error)
            })
            .await?;
            Ok(Json(json!({"allowed":allow})))
        }
        Action::Resources => json_value(
            with_store(&state, move |store| {
                let grants = store.iam_grants(&actor).map_err(plugin_error)?;
                let services = store.model_services().map_err(|_| unavailable())?;
                Ok(services
                    .into_iter()
                    .filter(|service| account.role == Role::Owner || grants.contains(&service.id))
                    .map(|service| json!({"id":service.id,"name":service.name}))
                    .collect::<Vec<_>>())
            })
            .await?,
        ),
        Action::Resource { service_id } => json_value(
            with_store(&state, move |store| {
                let missing = || failure(StatusCode::NOT_FOUND, "not_found", "当前资源不可查看");
                if account.role != Role::Owner
                    && !store
                        .iam_grants(&actor)
                        .map_err(plugin_error)?
                        .contains(&service_id)
                {
                    return Err(missing());
                }
                let service = store.model_service(&service_id).map_err(|_| missing())?;
                let creator = store.iam_model_creator(&service.id).map_err(plugin_error)?;
                Ok(
                    json!({"id":service.id,"name":service.name,"creator":creator,
                "models":service.models.into_iter().map(|model| model.name).collect::<Vec<_>>(),
                "tenantId":DEFAULT_TENANT}),
                )
            })
            .await?,
        ),
        _ => Err(denied()),
    }
}
