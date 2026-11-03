use super::*;
use rusqlite::functions::{Aggregate, Context as FunctionContext, FunctionFlags};
use sha2::{Digest, Sha256};
use std::borrow::Cow;

// A statement-local decode cache, not a result cache. Each row is still added
// individually, preserving saturating counters, money order and missing values.
// Large or high-cardinality summaries cannot grow the cache without bound.
fn decoded<'a, T: serde::de::DeserializeOwned + Clone>(
    cache: &'a mut HashMap<String, T>,
    raw: &str,
) -> rusqlite::Result<Cow<'a, T>> {
    let parse =
        || serde_json::from_str(raw).map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)));
    if raw.len() > 8192 {
        return parse().map(Cow::Owned);
    }
    if !cache.contains_key(raw) {
        if cache.len() == 64 {
            cache.clear();
        }
        cache.insert(raw.to_owned(), parse()?);
    }
    Ok(Cow::Borrowed(cache.get(raw).unwrap()))
}

#[derive(Default)]
struct Sum {
    summary: requests::Summary,
    input: u128,
    cached: u128,
    rows: HashMap<String, requests::Summary>,
}
struct SummarySum {
    block: bool,
}
impl Aggregate<Sum, String> for SummarySum {
    fn init(&self, _: &mut FunctionContext<'_>) -> rusqlite::Result<Sum> {
        Ok(Sum::default())
    }
    fn step(&self, ctx: &mut FunctionContext<'_>, s: &mut Sum) -> rusqlite::Result<()> {
        let row = decoded(&mut s.rows, ctx.get_raw(0).as_str()?)?;
        let a = &mut s.summary;
        a.requests += row.requests;
        a.attempts += row.attempts;
        a.usage_missing += row.usage_missing;
        a.provisional += row.provisional;
        a.cache_samples += row.cache_samples;
        a.usage.add(&row.usage);
        a.cost.merge(&row.cost);
        for (k, n) in &row.statuses {
            if let Some(old) = a.statuses.get_mut(k) {
                *old += n;
            } else {
                a.statuses.insert(k.clone(), *n);
            }
        }
        for (k, n) in &row.issues {
            if let Some(old) = a.issues.get_mut(k) {
                *old += n;
            } else {
                a.issues.insert(k.clone(), *n);
            }
        }
        s.input = s
            .input
            .saturating_add(ctx.get_raw(1).as_str()?.parse::<u128>().unwrap_or(0));
        s.cached = s
            .cached
            .saturating_add(ctx.get_raw(2).as_str()?.parse::<u128>().unwrap_or(0));
        Ok(())
    }
    fn finalize(&self, _: &mut FunctionContext<'_>, s: Option<Sum>) -> rusqlite::Result<String> {
        let mut s = s.unwrap_or_default();
        let count = |k: &str| *s.summary.statuses.get(k).unwrap_or(&0);
        let completed = count("success") + count("recovered") + count("failed");
        s.summary.success_rate = (completed > 0 && s.summary.attempts > 0)
            .then(|| (count("success") + count("recovered")) as f64 / completed as f64);
        s.summary.cache_ratio = (s.input > 0).then(|| s.cached as f64 / s.input as f64);
        if self.block {
            let mut value = serde_json::to_value(&s.summary)
                .map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)))?;
            value["_cacheInput"] = json!(s.input.to_string());
            value["_cacheRead"] = json!(s.cached.to_string());
            value["_money"] =
                json!(!s.summary.cost.actual.is_empty() || !s.summary.cost.estimated.is_empty());
            Ok(value.to_string())
        } else {
            serde_json::to_string(&s.summary)
                .map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)))
        }
    }
}
struct HashRows;
impl Aggregate<Sha256, String> for HashRows {
    fn init(&self, _: &mut FunctionContext<'_>) -> rusqlite::Result<Sha256> {
        Ok(Sha256::new())
    }
    fn step(&self, c: &mut FunctionContext<'_>, s: &mut Sha256) -> rusqlite::Result<()> {
        let v = c.get_raw(0).as_str()?;
        s.update((v.len() as u64).to_le_bytes());
        s.update(v.as_bytes());
        Ok(())
    }
    fn finalize(&self, _: &mut FunctionContext<'_>, s: Option<Sha256>) -> rusqlite::Result<String> {
        Ok(format!("{:x}", s.unwrap_or_default().finalize()))
    }
}
/// Only additive totals are merged here; ranks and most-recent references are
/// selected by SQLite. Counters retain u64 saturating semantics, including >i64.
struct JsonTotals;
#[derive(Default)]
struct JsonSum {
    value: Value,
    rows: HashMap<String, Value>,
}
fn merge(a: &mut Value, b: &Value, key: &str) {
    if key == "lastSeen" {
        let instant = |v: &Value| {
            v.as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        };
        let replace = match (instant(a), instant(b)) {
            (old, Some(new)) => old.is_none_or(|old| new > old),
            // Preserve the generic aggregate's legacy text semantics when
            // neither side is a timestamp; malformed text cannot replace time.
            (None, None) => b
                .as_str()
                .is_some_and(|new| a.as_str().is_none_or(|old| new > old)),
            (Some(_), None) => false,
        };
        if replace {
            *a = b.clone();
        }
        return;
    }
    if key.contains("P95") || key.ends_with("Samples") {
        return;
    }
    match (a, b) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                merge(a.entry(k).or_insert(Value::Null), v, k);
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for item in b {
                if let Some(old) = a.iter_mut().find(|v| v["currency"] == item["currency"]) {
                    merge(old, item, "");
                } else {
                    a.push(item.clone());
                }
            }
        }
        (a, Value::Number(b)) => {
            if let Some(n) = b.as_u64() {
                *a = json!(a.as_u64().unwrap_or(0).saturating_add(n));
            } else if let Some(n) = b.as_f64() {
                *a = json!(a.as_f64().unwrap_or(0.0) + n);
            }
        }
        (a, b) if a.is_null() => *a = b.clone(),
        _ => {}
    }
}
impl Aggregate<JsonSum, String> for JsonTotals {
    fn init(&self, _: &mut FunctionContext<'_>) -> rusqlite::Result<JsonSum> {
        Ok(JsonSum {
            value: json!({}),
            ..Default::default()
        })
    }
    fn step(&self, c: &mut FunctionContext<'_>, s: &mut JsonSum) -> rusqlite::Result<()> {
        let v = decoded(&mut s.rows, c.get_raw(0).as_str()?)?;
        merge(&mut s.value, &v, "");
        Ok(())
    }
    fn finalize(
        &self,
        _: &mut FunctionContext<'_>,
        s: Option<JsonSum>,
    ) -> rusqlite::Result<String> {
        Ok(s.map(|s| s.value).unwrap_or(json!({})).to_string())
    }
}
pub(super) fn register_directory(
    db: &Connection,
    directory: std::sync::Arc<crate::agents::AgentDirectory>,
) -> Result<(), String> {
    let agents = directory.clone();
    db.create_scalar_function(
        "agent_matches",
        3,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        move |c| {
            Ok(agents.matches(
                c.get::<Option<String>>(0)?.as_deref(),
                &c.get::<String>(1)?,
                c.get::<Option<String>>(2)?.as_deref(),
            ))
        },
    )
    .map_err(fail)?;
    db.create_scalar_function(
        "agent_group_matches",
        2,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        move |c| {
            Ok(directory
                .matches_group(c.get::<Option<String>>(0)?.as_deref(), &c.get::<String>(1)?))
        },
    )
    .map_err(fail)
}
pub(in crate::observation) fn register(db: &Connection) -> Result<(), String> {
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;
    db.create_aggregate_function("summary_sum", 3, flags, SummarySum { block: false })
        .map_err(fail)?;
    db.create_aggregate_function("summary_block_sum", 3, flags, SummarySum { block: true })
        .map_err(fail)?;
    db.create_aggregate_function("stats_sum", 1, flags, JsonTotals)
        .map_err(fail)?;
    db.create_aggregate_function("hash_rows", 1, flags, HashRows)
        .map_err(fail)?;
    db.create_scalar_function("row_digest", 1, flags, |c| {
        Ok(format!(
            "{:x}",
            Sha256::digest(c.get_raw(0).as_str()?.as_bytes())
        ))
    })
    .map_err(fail)?;
    db.create_scalar_function("agent_matches", 3, flags, |c| {
        Ok(crate::agents::matches(
            c.get::<Option<String>>(0)?.as_deref(),
            &c.get::<String>(1)?,
            c.get::<Option<String>>(2)?.as_deref(),
        ))
    })
    .map_err(fail)?;
    db.create_scalar_function("unicode_lower", 1, flags, |c| {
        Ok(c.get::<String>(0)?.to_lowercase())
    })
    .map_err(fail)?;
    db.create_scalar_function("agent_group_matches", 2, flags, |c| {
        Ok(crate::agents::matches_group(
            c.get::<Option<String>>(0)?.as_deref(),
            &c.get::<String>(1)?,
        ))
    })
    .map_err(fail)?;
    db.create_scalar_function("instant_ms", 1, flags, |c| {
        Ok(c.get::<Option<String>>(0)?.as_deref().and_then(timestamp))
    })
    .map_err(fail)?;
    db.create_scalar_function("instant_cmp", 2, flags, |c| {
        let parse = |s: String| chrono::DateTime::parse_from_rfc3339(&s).ok();
        Ok(parse(c.get(0)?)
            .zip(parse(c.get(1)?))
            .map(|(a, b)| match a.cmp(&b) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            }))
    })
    .map_err(fail)?;
    db.create_scalar_function("instant_key", 1, flags, |c| {
        Ok(c.get::<Option<String>>(0)?
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok())
            .map(|t| {
                t.with_timezone(&chrono::Utc)
                    .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
            }))
    })
    .map_err(fail)?;
    Ok(())
}
pub(super) fn cost_metric(c: &crate::provider_cost::Totals, currency: &str) -> Option<f64> {
    let amounts: Vec<_> = c
        .actual
        .iter()
        .chain(&c.estimated)
        .filter(|c| c.currency == currency)
        .collect();
    (!amounts.is_empty()).then(|| amounts.iter().map(|v| v.amount).sum())
}
pub(in crate::observation) fn percentile(
    db: &Connection,
    from: &str,
    expression: &str,
    p: usize,
) -> Result<(usize, Option<f64>), String> {
    let count: usize = db
        .query_row(
            &format!("SELECT count(*) FROM {from} WHERE {expression} IS NOT NULL"),
            [],
            |r| sql_count(r, 0),
        )
        .map_err(fail)?;
    if count == 0 {
        return Ok((0, None));
    }
    let offset = (count * p).div_ceil(100) - 1;
    let n=db.query_row(&format!("SELECT {expression} FROM {from} WHERE {expression} IS NOT NULL ORDER BY {expression} LIMIT 1 OFFSET {offset}"),[],|r|r.get(0)).map_err(fail)?;
    Ok((count, Some(n)))
}
pub(in crate::observation) fn summary(
    db: &Connection,
    from: &str,
) -> Result<requests::Summary, String> {
    finish_summary(db, from, summary_base(db, from)?)
}

