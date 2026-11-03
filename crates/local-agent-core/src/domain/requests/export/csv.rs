//! Fixed columns, lossless decimal text and bounded CSV rows. The scope row
//! preserves unavailable sources even when the readable record count is zero.
use super::Info;
use crate::domain::{Decimal, requests::query as q};
use crate::requests::export::{EXPORT_TARGET_BYTES, ExportError};
use serde::Serialize;

const COLUMNS: [&str; 37] = [
    "行类型",
    "冻结范围与来源状态(JSON)",
    "记录ID",
    "来源ID",
    "统计单位",
    "证据来源",
    "产品ID",
    "SessionID",
    "会话标题",
    "项目ID",
    "项目名称",
    "来源记录时间",
    "记录结束时间",
    "模型",
    "ProviderKey",
    "Provider名称",
    "记录结果",
    "输入Token",
    "输出Token",
    "总Token",
    "缓存读取Token",
    "缓存写入Token",
    "推理Token",
    "用量缺失记录数",
    "已记录尝试次数",
    "宿主记录耗时ms",
    "宿主记录首出ms",
    "宿主记录Token每秒",
    "来源报告耗时ms",
    "来源报告响应ID",
    "宿主记录ID",
    "冻结关联证据(JSON)",
    "冻结实际费用(JSON)",
    "冻结估算费用(JSON)",
    "费用未知部分数",
    "计数溢出字段(JSON)",
    "时间口径说明",
];

pub fn start(info: &Info) -> Result<String, ExportError> {
    info.validate()?;
    let mut csv = String::from("\u{feff}");
    cells(&mut csv, &COLUMNS.map(str::to_owned), EXPORT_TARGET_BYTES)?;
    let mut fields = vec![String::new(); COLUMNS.len()];
    fields[0] = "scope".into();
    fields[1] = encoded(info)?;
    let remaining = EXPORT_TARGET_BYTES.saturating_sub(csv.len());
    cells(&mut csv, &fields, remaining)?;
    Ok(csv)
}

pub fn append(
    csv: &mut String,
    info: &Info,
    item: &q::UnifiedRequestItem,
    limit: usize,
) -> Result<(), ExportError> {
    let r = &item.record;
    let requested = info.applied_query.include_evidence == Some(true);
    let reported = item.evidence == q::RequestEvidence::SourceReported;
    if r.unit != info.applied_query.unit
        || !info.applied_query.scope.source_ids.contains(&r.source_id)
        || !info
            .source_summaries
            .iter()
            .any(|s| s.source_id == r.source_id && s.evidence == item.evidence)
        || item.evidence_detail.is_some() != requested
        || reported
            && (r.unit != crate::domain::requests::RequestUnit::ModelResponse
                || r.attempts.is_some()
                || r.duration_ms.is_some()
                || r.ttft_ms.is_some()
                || r.throughput.is_some()
                || item.host_request_id.is_some()
                || item.evidence_detail.as_ref().is_some_and(|e| {
                    e.matching != q::RequestMatching::SourceReported
                        || e.billing.is_some()
                        || !e.overflow_fields.is_empty()
                        || e.timing_note.is_some()
                }))
    {
        return Err(ExportError::Unavailable);
    }
    let evidence = item.evidence_detail.as_ref();
    let billing = evidence.and_then(|e| e.billing.as_ref());
    let not_requested = "未请求证据";
    let fields = vec![
        "record".into(),
        String::new(),
        r.id.clone(),
        r.source_id.clone(),
        match r.unit {
            crate::domain::requests::RequestUnit::ModelResponse => "model-response",
            crate::domain::requests::RequestUnit::LogicalRequest => "logical-request",
        }
        .into(),
        if reported {
            "source-reported"
        } else {
            "host-observed"
        }
        .into(),
        text(r.product_id.as_ref()),
        text(r.session_ref.as_ref()),
        text(r.session_title.as_ref()),
        text(r.project_ref.as_ref()),
        text(r.project_name.as_ref()),
        text(r.occurred_at.as_ref()),
        text(r.ended_at.as_ref()),
        text(r.model.as_ref()),
        r.provider_key.clone(),
        r.provider_name.clone(),
        r.outcome.clone(),
        decimal(r.usage.input_tokens.as_ref()),
        decimal(r.usage.output_tokens.as_ref()),
        decimal(r.usage.total_tokens.as_ref()),
        decimal(r.usage.cached_input_tokens.as_ref()),
        decimal(r.usage.cache_write_input_tokens.as_ref()),
        decimal(r.usage.reasoning_output_tokens.as_ref()),
        r.usage_missing.value().to_string(),
        decimal(r.attempts.as_ref()),
        finite(r.duration_ms)?,
        finite(r.ttft_ms)?,
        finite(r.throughput)?,
        decimal(item.reported_duration_ms.as_ref()),
        text(item.reported_response_id.as_ref()),
        text(item.host_request_id.as_ref()),
        evidence
            .map(|e| encoded(&e.matching))
            .transpose()?
            .unwrap_or_else(|| not_requested.into()),
        billing
            .map(|b| encoded(&b.actual))
            .transpose()?
            .unwrap_or_else(|| "未知".into()),
        billing
            .map(|b| encoded(&b.estimated))
            .transpose()?
            .unwrap_or_else(|| "未知".into()),
        billing
            .map(|b| b.unknown_parts.value().to_string())
            .unwrap_or_else(|| "未知".into()),
        evidence
            .map(|e| encoded(&e.overflow_fields))
            .transpose()?
            .unwrap_or_else(|| not_requested.into()),
        evidence
            .and_then(|e| e.timing_note.clone())
            .unwrap_or_else(|| {
                if reported {
                    "来源报告；不建立宿主计时、尝试或精确关联证据".into()
                } else {
                    "沿用冻结查询中的宿主记录口径".into()
                }
            }),
    ];
    cells(csv, &fields, limit)
}

