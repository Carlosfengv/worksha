//! Exact private index metadata. Full-scope queries never decode historical
//! attempt payloads; old public summaries retain their saturating semantics.
use super::*;
use rusqlite::functions::{Aggregate, Context, FunctionFlags};
const FIELDS: [&str; 6] = [
    "inputTokens",
    "outputTokens",
    "cachedInputTokens",
    "cacheWriteInputTokens",
    "reasoningOutputTokens",
    "totalTokens",
];

#[derive(Clone)]
struct Sum {
    values: [Option<u64>; 6],
    overflow: [bool; 6],
    missing_parts: Option<u64>,
}
impl Default for Sum {
    fn default() -> Self {
        Self {
            values: [None; 6],
            overflow: [false; 6],
            missing_parts: Some(0),
        }
    }
}
impl Sum {
    fn add(&mut self, i: usize, value: Option<u64>) {
        if let Some(value) = value
            && !self.overflow[i]
        {
            match self.values[i].unwrap_or(0).checked_add(value) {
                Some(next) => self.values[i] = Some(next),
                None => {
                    self.values[i] = None;
                    self.overflow[i] = true;
                }
            }
        }
    }
    fn observe(&mut self, u: &crate::sessions::Usage) {
        if u.total_tokens.is_none() {
            self.missing_parts = self.missing_parts.and_then(|n| n.checked_add(1));
        }
        for (i, value) in [
            u.input_tokens,
            u.output_tokens,
            u.cached_input_tokens,
            u.cache_write_input_tokens,
            u.reasoning_output_tokens,
            u.total_tokens,
        ]
        .into_iter()
        .enumerate()
        {
            self.add(i, value);
        }
    }
    fn merge(&mut self, other: &Self) {
        for i in 0..6 {
            if other.overflow[i] {
                self.overflow[i] = true;
                self.values[i] = None;
            } else {
                self.add(i, other.values[i]);
            }
        }
        self.missing_parts = self
            .missing_parts
            .zip(other.missing_parts)
            .and_then(|(a, b)| a.checked_add(b));
    }
    fn json(&self) -> String {
        let mut usage = serde_json::Map::new();
        let mut overflow = vec![];
        for (i, name) in FIELDS.iter().enumerate() {
            usage.insert((*name).into(), json!(self.values[i].map(|v| v.to_string())));
            if self.overflow[i] {
                overflow.push(*name);
            }
        }
        if self.missing_parts.is_none() {
            overflow.push("usageMissingParts");
        }
        json!({"usage":usage,"overflowFields":overflow,"missingParts":self.missing_parts.map(|n|n.to_string())}).to_string()
    }
    fn parse(raw: &str) -> rusqlite::Result<Self> {
        type Stored = ([Option<d::Decimal>; 6], u8, Option<d::Decimal>);
        let (values, mask, missing): Stored = serde_json::from_str(raw)
            .map_err(|e| rusqlite::Error::UserFunctionError(Box::new(e)))?;
        Ok(Self {
            values: values.map(|v| v.map(|n| n.value())),
            overflow: std::array::from_fn(|i| mask & (1 << i) != 0),
            missing_parts: missing.map(|n| n.value()),
        })
    }
    fn metadata(&self) -> String {
        let mask = self
            .overflow
            .iter()
            .enumerate()
            .fold(0u8, |mask, (i, yes)| mask | (u8::from(*yes) << i));
        json!([
            self.values.map(|v| v.map(|n| n.to_string())),
            mask,
            self.missing_parts.map(|n| n.to_string())
        ])
        .to_string()
    }
}
#[derive(Default)]
struct Accumulator {
    sum: Sum,
    rows: HashMap<String, Sum>,
}
struct ExactUsage;
impl Aggregate<Accumulator, String> for ExactUsage {
    fn init(&self, _: &mut Context<'_>) -> rusqlite::Result<Accumulator> {
        Ok(Accumulator::default())
    }
    fn step(&self, c: &mut Context<'_>, a: &mut Accumulator) -> rusqlite::Result<()> {
        let raw = c.get_raw(0).as_str()?;
        if raw.len() > 8192 {
            a.sum.merge(&Sum::parse(raw)?);
            return Ok(());
        }
        if let Some(row) = a.rows.get(raw) {
            a.sum.merge(row);
        } else {
            let row = Sum::parse(raw)?;
            a.sum.merge(&row);
            if a.rows.len() == 64 {
                a.rows.clear();
            }
            a.rows.insert(raw.into(), row);
        }
        Ok(())
    }
    fn finalize(&self, _: &mut Context<'_>, a: Option<Accumulator>) -> rusqlite::Result<String> {
        Ok(a.unwrap_or_default().sum.json())
    }
}
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Usage {
    pub usage: d::Usage,
    pub overflow_fields: Vec<String>,
    pub missing_parts: Option<d::Decimal>,
}
pub(super) fn init(db: &Connection) -> Result<(), String> {
    db.create_aggregate_function(
        "domain_request_usage",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        ExactUsage,
    )
    .map_err(fail)
}
pub(super) fn captured(row: &requests::Request) -> String {
    let mut sum = Sum::default();
    if row.source == "gateway" {
        for part in &row.attempts {
            sum.observe(&part.observation.usage);
        }
    } else {
        sum.observe(&row.usage);
    }
    sum.metadata()
}
pub(super) fn summary(db: &Connection, from: &str) -> d::Result<(requests::Summary, Usage)> {
    let (base, raw): (String, String) = db
        .query_row(
            &format!("SELECT summary_sum(stats,cache_in,cache_read),domain_request_usage(domain_usage) FROM {from}"),
            [],
            |r| Ok((r.get(0)?,r.get(1)?)),
        )
        .map_err(unavailable)?;
    Ok((
        serde_json::from_str(&base).map_err(unavailable)?,
        serde_json::from_str(&raw).map_err(unavailable)?,
    ))
}
pub(super) fn record(row: &Value, unit: d::requests::RequestUnit) -> d::Result<Usage> {
    let mut sum = Sum::default();
    if unit == d::requests::RequestUnit::LogicalRequest {
        for part in row["attempts"]
            .as_array()
            .ok_or_else(|| unavailable("missing captured attempts"))?
        {
            let usage: crate::sessions::Usage =
                serde_json::from_value(part["usage"].clone()).map_err(unavailable)?;
            sum.observe(&usage);
        }
    } else {
        let usage: crate::sessions::Usage =
            serde_json::from_value(row["usage"].clone()).map_err(unavailable)?;
        sum.observe(&usage);
    }
    serde_json::from_str(&sum.json()).map_err(unavailable)
}
