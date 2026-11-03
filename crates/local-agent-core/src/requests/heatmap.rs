use super::*;
use chrono::{Datelike, FixedOffset};
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Day {
    pub date: String,
    pub start: String,
    pub end: String,
    pub state: &'static str,
    pub records: usize,
    pub tokens: Option<u64>,
    pub cost: provider_cost::Totals,
    pub usage_missing: usize,
}
#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Heatmap {
    pub days: Vec<Day>,
    pub missing_time: usize,
    pub outside_window: usize,
}
impl Heatmap {
    pub fn new(rows: &[Request], now: DateTime<Utc>, query: &Query) -> Self {
        let tz = FixedOffset::east_opt(8 * 3600).unwrap();
        let today = now.with_timezone(&tz).date_naive();
        let monday = today - Duration::days(today.weekday().num_days_from_monday().into());
        let start = monday - Duration::weeks(52);
        let mut grouped = BTreeMap::<String, Vec<&Request>>::new();
        let mut missing_time = 0;
        let mut outside_window = 0;
        for r in rows {
            if let Some(t) = time(&r.started_at) {
                let day = t.with_timezone(&tz).date_naive();
                if day < start {
                    outside_window += 1;
                } else {
                    grouped.entry(day.to_string()).or_default().push(r);
                }
            } else {
                missing_time += 1;
            }
        }
        let days = (0..371)
            .map(|i| {
                let date = start + Duration::days(i);
                let at = date
                    .and_hms_opt(0, 0, 0)
                    .unwrap()
                    .and_local_timezone(tz)
                    .unwrap();
                let end = at + Duration::days(1);
                let summary = Summary::of(
                    grouped
                        .get(&date.to_string())
                        .into_iter()
                        .flatten()
                        .copied(),
                );
                let out = query
                    .start
                    .as_deref()
                    .and_then(time)
                    .is_some_and(|s| end.with_timezone(&Utc) <= s)
                    || query
                        .end
                        .as_deref()
                        .and_then(time)
                        .is_some_and(|e| at.with_timezone(&Utc) >= e);
                Day {
                    date: date.to_string(),
                    start: at.to_rfc3339(),
                    end: end.to_rfc3339(),
                    state: if date > today {
                        "future"
                    } else if out {
                        "out_of_range"
                    } else if summary.requests == 0 {
                        "unrecorded"
                    } else {
                        "recorded"
                    },
                    records: summary.requests,
                    tokens: summary.usage.total_tokens,
                    cost: summary.cost,
                    usage_missing: summary.usage_missing,
                }
            })
            .collect();
        Self {
            days,
            missing_time,
            outside_window,
        }
    }
}
