//! Token-only estimates use a dated, bounded official price snapshot. A model
//! name on a reseller endpoint is never evidence of the upstream vendor's price.
use crate::sessions::Usage;
use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::{Deserialize, Serialize};

const PRICING: &str = "https://api-docs.deepseek.com/quick_start/pricing/";
const OPENROUTER: &str = "https://openrouter.ai/docs/cookbook/administration/usage-accounting";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportedCost {
    pub amount: f64,
    pub currency: String,
    pub source: String,
}
pub fn openrouter_endpoint(endpoint: &str) -> bool {
    url::Url::parse(endpoint).is_ok_and(|u| {
        u.scheme() == "https"
            && ["openrouter.ai", "eu.openrouter.ai", "us.openrouter.ai"]
                .contains(&u.host_str().unwrap_or(""))
            && u.port_or_known_default() == Some(443)
            && u.username().is_empty()
            && u.password().is_none()
            && u.query().is_none()
            && u.fragment().is_none()
            && matches!(
                u.path().trim_end_matches('/'),
                "/api"
                    | "/api/v1"
                    | "/api/v1/chat/completions"
                    | "/api/v1/messages"
                    | "/api/v1/responses"
            )
    })
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Price {
    pub version: &'static str,
    pub checked_at: &'static str,
    pub valid_from: &'static str,
    pub valid_until: &'static str,
    pub model: String,
    pub period: &'static str,
    pub input_per_million: f64,
    pub cached_input_per_million: f64,
    pub output_per_million: f64,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Component {
    pub kind: &'static str,
    pub tokens: u64,
    pub per_million: f64,
    pub amount: f64,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cost {
    pub kind: &'static str,
    pub amount: Option<f64>,
    pub currency: Option<&'static str>,
    pub reason: Option<&'static str>,
    pub source: Option<&'static str>,
    pub price: Option<Price>,
    pub components: Vec<Component>,
}
impl Cost {
    pub fn unknown(reason: &'static str) -> Self {
        Self {
            kind: "unknown",
            amount: None,
            currency: None,
            reason: Some(reason),
            source: None,
            price: None,
            components: vec![],
        }
    }
}

/// Provider pages use the independently published CNY tariff. Other consumers
/// keep their existing USD estimates; no amount is converted between currencies.
pub fn for_provider(row: &crate::observation::ModelObservation) -> Cost {
    let mut cost = calculate(
        Some(&row.endpoint),
        row.model.as_deref(),
        Some(&row.started_at),
        &row.status,
        &row.usage,
        row.reported_cost.as_ref(),
        row.ended_at.as_deref(),
    );
    if cost.reason == Some("missing_usage") {
        // Classify only recorded, recognized gateway outcomes. Do not expose
        // arbitrary upstream error text or infer that a failed call was free.
        cost.reason = Some(
            match (
                row.status.as_str(),
                row.http_status,
                row.timing_issue.as_deref(),
            ) {
                ("error", Some(400 | 422), Some("上游拒绝跨协议请求")) => {
                    "missing_usage_upstream_rejected"
                }
                (
                    "error",
                    Some(200),
                    Some("上游返回当前不可转换的思考或拒绝内容，请使用同协议连接"),
                ) => "missing_usage_unsupported_content",
                ("cancelled", Some(200), Some("客户端结束读取，未收到完整结束事件")) => {
                    "missing_usage_client_cancelled"
                }
                _ => "missing_usage",
            },
        );
    }
    if cost.kind != "estimated" {
        return cost;
    }
    let Some(price) = cost.price.as_mut() else {
        return Cost::unknown("missing_price");
    };
    // Official CNY per-token rates in integer billionths of a yuan, not USD
    // amounts multiplied by an exchange rate. Eligibility/time/usage validation
    // above is shared with the existing dated DeepSeek tariff.
    let peak_rates = if price.model == "deepseek-flash" {
        [2_000u64, 40, 8_000]
    } else if price.model == "deepseek-v4-pro" {
        [9_000u64, 300, 27_000]
    } else {
        return Cost::unknown("missing_price");
    };
    let rates = peak_rates.map(|rate| {
        if price.period == "peak" {
            rate
        } else {
            rate / 2
        }
    });
    let mut total = 0u64;
    for (component, rate) in cost.components.iter_mut().zip(rates) {
        let Some(value) = component.tokens.checked_mul(rate) else {
            return Cost::unknown("inconsistent_usage");
        };
        let Some(sum) = total.checked_add(value) else {
            return Cost::unknown("inconsistent_usage");
        };
        total = sum;
        component.per_million = rate as f64 / 1_000.0;
        component.amount = value as f64 / 1e9;
    }
    price.version = "deepseek-cny-2026-10-25";
    price.checked_at = "2026-10-25";
    price.input_per_million = rates[0] as f64 / 1_000.0;
    price.cached_input_per_million = rates[1] as f64 / 1_000.0;
    price.output_per_million = rates[2] as f64 / 1_000.0;
    cost.source = Some("https://api-docs.deepseek.com/zh-cn/quick_start/pricing/");
    cost.currency = Some("CNY");
    cost.amount = Some(total as f64 / 1e9);
    cost
}

pub fn calculate(
    endpoint: Option<&str>,
    model: Option<&str>,
    time: Option<&str>,
    status: &str,
    usage: &Usage,
    reported: Option<&ReportedCost>,
    ended_at: Option<&str>,
) -> Cost {
    if status == "in_progress" {
        return Cost::unknown("in_progress");
    }
    // Only this documented field, captured on this official endpoint, is an
    // account charge. Native clients' computed `cost: 0` is not a bill.
    if let Some(r) = reported.filter(|r| {
        endpoint.is_some_and(openrouter_endpoint)
            && r.currency == "USD"
            && r.source == "openrouter.usage.cost"
            && r.amount.is_finite()
            && r.amount >= 0.0
            && r.amount <= 1e9
    }) {
        return Cost {
            kind: "actual",
            amount: Some(r.amount),
            currency: Some("USD"),
            reason: None,
            source: Some(OPENROUTER),
            price: None,
            components: vec![],
        };
    }
    if ![
        "success",
        "returned",
        "incomplete",
        "error",
        "failed",
        "cancelled",
    ]
    .contains(&status)
    {
        return Cost::unknown("unconfirmed_usage");
    }
    let Some(endpoint) = endpoint else {
        return Cost::unknown("unknown_endpoint");
    };
    let official = url::Url::parse(endpoint).is_ok_and(|u| {
        u.scheme() == "https"
            && u.host_str() == Some("api.deepseek.com")
            && u.port_or_known_default() == Some(443)
            && ["", "/v1", "/anthropic", "/anthropic/v1"].contains(&u.path().trim_end_matches('/'))
    });
    if !official {
        return Cost::unknown("missing_price");
    }
    // Claude's documented [1m] marker describes context, not another model.
    // DeepSeek officially supports it on its Anthropic endpoint. Keep the raw
    // ID in history, and expose the normalized billing model in Price.
    let model = model.map(|m| {
        if url::Url::parse(endpoint).is_ok_and(|u| u.path().starts_with("/anthropic")) {
            m.strip_suffix("[1m]")
                .or_else(|| m.strip_suffix("[1M]"))
                .unwrap_or(m)
        } else {
            m
        }
    });
    let model = match model {
        Some("deepseek-flash" | "deepseek-v4-flash" | "deepseek-v4-flash-vision-exp") => {
            "deepseek-flash"
        }
        Some("deepseek-v4-pro") => "deepseek-v4-pro",
        _ => return Cost::unknown("missing_price"),
    };
    let Some(time) = time
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.with_timezone(&Utc))
    else {
        return Cost::unknown("unknown_time");
    };
    // Conservative historical boundary: the 10/17 official update explicitly
    // left pricing unchanged. Never backfill an older request with today's rate.
    // Expire after 30 days; further dates require a reviewed price snapshot.
    let from = "2026-10-17T00:00:00Z";
    let until = "2026-11-18T00:00:00Z";
    if time < DateTime::parse_from_rfc3339(from).unwrap()
        || time >= DateTime::parse_from_rfc3339(until).unwrap()
    {
        return Cost::unknown("price_outside_period");
    }
    // There are no Chinese public holidays in this bounded price period (2026
    // State Council calendar). Weekends are off-peak even on make-up work days.
    fn peak(time: DateTime<Utc>) -> bool {
        time.weekday().number_from_monday() <= 5
            && ((1..4).contains(&time.hour()) || (6..10).contains(&time.hour()))
    }
    let Some(end) = ended_at
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.with_timezone(&Utc))
    else {
        return Cost::unknown("unknown_time");
    };
    // A delayed response may cross an entire peak window even if both ends
    // happen to have the same rate. Only a single pricing segment is safe.
    let segment = |t: DateTime<Utc>| {
        (
            t.date_naive(),
            match t.hour() {
                0 => 0,
                1..=3 => 1,
                4..=5 => 2,
                6..=9 => 3,
                _ => 4,
            },
        )
    };
    if end < time
        || end >= DateTime::parse_from_rfc3339(until).unwrap()
        || (segment(time) != segment(end)
            && (peak(time) || peak(end) || end.signed_duration_since(time).num_hours() >= 1))
    {
        return Cost::unknown("rate_boundary");
    }
    let peak = peak(time);
    let (Some(input), Some(output)) = (usage.input_tokens, usage.output_tokens) else {
        return Cost::unknown("missing_usage");
    };
    let Some(cached) = usage.cached_input_tokens.or((input == 0).then_some(0)) else {
        return Cost::unknown("missing_cache_usage");
    };
    if cached > input
        || usage.cache_write_input_tokens.is_some_and(|n| n > 0)
        || usage.reasoning_output_tokens.is_some_and(|n| n > output)
        || input.checked_add(output).is_none()
        || usage
            .total_tokens
            .is_some_and(|n| Some(n) != input.checked_add(output))
    {
        return Cost::unknown("inconsistent_usage");
    }
    // Exact integer nanodollars per token; reasoning is already in output.
    let rates: [u64; 3] = if model == "deepseek-flash" {
        [300, 6, 1200]
    } else {
        [1320, 44, 3960]
    };
    let rates = rates.map(|r| if peak { r } else { r / 2 });
    let quantities = [input - cached, cached, output];
    let mut total = 0u64;
    let mut components = vec![];
    for ((kind, tokens), rate) in ["input", "cached_input", "output"]
        .into_iter()
        .zip(quantities)
        .zip(rates)
    {
        let Some(amount) = tokens
            .checked_mul(rate)
            .and_then(|n| total.checked_add(n).map(|sum| (n, sum)))
        else {
            return Cost::unknown("inconsistent_usage");
        };
        total = amount.1;
        components.push(Component {
            kind,
            tokens,
            per_million: rate as f64 / 1000.0,
            amount: amount.0 as f64 / 1e9,
        });
    }
    Cost {
        kind: "estimated",
        amount: Some(total as f64 / 1e9),
        currency: Some("USD"),
        reason: None,
        source: Some(PRICING),
        price: Some(Price {
            version: "deepseek-usd-2026-10-19",
            checked_at: "2026-10-19",
            valid_from: from,
            valid_until: until,
            model: model.into(),
            period: if peak { "peak" } else { "off_peak" },
            input_per_million: rates[0] as f64 / 1000.0,
            cached_input_per_million: rates[1] as f64 / 1000.0,
            output_per_million: rates[2] as f64 / 1000.0,
        }),
        components,
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoneyTotal {
    pub currency: String,
    pub amount: f64,
    pub calls: usize,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub actual: Vec<MoneyTotal>,
    pub estimated: Vec<MoneyTotal>,
    pub unknown_calls: usize,
}
impl Totals {
    pub fn merge(&mut self, other: &Self) {
        self.unknown_calls += other.unknown_calls;
        for (target, source) in [
            (&mut self.actual, &other.actual),
            (&mut self.estimated, &other.estimated),
        ] {
            for row in source {
                if let Some(old) = target.iter_mut().find(|v| v.currency == row.currency) {
                    old.amount += row.amount;
                    old.calls += row.calls;
                } else {
                    target.push(row.clone());
                }
            }
        }
    }
    pub fn add(&mut self, cost: &Cost) {
        let (Some(amount), Some(currency)) = (cost.amount, cost.currency) else {
            self.unknown_calls += 1;
            return;
        };
        let group = if cost.kind == "actual" {
            &mut self.actual
        } else {
            &mut self.estimated
        };
        if let Some(total) = group.iter_mut().find(|t| t.currency == currency) {
            total.amount += amount;
            total.calls += 1;
        } else {
            group.push(MoneyTotal {
                currency: currency.into(),
                amount,
                calls: 1,
            });
        }
    }
}
