//! Keep account bookkeeping off the async runtime; cancellation only flips a token.
use super::*;

#[cfg(test)]
mod tests;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Refresh {
    expected_revision: String,
}

struct Flight {
    active: Arc<AtomicBool>,
}
impl Flight {
    fn finish(&self) {
        self.active.store(false, Ordering::Release);
    }
}
impl Drop for Flight {
    fn drop(&mut self) {
        self.finish();
    }
}

enum Prepared {
    Cached(Json<Value>),
    Read(Box<Reading>),
}
struct Reading {
    account: Account,
    home: PathBuf,
    sources: Arc<sources::AccountSources>,
    flight: Flight,
}

fn worker_error(error: crate::blocking::Error) -> (StatusCode, Json<Value>) {
    let (code, message, unknown) = match error {
        crate::blocking::Error::Busy => ("busy", "账户本机处理繁忙，请稍后重试", false),
        crate::blocking::Error::Unavailable => (
            "account_unavailable",
            "账户本机处理未能完成，请重新核查",
            true,
        ),
    };
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"code":code,"message":message,"outcomeUnknown":unknown})),
    )
}

fn prepare(
    state: &AppState,
    id: String,
    input: Refresh,
) -> Result<Prepared, (StatusCode, Json<Value>)> {
    let accounts = catalog(state)?;
    let account = accounts
        .iter()
        .find(|a| a.id == id)
        .cloned()
        .ok_or_else(|| err("账户不存在或配置已移除"))?;
    if input.expected_revision != account.source_revision {
        return Err(err("账户配置已改变，请重新读取目录"));
    }
    if !account.readable {
        return Err(err("此来源需通过官方入口或 Claude 额度采集入口更新"));
    }
    let mut billing = state.billing.lock().map_err(|_| err("账户服务暂不可用"))?;
    billing
        .in_flight
        .retain(|_, active| active.load(Ordering::Acquire));
    if billing.in_flight.contains_key(&id) {
        return Err(err("此账户正在刷新，请等待当前请求完成"));
    }
    let recent = billing
        .attempted
        .get(&id)
        .is_some_and(|t| t.elapsed() < Duration::from_secs(10));
    if recent
        && !billing.cache.get(&id).is_some_and(|c| {
            c.revision == input.expected_revision && c.error.is_none() && !is_stale(&c.snapshot)
        })
    {
        return Err(err("请等待 10 秒后再刷新账户，避免服务商限流"));
    }
    if recent {
        drop(billing);
        return Ok(Prepared::Cached(json_value(
            json!({"accounts":catalog(state)?,"asOf":chrono::Utc::now().to_rfc3339()}),
        )?));
    }
    let active = Arc::new(AtomicBool::new(true));
    let flight = Flight {
        active: active.clone(),
    };
    billing.in_flight.insert(id.clone(), active.clone());
    billing.attempted.insert(id, Instant::now());
    // Construct the owner inside the worker. A cancelled receiver drops it
    // here, rather than leaving registration without a cancellation guard.
    Ok(Prepared::Read(Box::new(Reading {
        account,
        home: billing.home.clone(),
        sources: billing.sources.clone(),
        flight,
    })))
}

fn publish(
    state: &AppState,
    account: Account,
    sources: Arc<sources::AccountSources>,
    flight: Flight,
    outcome: Result<AccountSnapshot, &'static str>,
) -> ApiResult {
    if !catalog(state)?
        .iter()
        .any(|a| a.id == account.id && a.source_revision == account.source_revision)
    {
        return Err(err("读取期间配置已改变，已丢弃旧账户结果"));
    }
    {
        let mut billing = state.billing.lock().map_err(|_| err("账户缓存暂不可用"))?;
        if !flight.active.load(Ordering::Acquire)
            || !billing
                .in_flight
                .get(&account.id)
                .is_some_and(|active| Arc::ptr_eq(active, &flight.active))
        {
            return Err(err("账户刷新已结束，已丢弃旧结果"));
        }
        if account
            .source_kind
            .is_some_and(|kind| !sources.active(kind))
        {
            return Err(err("读取期间额度插件已停止，已丢弃结果"));
        }
        match outcome {
            Ok(snapshot) => {
                billing.failures.remove(&account.id);
                billing.cache.insert(
                    account.id.clone(),
                    Cached {
                        revision: account.source_revision,
                        snapshot,
                        error: None,
                    },
                );
            }
            Err(reason) => {
                billing.failures.insert(
                    account.id.clone(),
                    (account.source_revision.clone(), reason.into()),
                );
                if let Some(cached) = billing
                    .cache
                    .get_mut(&account.id)
                    .filter(|c| c.revision == account.source_revision)
                {
                    cached.error = Some(reason.into());
                }
                return Err(err(reason));
            }
        }
        // A completed refresh must not return its own stale "refreshing" flag.
        // Ownership stays with this worker if its HTTP waiter was cancelled.
        flight.finish();
    }
    // Resolve again: the catalog only attaches cache entries with the current
    // source revision, including a configuration edited during publication.
    json_value(json!({"accounts":catalog(state)?,"asOf":chrono::Utc::now().to_rfc3339()}))
}

pub(crate) async fn refresh(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(input): Json<Refresh>,
) -> ApiResult {
    let worker_state = state.clone();
    let prepared = crate::blocking::run_with(state.query_capacity.clone(), move || {
        prepare(&worker_state, id, input)
    })
    .await
    .map_err(worker_error)??;
    let (account, home, sources, flight) = match prepared {
        Prepared::Cached(value) => return Ok(value),
        Prepared::Read(reading) => {
            let Reading {
                account,
                home,
                sources,
                flight,
            } = *reading;
            (account, home, sources, flight)
        }
    };
    let outcome = if account.id == "native-codex" {
        if Some(&home) != dirs::home_dir().as_ref() {
            Err("隔离来源不读取本机真实登录账户")
        } else {
            sources.codex().await
        }
    } else {
        read_api(&state, &account, &home, &sources).await
    };
    let capacity = state.query_capacity.clone();
    crate::blocking::run_with(capacity, move || {
        publish(&state, account, sources, flight, outcome)
    })
    .await
    .map_err(worker_error)?
}
