use crate::backend::{ConnectionContext, ExportKind};
use local_agent_core::domain::requests::export as x;
use local_agent_core::requests::export::{
    BeginExport, EXPORT_RECORD_BYTES, EXPORT_ROWS, EXPORT_TARGET_BYTES, ExportChunk, ExportInfo,
    FinishExport,
};
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
#[cfg(test)]
mod tests;

#[derive(PartialEq)]
enum Receipt {
    Legacy(ExportInfo),
    Unified(Box<x::Info>),
}
impl Receipt {
    fn decode(kind: ExportKind, value: serde_json::Value) -> Result<Self, String> {
        match kind {
            ExportKind::Legacy => serde_json::from_value(value)
                .map(Self::Legacy)
                .map_err(|_| "导出任务回复无效".into()),
            ExportKind::Unified => {
                let info: x::Info =
                    serde_json::from_value(value).map_err(|_| "统一导出任务回复无效")?;
                info.validate().map_err(|_| "统一导出范围无效")?;
                Ok(Self::Unified(Box::new(info)))
            }
        }
    }
    fn identity(&self) -> (&str, &str, u64) {
        match self {
            Self::Legacy(i) => (&i.operation_id, &i.snapshot, i.count),
            Self::Unified(i) => (&i.operation_id, &i.snapshot_token, i.count.value()),
        }
    }
    fn saved(self, target: &std::path::Path) -> serde_json::Value {
        let name = target.file_name().map(|s| s.to_string_lossy().to_string());
        match self {
            Self::Legacy(i) => {
                serde_json::json!({"status":"saved","count":i.count,"fileName":name})
            }
            Self::Unified(info) => serde_json::json!(x::SaveReply::Saved {
                info,
                file_name: name
            }),
        }
    }
}

pub(super) async fn export_to(
    context: &ConnectionContext,
    snapshot: String,
    target: PathBuf,
) -> Result<serde_json::Value, String> {
    export(context, snapshot, target, ExportKind::Legacy).await
}
pub(super) async fn export_unified_to(
    context: &ConnectionContext,
    snapshot: String,
    target: PathBuf,
) -> Result<serde_json::Value, String> {
    export(context, snapshot, target, ExportKind::Unified).await
}
async fn export(
    context: &ConnectionContext,
    snapshot: String,
    target: PathBuf,
    kind: ExportKind,
) -> Result<serde_json::Value, String> {
    let (temporary, info) = prepare(context, snapshot, target.clone(), kind).await.map_err(|error| {
        let detail = serde_json::from_str::<serde_json::Value>(&error).ok();
        let message = detail.as_ref().and_then(|v|v["message"].as_str()).unwrap_or(&error);
        serde_json::json!({"code":"export_not_saved","outcomeUnknown":false,"message":format!("导出未保存：{message}")}).to_string()
    })?;
    tokio::task::spawn_blocking(move || {
        temporary.persist(&target).map_err(|_|"无法保存导出文件，请检查目录权限并核查目标文件")?;
        Ok(info.saved(&target))
    }).await.map_err(|_| serde_json::json!({"code":"export_result_unknown","outcomeUnknown":true,"message":"导出文件提交结果未确认，请先核查目标文件"}).to_string())?
}

