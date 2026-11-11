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
    key: RefreshKey,
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
    lease: Option<local_agent_core::plugin::PluginLease>,
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
    let lease = account
        .source_kind
        .map(|kind| billing.sources.lease(kind))
        .transpose()
        .map_err(|_| err("额度来源插件已停止"))?;
    let key = RefreshKey::new(&account, lease.as_ref().map(|lease| lease.epoch));
    billing
        .in_flight
        .retain(|_, active| active.load(Ordering::Acquire));
    if billing.in_flight.keys().any(|other| key.same_read(other)) {
        return Err(err("此账户正在刷新，请等待当前请求完成"));
    }
    billing
        .attempted
        .retain(|_, started| started.elapsed() < Duration::from_secs(10));
    let recent = billing.attempted.keys().any(|other| key.same_read(other));
    if recent
        && (!billing.attempted.contains_key(&key)
            || !billing
                .cached(&account, key.epoch)
                .is_some_and(|c| c.error.is_none() && !is_stale(&c.snapshot)))
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
        key: key.clone(),
        active: active.clone(),
    };
    billing.in_flight.insert(key.clone(), active);
    billing.attempted.insert(key, Instant::now());
    // Construct the owner inside the worker. A cancelled receiver drops it
    // here, rather than leaving registration without a cancellation guard.
    Ok(Prepared::Read(Box::new(Reading {
        account,
        home: billing.home.clone(),
        sources: billing.sources.clone(),
        flight,
        lease,
    })))
}

fn publish(
    state: &AppState,
    account: Account,
    flight: Flight,
    outcome: Result<AccountSnapshot, ReadError>,
    lease: Option<local_agent_core::plugin::PluginLease>,
) -> ApiResult {
    let changed = || err("读取期间配置已改变，已丢弃旧账户结果");
    let store = if let Some(provider_id) = &account.provider_id {
        let store = state.store.lock().map_err(|_| err("账户目录暂不可用"))?;
        let provider = store
            .provider_by_id(provider_id)
            .map_err(crate::error)?
            .filter(|provider| {
                provider.configured
                    && (account.upstream_account.is_some() || provider.category != "local")
            })
            .ok_or_else(changed)?;
        let target = upstream::target(&store, provider, &account).map_err(|_| changed())?;
        if api::adapter(&target).map(|(kind, _)| kind) != account.source_kind {
            return Err(changed());
        }
        // An account mutation must not interleave between this check and the
        // publication of a result that can also affect other accounts.
        Some((store, target))
    } else {
        if !catalog(state)?
            .iter()
            .any(|a| a.id == account.id && a.source_revision == account.source_revision)
        {
            return Err(changed());
        }
        None
    };
    let source_epoch = lease.as_ref().map(|lease| lease.epoch);
    let mut billing = state.billing.lock().map_err(|_| err("账户缓存暂不可用"))?;
    if !flight.active.load(Ordering::Acquire)
        || !billing
            .in_flight
            .get(&flight.key)
            .is_some_and(|active| Arc::ptr_eq(active, &flight.active))
    {
        return Err(err("账户刷新已结束，已丢弃旧结果"));
    }
    let update = || {
        if let Some(scope) = &flight.key.read_scope {
            // A completed read supersedes older amounts, including on other
            // entries for the same credential. Keep their display snapshots.
            for cached in billing.cache.values_mut().filter(|cached| {
                cached.source_epoch == source_epoch
                    && cached.current_read_scope.as_ref() == Some(scope)
            }) {
                cached.current_read_scope = None;
            }
        }
        outcome.and_then(|snapshot| {
            billing
                .cache_snapshot(
                    account.id.clone(),
                    account.source_revision.clone(),
                    source_epoch,
                    store.as_ref().map(|(_, target)| target),
                    snapshot,
                    chrono::Utc::now(),
                )
                .map_err(Into::into)
        })
    };
    let outcome = if let Some(lease) = &lease {
        lease
            .publish(update)
            .map_err(|_| err("读取期间额度插件激活已变化，已丢弃旧结果"))?
    } else {
        update()
    };
    let outcome = outcome.and_then(|()| {
        if let (Some((store, target)), Some(epoch), Some(source)) =
            (&store, source_epoch, account.source_kind)
        {
            // Configuration and source transitions share this store guard.
            // Keep SQLite work outside the plugin publication barrier.
            match source {
                "openrouter" => billing
                    .persist_openrouter_refresh(
                        store,
                        epoch,
                        target,
                        &billing.cache[&account.id].snapshot,
                    )
                    .map_err(|_| "OpenRouter 额度证据未能保存，请核查后刷新")?,
                local_agent_core::providers::subscription::MINIMAX_PLAN => {
                    let snapshot = &billing.cache[&account.id].snapshot;
                    upstream::minimax::record(store, target, snapshot, chrono::Utc::now())
                        .map_err(|_| "MiniMax 额度证据未能保存，请核查后刷新")?;
                }
                "deepseek" | "kimi-cn" | "kimi-global" => {
                    let snapshot = &billing.cache[&account.id].snapshot;
                    upstream::balance::record(store, target, snapshot, chrono::Utc::now())
                        .map_err(|_| "上游余额状态未能保存，请核查后刷新")?;
                }
                _ => (),
            }
        }
        Ok(())
    });
    match outcome {
        Ok(()) => {
            billing.failures.remove(&account.id);
        }
        Err(reason) => {
            billing
                .failures
                .insert(account.id.clone(), (flight.key.clone(), reason));
            if let Some(cached) = billing
                .cache
                .get_mut(&account.id)
                .filter(|c| c.revision == account.source_revision && c.source_epoch == source_epoch)
            {
                cached.error = Some(reason.message().into());
            }
            return Err(err(reason.message()));
        }
    }
    // Ownership stays with this worker if its HTTP waiter was cancelled.
    flight.finish();
    drop(billing);
    drop(store);
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
    let (account, home, sources, flight, lease) = match prepared {
        Prepared::Cached(value) => return Ok(value),
        Prepared::Read(reading) => {
            let Reading {
                account,
                home,
                sources,
                flight,
                lease,
            } = *reading;
            (account, home, sources, flight, lease)
        }
    };
    let outcome = if account.id == "native-codex" {
        if Some(&home) != dirs::home_dir().as_ref() {
            Err("隔离来源不读取本机真实登录账户".into())
        } else {
            sources.codex(flight.key.epoch).await
        }
    } else {
        read_api(&state, &account, &home, &sources, flight.key.epoch).await
    };
    let capacity = state.query_capacity.clone();
    crate::blocking::run_with(capacity, move || {
        publish(&state, account, flight, outcome, lease)
    })
    .await
    .map_err(worker_error)?
}