fn text(value: Option<&String>) -> String {
    value.cloned().unwrap_or_else(|| "未知".into())
}
fn decimal(value: Option<&Decimal>) -> String {
    value
        .map(|v| v.value().to_string())
        .unwrap_or_else(|| "未知".into())
}
fn finite(value: Option<f64>) -> Result<String, ExportError> {
    match value {
        Some(v) if v.is_finite() && v >= 0.0 => Ok(v.to_string()),
        Some(_) => Err(ExportError::Unavailable),
        None => Ok("未知".into()),
    }
}
fn encoded(value: &impl Serialize) -> Result<String, ExportError> {
    serde_json::to_string(value).map_err(|_| ExportError::Unavailable)
}
fn guarded(value: &str) -> bool {
    let first = value
        .trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '\u{feff}' | '\u{200b}'));
    first.starts_with(['=', '+', '-', '@', '＝', '＋', '－', '＠'])
        || value.starts_with(['\t', '\r', '\n'])
}
fn cells(out: &mut String, fields: &[String], limit: usize) -> Result<(), ExportError> {
    if fields.len() != COLUMNS.len() {
        return Err(ExportError::Unavailable);
    }
    let mut size = 2usize;
    for (index, value) in fields.iter().enumerate() {
        size = size
            .checked_add(value.len())
            .and_then(|n| n.checked_add(value.bytes().filter(|b| *b == b'"').count()))
            .and_then(|n| {
                n.checked_add(2 + usize::from(index > 0) + if guarded(value) { 2 } else { 0 })
            })
            .ok_or(ExportError::RecordTooLarge)?;
    }
    if size > limit {
        return Err(ExportError::RecordTooLarge);
    }
    for (index, value) in fields.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push('"');
        // Text and tab guards protect untrusted formula starters, including
        // full-width variants; ordinary decimal cells remain unchanged.
        if guarded(value) {
            out.push_str("'\t");
        }
        for c in value.chars() {
            if c == '"' {
                out.push('"');
            }
            out.push(c);
        }
        out.push('"');
    }
    out.push_str("\r\n");
    Ok(())
}

#[cfg(test)]
mod tests;