async fn prepare(
    context: &ConnectionContext,
    snapshot: String,
    target: PathBuf,
    kind: ExportKind,
) -> Result<(tempfile::NamedTempFile, Receipt), String> {
    context.ensure_current()?;
    let operation = uuid::Uuid::new_v4().to_string();
    let mut cleanup = match kind {
        ExportKind::Legacy => context.export_cleanup(operation.clone()),
        ExportKind::Unified => context.request_export_cleanup(operation.clone(), kind),
    };
    let begin = match kind {
        ExportKind::Legacy => serde_json::to_value(BeginExport {
            operation_id: operation.clone(),
            snapshot: snapshot.clone(),
        }),
        ExportKind::Unified => serde_json::to_value(x::Begin {
            operation_id: operation.clone(),
            snapshot_token: snapshot.clone(),
        }),
    }
    .map_err(|_| "导出参数无效")?;
    let created = context
        .request(reqwest::Method::POST, kind.path(), Some(begin))
        .await?;
    let info = Receipt::decode(kind, created)?;
    let (created_operation, created_snapshot, count) = info.identity();
    if created_operation != operation || created_snapshot != snapshot || count > i64::MAX as u64 {
        return Err("导出任务与请求不一致".into());
    }
    let path = target.clone();
    let (temporary, file) = tokio::task::spawn_blocking(move || {
        let parent = path.parent().ok_or("导出位置无效")?;
        let temporary = tempfile::Builder::new()
            .prefix(".requests-")
            .suffix(".tmp")
            .tempfile_in(parent)
            .map_err(|_| "无法创建导出文件，请检查目录权限与磁盘空间")?;
        let file = temporary
            .as_file()
            .try_clone()
            .map_err(|_| "无法打开导出临时文件")?;
        Ok::<_, String>((temporary, file))
    })
    .await
    .map_err(|_| "导出文件创建未完成")??;
    let mut file = tokio::fs::File::from_std(file);
    let mut after = -1i64;
    let mut rows = 0u64;
    loop {
        let path = match kind {
            ExportKind::Legacy => format!("{}/{operation}?after={after}", kind.path()),
            ExportKind::Unified => format!("{}/{operation}?offset={rows}", kind.path()),
        };
        let value = context.request(reqwest::Method::GET, &path, None).await?;
        let chunk = match kind {
            ExportKind::Legacy => serde_json::from_value(value).map_err(|_| "导出批次回复无效")?,
            ExportKind::Unified => {
                let part: x::Chunk =
                    serde_json::from_value(value).map_err(|_| "统一导出批次回复无效")?;
                let previous = |offset: u64| {
                    i64::try_from(offset)
                        .map(|n| n - 1)
                        .map_err(|_| "统一导出位置无效")
                };
                ExportChunk {
                    operation_id: part.operation_id,
                    after: previous(part.offset.value())?,
                    next_after: part.next_offset.map(|n| previous(n.value())).transpose()?,
                    rows: part.rows.value(),
                    csv: part.csv,
                }
            }
        };
        let next_rows = rows.checked_add(chunk.rows).ok_or("导出数量无效")?;
        let end = after
            .checked_add(i64::try_from(chunk.rows).map_err(|_| "导出数量无效")?)
            .ok_or("导出位置无效")?;
        if chunk.operation_id != operation
            || chunk.after != after
            || chunk.rows > EXPORT_ROWS as u64
            || (chunk.csv.is_empty() && (after == -1 || chunk.rows > 0))
            || next_rows > count
            || chunk.csv.len() > EXPORT_RECORD_BYTES * 2 + EXPORT_TARGET_BYTES
            || chunk
                .next_after
                .is_some_and(|next| next != end || chunk.rows == 0)
            || (chunk.next_after.is_none() && next_rows != count)
            || (chunk.next_after.is_some() && next_rows == count)
        {
            return Err("导出批次不完整或顺序无效，未保存目标文件".into());
        }
        file.write_all(chunk.csv.as_bytes())
            .await
            .map_err(|_| "请求导出写入失败，请检查磁盘空间")?;
        rows = next_rows;
        after = end;
        if chunk.next_after.is_none() {
            break;
        }
    }
    file.sync_all()
        .await
        .map_err(|_| "请求导出写入失败，请检查磁盘空间")?;
    drop(file);
    // This final authorization check commits permission before publishing the
    // file. A revoke acknowledged before this point rejects the export.
    let finish = match kind {
        ExportKind::Legacy => serde_json::to_value(FinishExport { after, rows }),
        ExportKind::Unified => serde_json::to_value(x::Finish {
            next_offset: rows.into(),
            rows: rows.into(),
        }),
    }
    .map_err(|_| "导出确认无效")?;
    let receipt = context
        .request(
            reqwest::Method::POST,
            &format!("{}/{operation}/finish", kind.path()),
            Some(finish),
        )
        .await?;
    let confirmed = Receipt::decode(kind, receipt)?;
    if confirmed != info {
        return Err("导出确认与原快照不一致，未保存目标文件".into());
    }
    cleanup.completed();
    Ok((temporary, info))
}
