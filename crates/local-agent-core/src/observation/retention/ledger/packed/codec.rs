use super::*;
use flate2::{Compression, Decompress, FlushDecompress, Status, write::ZlibEncoder};
use std::io::Write;

pub(super) const MAX_RAW: usize = 8 * 1024 * 1024;
pub(super) const MAX_COMPRESSED: usize = MAX_RAW + 65536;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Frame {
    pub version: u16,
    pub store_id: String,
    pub namespace: String,
    pub kind: PackedKind,
    pub entries: Vec<Entry>,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Entry {
    pub order: i64,
    pub key: String,
    pub source: String,
    // Derivable from the exact value; do not persist a second random digest.
    #[serde(skip)]
    pub projection: String,
    // This is staging location metadata, not a statistical dimension or receipt.
    #[serde(skip)]
    pub locator: Option<String>,
    // Preserve exact JSON numbers/decimal text rather than roundtripping f64.
    pub value: String,
}

pub(super) fn bytes_hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
pub(super) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub(super) fn unhex(value: &str) -> Result<Vec<u8>, LedgerError> {
    if !valid_digest(value) {
        return Err(LedgerError::InvalidRecord);
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|v| {
            std::str::from_utf8(v)
                .map_err(invalid)
                .and_then(|s| u8::from_str_radix(s, 16).map_err(invalid))
        })
        .collect()
}
pub(super) fn encode(frame: &Frame) -> Result<(Vec<u8>, Vec<u8>), LedgerError> {
    let raw = serde_json::to_vec(frame).map_err(invalid)?;
    if raw.len() > MAX_RAW {
        return Err(LedgerError::BatchLimit);
    }
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(&raw).map_err(unavailable)?;
    let payload = encoder.finish().map_err(unavailable)?;
    if payload.len() > MAX_COMPRESSED {
        return Err(LedgerError::BatchLimit);
    }
    Ok((raw, payload))
}

pub(super) fn decode(payload: &[u8], raw_bytes: usize) -> Result<Vec<u8>, LedgerError> {
    if raw_bytes == 0 || raw_bytes > MAX_RAW || payload.is_empty() || payload.len() > MAX_COMPRESSED
    {
        return Err(LedgerError::InvalidRecord);
    }
    let mut decoder = Decompress::new(true);
    // Require a verified stream end, including its checksum, inside a fixed
    // output buffer. Truncation, concatenation and expansion bombs fail closed.
    let mut raw = vec![0; raw_bytes + 1];
    let status = decoder
        .decompress(payload, &mut raw, FlushDecompress::Finish)
        .map_err(invalid)?;
    if status != Status::StreamEnd
        || decoder.total_out() != raw_bytes as u64
        || decoder.total_in() != payload.len() as u64
    {
        return Err(LedgerError::InvalidRecord);
    }
    raw.truncate(raw_bytes);
    Ok(raw)
}
