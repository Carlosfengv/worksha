use local_agent_core::{
    domain::{self as d, activity::query as a},
    sessions::{
        activity_performance::{ActivitySampling, RateSample},
        live::NativeActivity,
    },
};

fn finite(n: Option<f64>) -> Option<f64> {
    n.filter(|n| n.is_finite() && *n >= 0.0)
}
fn recent(
    sample: Option<&RateSample>,
    start: Option<&str>,
    now: i64,
) -> Option<a::RateObservation> {
    let sample = sample?;
    let start = start.and_then(d::timestamp);
    (sample.valid() && sample.at <= now + 1000 && start.is_none_or(|s| sample.at >= s)).then(|| {
        a::RateObservation {
            at_ms: (sample.at as u64).into(),
            window_ms: u64::from(sample.window_ms).into(),
            tokens_per_second: sample.tokens_per_second,
        }
    })
}
pub(super) fn rate(
    sampling: Option<&ActivitySampling>,
    start: Option<&str>,
    now: i64,
    live: bool,
) -> (Option<a::RateObservation>, Option<a::RateObservation>) {
    let Some(s) = sampling else {
        return (None, None);
    };
    let recent = recent(s.recent.as_ref(), start, now);
    let current = (live
        && s.window_ms == 5000
        && s.at > 0
        && (-1000..=5000).contains(&now.saturating_sub(s.at))
        && recent.as_ref().is_some_and(|r| {
            s.at as u64 >= r.at_ms.value() && (s.at as u64).saturating_sub(r.at_ms.value()) < 5000
        }))
    .then(|| {
        finite(s.tokens_per_second)
            .filter(|n| *n > 0.0)
            .map(|n| a::RateObservation {
                at_ms: (s.at as u64).into(),
                window_ms: u64::from(s.window_ms).into(),
                tokens_per_second: n,
            })
    })
    .flatten();
    (current, recent)
}
pub(super) fn native(row: &NativeActivity, now: i64, current: bool) -> a::ActivityMeasurements {
    let runtime = row.realtime.as_ref();
    let (live_rate, recent_rate) = rate(
        runtime.and_then(|r| r.sampling.as_ref()),
        row.message_started_at.as_deref(),
        now,
        current && runtime.is_some_and(|r| r.state == "connected"),
    );
    let usage = row
        .message_usage
        .as_ref()
        .or_else(|| {
            runtime
                .and_then(|r| r.sampling.as_ref())
                .and_then(|s| s.reported.as_ref())
        })
        .filter(|u| {
            u.valid()
                && d::timestamp(&u.at).is_some_and(|at| {
                    at <= now + 1000
                        && row
                            .message_started_at
                            .as_deref()
                            .and_then(d::timestamp)
                            .is_none_or(|s| at >= s)
                })
        })
        .map(|u| d::Usage {
            input_tokens: None,
            output_tokens: Some(u.output_tokens.into()),
            cached_input_tokens: None,
            cache_write_input_tokens: None,
            reasoning_output_tokens: u.reasoning_tokens.map(Into::into),
            total_tokens: None,
        });
    a::ActivityMeasurements {
        reported_ttft_ms: finite(row.turn_ttft_ms),
        runtime_state: runtime.map(|r| r.state.clone()),
        first_token_ms: finite(runtime.and_then(|r| r.first_token_ms)),
        first_text_ms: finite(runtime.and_then(|r| r.first_text_ms)),
        first_output_ms: finite(runtime.and_then(|r| r.first_output_ms)),
        usage,
        live_rate,
        recent_rate,
        historical_average_text_rate: finite(
            runtime.and_then(|r| r.average_text_tokens_per_second),
        ),
    }
}
pub(super) fn gateway(
    row: &crate::observation::GatewayActivity,
    now: i64,
    current: bool,
) -> a::ActivityMeasurements {
    let (live_rate, recent_rate) = rate(
        Some(&row.sampling),
        Some(&row.request.started_at),
        now,
        current
            && ["streaming", "public_reasoning", "tool_arguments"].contains(&row.phase.as_str()),
    );
    let timings = row.request.timing_issue.is_none();
    a::ActivityMeasurements {
        reported_ttft_ms: None,
        runtime_state: None,
        first_token_ms: timings.then(|| finite(row.request.ttft_ms)).flatten(),
        first_text_ms: timings.then(|| finite(row.request.first_text_ms)).flatten(),
        first_output_ms: None,
        usage: Some((&row.request.usage).into()),
        live_rate,
        recent_rate,
        historical_average_text_rate: finite(row.average_text_tokens_per_second),
    }
}
