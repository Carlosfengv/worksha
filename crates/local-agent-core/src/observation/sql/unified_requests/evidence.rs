//! Host evidence is projected from the exact immutable record, never from a
//! reported response ID or a source duration. Money keeps its recorded currency.
use super::*;
const LINK_LIMIT: usize = 40;

pub(super) fn billing(value: &crate::provider_cost::Totals) -> d::Result<q::RequestBilling> {
    let convert = |rows: &[crate::provider_cost::MoneyTotal]| -> d::Result<Vec<q::RequestMoney>> {
        rows.iter()
            .map(|row| {
                if !row.amount.is_finite()
                    || row.amount < 0.0
                    || row.currency.is_empty()
                    || row.currency.len() > 16
                    || !row.currency.as_bytes()[0].is_ascii_uppercase()
                    || !row
                        .currency
                        .bytes()
                        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
                {
                    return Err(unavailable("invalid recorded money"));
                }
                Ok(q::RequestMoney {
                    currency: row.currency.clone(),
                    amount: if row.amount == 0.0 {
                        "0".into()
                    } else {
                        row.amount.to_string()
                    },
                    parts: (row.calls as u64).into(),
                })
            })
            .collect()
    };
    Ok(q::RequestBilling {
        actual: convert(&value.actual)?,
        estimated: convert(&value.estimated)?,
        unknown_parts: (value.unknown_calls as u64).into(),
    })
}
pub(super) fn detail(
    row: &Value,
    unit: d::requests::RequestUnit,
    overflow_fields: Vec<String>,
) -> d::Result<q::RequestEvidenceDetail> {
    let matching = if unit == d::requests::RequestUnit::ModelResponse {
        let native = &row["native"];
        let text = |key: &str| native[key].as_str().map(String::from);
        let state = match text("association").as_deref() {
            Some("response_id") => q::NativeMatchState::ExactResponseId,
            Some("unlinked") => q::NativeMatchState::Unlinked,
            Some("ambiguous") => q::NativeMatchState::Ambiguous,
            Some("identity_conflict") => q::NativeMatchState::IdentityConflict,
            _ => q::NativeMatchState::Unknown,
        };
        let response_id = text("responseId");
        let logical_request_id = text("gatewayRequestId");
        let attempt_id = text("matchedAttemptId");
        if state == q::NativeMatchState::ExactResponseId
            && [&response_id, &logical_request_id, &attempt_id]
                .iter()
                .any(|v| v.as_ref().is_none_or(String::is_empty))
        {
            return Err(unavailable("incomplete exact match evidence"));
        }
        q::RequestMatching::NativeResponse {
            state,
            response_id,
            logical_request_id,
            attempt_id,
            usage_conflict: native["usageConflict"].as_bool().unwrap_or(false),
        }
    } else {
        let raw = row["nativeLinks"]
            .as_array()
            .ok_or_else(|| unavailable("invalid exact links"))?;
        let links = raw
            .iter()
            .take(LINK_LIMIT)
            .map(|link| {
                serde_json::from_value::<q::ExactResponseLink>(link.clone()).map_err(unavailable)
            })
            .collect::<d::Result<_>>()?;
        q::RequestMatching::GatewayRequest {
            matched_responses: (raw.len() as u64).into(),
            links,
            links_limited: raw.len() > LINK_LIMIT,
        }
    };
    let totals = serde_json::from_value(row["cost"].clone()).map_err(unavailable)?;
    Ok(q::RequestEvidenceDetail {
        matching,
        billing: Some(billing(&totals)?),
        overflow_fields,
        timing_note: row["timingNote"].as_str().map(String::from),
    })
}
pub(super) fn summary(
    db: &Connection,
    from: &str,
    unit: d::requests::RequestUnit,
    summary: &requests::Summary,
    overflow_fields: Vec<String>,
    usage_missing_parts: Option<d::Decimal>,
) -> d::Result<q::RequestEvidenceSummary> {
    let matching = if unit == d::requests::RequestUnit::ModelResponse {
        let sql = format!(
            "SELECT count(*) FILTER(WHERE association='response_id'),count(*) FILTER(WHERE association='unlinked'),count(*) FILTER(WHERE association='ambiguous'),count(*) FILTER(WHERE association='identity_conflict'),count(*) FILTER(WHERE association IS NULL OR association NOT IN ('response_id','unlinked','ambiguous','identity_conflict')),count(*) FILTER(WHERE usage_conflict=1) FROM {from}"
        );
        let counts = db
            .query_row(&sql, [], |r| {
                Ok([
                    sql_count(r, 0)?,
                    sql_count(r, 1)?,
                    sql_count(r, 2)?,
                    sql_count(r, 3)?,
                    sql_count(r, 4)?,
                    sql_count(r, 5)?,
                ])
            })
            .map_err(unavailable)?
            .map(|v| (v as u64).into());
        let [
            exact,
            unlinked,
            ambiguous,
            identity_conflicts,
            unknown,
            usage_conflicts,
        ] = counts;
        q::RequestMatchingSummary::NativeResponses {
            exact,
            unlinked,
            ambiguous,
            identity_conflicts,
            unknown,
            usage_conflicts,
        }
    } else {
        let with = db
            .query_row(
                &format!("SELECT count(*) FROM {from} r WHERE r.linked=1"),
                [],
                |r| sql_count(r, 0),
            )
            .map_err(unavailable)?;
        let (links,conflicts) = db.query_row(&format!("SELECT coalesce(sum(domain_link_count),0),coalesce(sum(domain_link_conflicts),0) FROM {from}"),[],|r|Ok((sql_count(r,0)?,sql_count(r,1)?))).map_err(unavailable)?;
        q::RequestMatchingSummary::GatewayRequests {
            with_exact_links: (with as u64).into(),
            without_confirmed_links: (summary.requests.saturating_sub(with) as u64).into(),
            matched_responses: (links as u64).into(),
            usage_conflict_links: (conflicts as u64).into(),
        }
    };
    Ok(q::RequestEvidenceSummary {
        matching,
        billing: Some(billing(&summary.cost)?),
        usage_missing_parts,
        overflow_fields,
    })
}
pub(super) fn reported(summary: &q::RequestSourceSummary) -> q::RequestEvidenceSummary {
    q::RequestEvidenceSummary {
        matching: q::RequestMatchingSummary::SourceReported {
            unverified_records: summary.records.clone(),
        },
        billing: None,
        usage_missing_parts: summary.usage_missing.clone(),
        overflow_fields: vec![],
    }
}
