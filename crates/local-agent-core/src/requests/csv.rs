//! The same CSV encoder serves frozen SQL exports and in-memory reference tests.
use serde_json::Value;
pub(crate) fn header() -> String {
    "\u{feff}请求,时间,Agent,模型,TTFT 秒,输出吞吐 Token/s,状态,Provider,总耗时 秒,输入 Token,输出 Token,缓存读取 Token,缓存写入 Token,尝试次数,实际费用,估算费用,费用未知次数（网关尝试/原生响应）,Session,来源,用量缺失数（网关尝试/原生响应）,暂定,记录类型,原生记录ID,响应ID,跨源关联,时间口径,局域网客户端ID,局域网客户端名称,客户端连接地址\r\n".into()
}
pub(crate) fn write(out: &mut String, r: &Value) {
    write_bounded(out, r, usize::MAX).expect("unbounded CSV string encoding");
}
pub(crate) fn write_bounded(
    out: &mut String,
    r: &Value,
    limit: usize,
) -> Result<(), std::fmt::Error> {
    let start = out.len();
    let native = r["native"].is_object();
    let text = |key: &str, unknown: &str| r[key].as_str().unwrap_or(unknown).to_owned();
    let n = |key: &str| {
        r["usage"][key]
            .as_u64()
            .map(|n| n.to_string())
            .unwrap_or("未知".into())
    };
    let f = |key: &str, scale: f64| {
        r[key]
            .as_f64()
            .map(|n| format!("{:.6}", n / scale))
            .unwrap_or("不可测".into())
    };
    let money = |key: &str| {
        let mut value = String::new();
        for (index, m) in r["cost"][key].as_array().into_iter().flatten().enumerate() {
            let currency = m["currency"].as_str().unwrap_or("");
            let amount = format!("{:.9}", m["amount"].as_f64().unwrap_or(0.0));
            let separator = if index == 0 { "" } else { "; " };
            if value
                .len()
                .saturating_add(separator.len())
                .saturating_add(currency.len())
                .saturating_add(amount.len())
                .saturating_add(1)
                > limit
            {
                return Err(std::fmt::Error);
            }
            value.push_str(separator);
            value.push_str(currency);
            value.push(' ');
            value.push_str(&amount);
        }
        Ok(value)
    };
    let attempts = r["attempts"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let links = r["nativeLinks"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let fields = vec![
        text("id", ""),
        super::time(r["startedAt"].as_str().unwrap_or(""))
            .map(|t| {
                t.with_timezone(&chrono::FixedOffset::east_opt(28800).unwrap())
                    .to_rfc3339()
            })
            .unwrap_or("时间未知".into()),
        crate::agents::group(
            r["agentId"].as_str().unwrap_or(""),
            r["agentProductId"].as_str(),
        ),
        text("model", "未知"),
        f("ttftMs", 1000.0),
        f("throughput", 1.0),
        text("status", ""),
        text("provider", ""),
        f("durationMs", 1000.0),
        n("inputTokens"),
        n("outputTokens"),
        n("cachedInputTokens"),
        n("cacheWriteInputTokens"),
        attempts.len().to_string(),
        money("actual")?,
        money("estimated")?,
        r["cost"]["unknownCalls"].as_u64().unwrap_or(0).to_string(),
        text("sessionId", "未知"),
        if native { "原生记录" } else { "网关" }.into(),
        r["usageMissing"].as_u64().unwrap_or(0).to_string(),
        if ["waiting", "streaming"].contains(&r["status"].as_str().unwrap_or("")) {
            "是"
        } else {
            "否"
        }
        .into(),
        if native {
            "模型响应记录"
        } else {
            "逻辑请求"
        }
        .into(),
        if native {
            r["native"]["eventId"].as_str().unwrap_or("").into()
        } else {
            links
                .iter()
                .filter_map(|l| l["eventId"].as_str())
                .collect::<Vec<_>>()
                .join("; ")
        },
        r["native"]["responseId"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| {
                attempts
                    .iter()
                    .filter_map(|a| a["responseId"].as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            }),
        if native {
            r["native"]["association"].as_str().unwrap_or("").into()
        } else if links.is_empty() {
            "未关联".into()
        } else {
            "明确响应ID".into()
        },
        if native {
            "原生记录时间（非请求开始时间）"
        } else {
            "请求开始时间"
        }
        .into(),
        r["client"]["id"].as_str().unwrap_or("").into(),
        r["client"]["name"].as_str().unwrap_or("").into(),
        r["client"]["peer"].as_str().unwrap_or("").into(),
    ];
    for (index, field) in fields.iter().enumerate() {
        let bytes = field
            .len()
            .saturating_add(field.bytes().filter(|b| *b == b'"').count())
            .saturating_add(2 + usize::from(super::csv_dangerous(field)))
            + usize::from(index > 0);
        if bytes > limit.saturating_sub(out.len() - start) {
            out.truncate(start);
            return Err(std::fmt::Error);
        }
        if index > 0 {
            out.push(',');
        }
        out.push_str(&super::csv_cell(field));
    }
    if out.len() - start > limit.saturating_sub(2) {
        out.truncate(start);
        return Err(std::fmt::Error);
    }
    out.push_str("\r\n");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_csv_preserves_money_format_formula_escaping_and_existing_prefix() {
        let value = serde_json::json!({"id":"  =formula(\"x\")","cost":{"actual":[{"currency":"EUR","amount":1.25},{"currency":"USD","amount":0}]}});
        let mut csv = String::from("prefix");
        write_bounded(&mut csv, &value, 4096).unwrap();
        assert!(csv.contains("\"'  =formula(\"\"x\"\")\""));
        assert!(csv.contains("\"EUR 1.250000000; USD 0.000000000\""));
        assert!(csv.ends_with("\r\n"));
        let mut csv = String::from("prefix");
        assert!(write_bounded(&mut csv, &value, 10).is_err());
        assert_eq!(csv, "prefix");
    }
    #[test]
    fn expanded_float_amounts_cannot_allocate_an_unbounded_joined_cell() {
        let value = serde_json::json!({"cost":{"actual":(0..1000).map(|_|serde_json::json!({"currency":"USD","amount":1e308})).collect::<Vec<_>>()}});
        let mut csv = String::from("prefix");
        assert!(write_bounded(&mut csv, &value, 4096).is_err());
        assert_eq!(csv, "prefix");
    }
}