/// Retention drafts need a stable arithmetic order across rebuilt projections.
/// Live report callers keep their existing aggregation path and query contract.
pub(in crate::observation) fn ordered_summary(
    db: &Connection,
    from: &str,
) -> Result<requests::Summary, String> {
    let raw:String=db.query_row(&format!("SELECT summary_sum(stats,cache_in,cache_read ORDER BY fact_order,sub_order) FROM {from}"),[],|r|r.get(0)).map_err(fail)?;
    finish_summary(db, from, serde_json::from_str(&raw).map_err(fail)?)
}

fn finish_summary(
    db: &Connection,
    from: &str,
    mut s: requests::Summary,
) -> Result<requests::Summary, String> {
    for (key, target) in [
        ("ttft", &mut s.ttft),
        ("duration", &mut s.duration),
        ("throughput", &mut s.throughput),
    ] {
        let expression = format!("{key}_sample");
        let (n, p50) = percentile(db, from, &expression, 50)?;
        let p95 = if n == 0 {
            None
        } else {
            let offset = (n * 95).div_ceil(100) - 1;
            Some(db.query_row(&format!("SELECT {expression} FROM {from} WHERE {expression} IS NOT NULL ORDER BY {expression} LIMIT 1 OFFSET {offset}"),[],|r|r.get(0)).map_err(fail)?)
        };
        *target = requests::Distribution {
            samples: n,
            p50,
            p95,
        };
    }
    Ok(s)
}
pub(super) fn summary_base(db: &Connection, from: &str) -> Result<requests::Summary, String> {
    let raw: String = db
        .query_row(
            &format!("SELECT summary_sum(stats,cache_in,cache_read) FROM {from}"),
            [],
            |r| r.get(0),
        )
        .map_err(fail)?;
    serde_json::from_str(&raw).map_err(fail)
}
pub(super) fn list(db: &Connection, sql: &str) -> Result<Vec<Value>, String> {
    let mut s = db.prepare(sql).map_err(fail)?;
    s.query_map([], |r| r.get::<_, String>(0))
        .map_err(fail)?
        .map(|v| serde_json::from_str(&v.map_err(fail)?).map_err(fail))
        .collect()
}
pub(in crate::observation) fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''").replace('\0', "'||char(0)||'"))
}
