//! Account snapshots come from billing sources, never from local transcript token totals.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    pub id: String,
    pub label: String,
    pub used_percent: Option<f64>,
    pub remaining_percent: Option<f64>,
    pub resets_at: Option<i64>,
    pub duration_mins: Option<u64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Money {
    pub label: String,
    pub amount: Option<String>,
    pub currency: String,
    pub kind: String,
    pub period: Option<String>,
    pub resets_at: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetCredits {
    pub available_count: Option<u64>,
    pub details_complete: bool,
    pub expirations: Vec<Option<i64>>,
    pub access_mode: String,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSnapshot {
    pub identity: Option<String>,
    pub plan: Option<String>,
    pub account_type: String,
    pub windows: Vec<QuotaWindow>,
    pub money: Vec<Money>,
    pub reset_credits: ResetCredits,
    pub as_of: String,
    pub valid_for_seconds: u64,
    pub source: String,
    pub notes: Vec<String>,
}
impl AccountSnapshot {
    pub fn new(kind: &str, source: &str) -> Self {
        Self {
            identity: None,
            plan: None,
            account_type: kind.into(),
            windows: vec![],
            money: vec![],
            reset_credits: ResetCredits {
                available_count: None,
                details_complete: false,
                expirations: vec![],
                access_mode: "official".into(),
                reason: "来源未提供券数量；在官方用量入口核查。".into(),
            },
            as_of: chrono::Utc::now().to_rfc3339(),
            valid_for_seconds: 300,
            source: source.into(),
            notes: vec![],
        }
    }
}
fn percentage(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .filter(|n| n.is_finite() && *n >= 0.0 && *n <= 100.0)
}
fn unix(value: &Value) -> Option<i64> {
    value.as_i64().filter(|n| *n > 0 && *n < 32_503_680_000)
}
fn window(id: String, label: String, v: &Value, percent: &str, reset: &str) -> QuotaWindow {
    let used = percentage(&v[percent]);
    QuotaWindow {
        id,
        label,
        used_percent: used,
        remaining_percent: used.map(|n| 100.0 - n),
        resets_at: unix(&v[reset]),
        duration_mins: v["windowDurationMins"].as_u64().filter(|n| *n > 0),
    }
}
pub fn codex(account: &Value, limits: &Value) -> Result<AccountSnapshot, &'static str> {
    if account["type"] != "chatgpt" {
        return Err("当前 Codex 登录不是 ChatGPT 订阅账户");
    }
    let mut result = AccountSnapshot::new("subscription", "Codex 官方账户接口");
    result.identity = account["email"]
        .as_str()
        .map(|s| crate::sessions::sanitize_label(s, 200));
    result.plan = account["planType"]
        .as_str()
        .map(|s| crate::sessions::sanitize_label(s, 80));
    let buckets = if let Some(map) = limits["rateLimitsByLimitId"]
        .as_object()
        .filter(|m| !m.is_empty())
    {
        map.iter()
            .map(|(id, v)| (id.clone(), v))
            .collect::<Vec<_>>()
    } else if limits["rateLimits"].is_object() {
        vec![("codex".into(), &limits["rateLimits"])]
    } else {
        vec![]
    };
    for (id, bucket) in buckets.into_iter().take(32) {
        let name =
            crate::sessions::sanitize_label(bucket["limitName"].as_str().unwrap_or(&id), 100);
        let name = if name == "codex" {
            "Codex".to_string()
        } else {
            name
        };
        for key in ["primary", "secondary"] {
            let v = &bucket[key];
            if !v.is_object() {
                continue;
            }
            let duration = v["windowDurationMins"]
                .as_u64()
                .map(|m| {
                    if m % 1440 == 0 {
                        format!("{} 天窗口", m / 1440)
                    } else if m % 60 == 0 {
                        format!("{} 小时窗口", m / 60)
                    } else {
                        format!("{m} 分钟窗口")
                    }
                })
                .unwrap_or_else(|| {
                    if key == "primary" {
                        "主要窗口"
                    } else {
                        "附加窗口"
                    }
                    .into()
                });
            result.windows.push(window(
                format!("{id}:{key}"),
                format!("{name} · {duration}"),
                v,
                "usedPercent",
                "resetsAt",
            ));
        }
    }
    let credits = &limits["rateLimitResetCredits"];
    result.reset_credits.available_count = credits["availableCount"].as_u64();
    let rows = credits["credits"].as_array();
    result.reset_credits.details_complete =
        rows.is_some_and(|r| Some(r.len() as u64) == result.reset_credits.available_count);
    result.reset_credits.expirations = rows
        .map(|r| r.iter().take(100).map(|r| unix(&r["expiresAt"])).collect())
        .unwrap_or_default();
    result.reset_credits.reason="可用数量以官方返回的总数为准。使用请前往官方用量页或 Codex /usage；账户额度与其他客户端共享。".into();
    result
        .notes
        .push("续费日、套餐周期与绝对剩余 Token 未由该接口提供。".into());
    Ok(result)
}
pub fn claude_statusline(v: &Value) -> Result<AccountSnapshot, &'static str> {
    let mut result =
        AccountSnapshot::new("subscription", "Claude Code 官方 statusLine · rate_limits");
    for (key, label) in [("five_hour", "5 小时窗口"), ("seven_day", "7 天窗口")] {
        if v["rate_limits"][key].is_object() {
            let mut w = window(
                key.into(),
                label.into(),
                &v["rate_limits"][key],
                "used_percentage",
                "resets_at",
            );
            w.duration_mins = Some(if key == "five_hour" { 300 } else { 10080 });
            result.windows.push(w);
        }
    }
    if result.windows.is_empty() {
        return Err("Claude 未提供订阅额度；API / 第三方会话可能没有此字段");
    }
    result.notes.push("来源是额度采集入口启动的 Claude 会话。官方状态栏未提供账户身份、Plan、续费日或重置券数量；不能归给其他账户或累计汇总。".into());
    Ok(result)
}
// Preserve service-provided decimal strings; don't round money via local token-price estimates.
fn amount(v: &Value) -> Option<String> {
    let text = if let Some(s) = v.as_str() {
        s.to_string()
    } else if v.is_number() {
        v.to_string()
    } else {
        return None;
    };
    let raw = text.strip_prefix('-').unwrap_or(&text);
    let parts = raw.split('.').collect::<Vec<_>>();
    (text.len() <= 48
        && parts.len() <= 2
        && parts
            .iter()
            .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())))
    .then_some(text)
}
fn money(label: &str, v: &Value, currency: &str, kind: &str, period: Option<&str>) -> Money {
    Money {
        label: label.into(),
        amount: amount(v),
        currency: currency.into(),
        kind: kind.into(),
        period: period.map(str::to_owned),
        resets_at: None,
    }
}
pub fn api(adapter: &str, v: &Value) -> Result<AccountSnapshot, &'static str> {
    let name = match adapter {
        "deepseek" => "DeepSeek",
        "openrouter" => "OpenRouter",
        "kimi-cn" => "Kimi 中国",
        "kimi-global" => "Kimi 国际",
        _ => adapter,
    };
    let mut s = AccountSnapshot::new("api", &format!("{name} 官方账户接口"));
    match adapter {
        "deepseek" => {
            let rows = v["balance_infos"]
                .as_array()
                .filter(|a| !a.is_empty())
                .ok_or("余额响应结构不完整")?;
            for row in rows.iter().take(8) {
                let c = row["currency"]
                    .as_str()
                    .filter(|c| ["CNY", "USD"].contains(c))
                    .ok_or("币种未知")?;
                s.money
                    .push(money("可用余额", &row["total_balance"], c, "balance", None));
            }
            s.notes
                .push("该接口只提供余额，不提供已用金额或账期；不以充值额倒推消费。".into());
        }
        "kimi-cn" | "kimi-global" => {
            if v["status"] != true || v["code"] != 0 {
                return Err("余额接口未确认成功");
            }
            s.money.push(money(
                "可用余额",
                &v["data"]["available_balance"],
                if adapter == "kimi-cn" { "CNY" } else { "USD" },
                "balance",
                None,
            ));
            s.notes
                .push("余额包含可用赠送金额；已用金额和账期未由此接口提供。".into());
        }
        "openrouter" => {
            let d = &v["data"];
            if !d.is_object() {
                return Err("用量响应结构不完整");
            }
            s.money.push(money(
                "本月已用",
                &d["usage_monthly"],
                "USD",
                "period_spend",
                Some("服务商月度统计"),
            ));
            s.money.push(money(
                "累计已用",
                &d["usage"],
                "USD",
                "lifetime_spend",
                None,
            ));
            s.money.push(money(
                "密钥剩余预算",
                &d["limit_remaining"],
                "USD",
                "budget",
                d["limit_reset"].as_str(),
            ));
            s.notes
                .push("密钥预算不等于账户余额。不同密钥 / 组织范围分别显示，不相加。".into());
            if d["effective_budget"].is_object() {
                let b = &d["effective_budget"];
                let mut m = money(
                    "当前生效的剩余预算",
                    &b["remaining_usd"],
                    "USD",
                    "effective_budget",
                    b["reset_interval"].as_str(),
                );
                m.resets_at = b["resets_at"]
                    .as_str()
                    .filter(|s| chrono::DateTime::parse_from_rfc3339(s).is_ok())
                    .map(str::to_owned);
                s.money.push(m);
            }
        }
        _ => return Err("该服务商尚无已接入的账户查询接口"),
    }
    if !s.money.iter().any(|m| m.amount.is_some()) {
        return Err("服务商未提供可读取的金额");
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn codex_multi_bucket_count_and_unknowns() {
        let s=codex(&json!({"type":"chatgpt","planType":"pro"}),&json!({"rateLimits":{"primary":{"usedPercent":99}},"rateLimitsByLimitId":{"codex":{"primary":{"usedPercent":0,"windowDurationMins":300,"resetsAt":2000000000}},"other":{"secondary":{"usedPercent":35}}},"rateLimitResetCredits":{"availableCount":3,"credits":[{"expiresAt":null}]}})).unwrap();
        assert_eq!(s.windows.len(), 2);
        assert_eq!(s.windows[0].remaining_percent, Some(100.));
        assert!(s.windows[1].resets_at.is_none());
        assert_eq!(s.reset_credits.available_count, Some(3));
        assert!(!s.reset_credits.details_complete);
        let unknown = codex(&json!({"type":"chatgpt"}), &json!({})).unwrap();
        assert!(unknown.reset_credits.available_count.is_none());
        assert!(unknown.plan.is_none());
    }
    #[test]
    fn money_zero_budget_currency_and_missing_spend() {
        let s=api("deepseek",&json!({"balance_infos":[{"currency":"CNY","total_balance":"0.000000"},{"currency":"USD","total_balance":"12.123456"}]})).unwrap();
        assert_eq!(s.money[0].amount.as_deref(), Some("0.000000"));
        assert_eq!(s.money[1].currency, "USD");
        assert!(!s.money.iter().any(|m| m.kind.contains("spend")));
        let s = api(
            "openrouter",
            &json!({"data":{"usage_monthly":0,"usage":1.25,"limit_remaining":null}}),
        )
        .unwrap();
        assert!(s.money[2].amount.is_none());
        assert_eq!(s.money[2].kind, "budget");
        assert!(
            api(
                "kimi-cn",
                &json!({"status":false,"code":1,"data":{"available_balance":100}})
            )
            .is_err()
        );
    }
    #[test]
    fn statusline_sanitizes_and_does_not_assume_plan_or_coupons() {
        let s=claude_statusline(&json!({"apiKey":"never persist","transcript_path":"never persist","rate_limits":{"five_hour":{"used_percentage":40,"resets_at":2000000000},"seven_day":{"used_percentage":null}}})).unwrap();
        assert_eq!(s.windows[0].remaining_percent, Some(60.));
        assert!(s.windows[1].used_percent.is_none());
        assert!(s.plan.is_none());
        assert!(s.reset_credits.available_count.is_none());
        let text = serde_json::to_string(&s).unwrap();
        assert!(!text.contains("never persist"));
        assert!(claude_statusline(&json!({})).is_err());
    }
}
