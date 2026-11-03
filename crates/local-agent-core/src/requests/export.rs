//! Bounded, temporary export resources; no destination paths cross the service boundary.
use serde::{Deserialize, Serialize};

pub const EXPORT_ROWS: usize = 128;
pub const EXPORT_TARGET_BYTES: usize = 64 * 1024;
pub const EXPORT_RECORD_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportError {
    Invalid,
    Busy,
    Stale,
    RecordTooLarge,
    Unavailable,
}
impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Invalid => "导出参数无效",
            Self::Busy => "已有两个导出任务，请稍后重试",
            Self::Stale => "请求导出已过期或失效，请刷新后重试",
            Self::RecordTooLarge => "单条记录或 CSV 编码超过导出容量限制，未截断或保存文件",
            Self::Unavailable => "请求导出暂不可用，原始记录保持不变",
        })
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExportInfo {
    pub operation_id: String,
    pub snapshot: String,
    pub count: u64,
    pub as_of: String,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportChunk {
    pub operation_id: String,
    pub after: i64,
    pub next_after: Option<i64>,
    pub rows: u64,
    pub csv: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BeginExport {
    pub operation_id: String,
    pub snapshot: String,
}
#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FinishExport {
    pub after: i64,
    pub rows: u64,
}
