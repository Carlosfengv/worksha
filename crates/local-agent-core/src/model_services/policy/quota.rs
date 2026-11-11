use super::AccountStrategy;
use crate::billing::{MINIMAX_PLAN_WINDOWS, QuotaWindow};

/// Only a fresh, account-scoped window from the host enters the planner.
#[derive(Clone, Copy, Debug)]
pub struct AccountQuotaHint {
    remaining: f64,
    reset: Option<i64>,
    remaining_per_hour: Option<f64>,
}

impl AccountQuotaHint {
    pub fn from_window(window: &QuotaWindow, now: i64) -> Option<Self> {
        let remaining = window
            .remaining_percent
            .filter(|p| (0.0..=100.0).contains(p))?;
        if window.resets_at.is_some_and(|reset| reset <= now) {
            return None;
        }
        let remaining_per_hour =
            window
                .resets_at
                .zip(window.duration_mins)
                .and_then(|(reset, duration)| {
                    let seconds = duration.checked_mul(60)? as f64;
                    let time_left = reset.checked_sub(now)? as f64;
                    (seconds > 0.0 && time_left <= seconds).then(|| remaining * 3600.0 / time_left)
                });
        Some(Self {
            remaining,
            reset: window.resets_at,
            remaining_per_hour,
        })
    }

    pub fn from_minimax_windows(windows: &[QuotaWindow], now: i64) -> Option<Self> {
        let [interval, weekly] = MINIMAX_PLAN_WINDOWS.map(|id| {
            windows
                .iter()
                .find(|window| window.id == id)
                .and_then(|window| Self::from_window(window, now))
        });
        match (interval, weekly) {
            (Some(interval), Some(weekly)) => Some(Self {
                remaining: interval.remaining.min(weekly.remaining),
                reset: interval.reset.zip(weekly.reset).map(|(a, b)| a.min(b)),
                remaining_per_hour: interval
                    .remaining_per_hour
                    .zip(weekly.remaining_per_hour)
                    .map(|(a, b)| a.min(b)),
            }),
            // One depleted window already exhausts the included allowance.
            // The other window's reset is unknown.
            (Some(known), None) | (None, Some(known)) if known.remaining == 0.0 => Some(Self {
                remaining: 0.0,
                reset: None,
                remaining_per_hour: None,
            }),
            _ => None,
        }
    }

    pub fn from_hard_windows<'a>(
        windows: impl IntoIterator<Item = &'a QuotaWindow>,
        now: i64,
    ) -> Option<Self> {
        let mut current = windows
            .into_iter()
            .filter_map(|window| Self::from_window(window, now));
        let mut result = current.next()?;
        // Independent windows overlap. The tighter allowance limits headroom and pace.
        for window in current {
            result.remaining = result.remaining.min(window.remaining);
            result.reset = result.reset.into_iter().chain(window.reset).min();
            result.remaining_per_hour = result
                .remaining_per_hour
                .into_iter()
                .chain(window.remaining_per_hour)
                .reduce(f64::min);
        }
        Some(result)
    }

    pub fn exhausted(self) -> bool {
        self.remaining == 0.0
    }
}

pub(super) fn order(
    strategy: AccountStrategy,
    candidates: &[(bool, Option<AccountQuotaHint>)],
) -> (Vec<usize>, bool) {
    let mut order: Vec<_> = (0..candidates.len()).collect();
    let available = |index: usize| candidates[index].0;
    order.sort_by_key(|&index| !available(index));
    let count = order.partition_point(|&index| available(index));
    if count == 0 {
        return (order, false);
    }
    let leading = &mut order[..count];
    // A pool falls back as a whole when the available alternatives lack a
    // common metric. Pairwise fallbacks could make the comparator cyclic.
    let comparable = leading.iter().all(|&index| {
        candidates[index].1.is_some_and(|hint| match strategy {
            AccountStrategy::QuotaReset => hint.reset.is_some(),
            AccountStrategy::QuotaPace => hint.remaining_per_hour.is_some(),
            _ => true,
        })
    });
    if comparable {
        leading.sort_by(|&a, &b| {
            let a = candidates[a].1.expect("checked quota");
            let b = candidates[b].1.expect("checked quota");
            let reset = || {
                a.reset
                    .unwrap_or(i64::MAX)
                    .cmp(&b.reset.unwrap_or(i64::MAX))
            };
            let remaining = || b.remaining.total_cmp(&a.remaining);
            match strategy {
                AccountStrategy::QuotaReset => reset().then_with(remaining),
                AccountStrategy::QuotaPace => b
                    .remaining_per_hour
                    .expect("checked pace")
                    .total_cmp(&a.remaining_per_hour.expect("checked pace"))
                    .then_with(remaining)
                    .then_with(reset),
                _ => remaining().then_with(reset),
            }
        });
    }
    (order, comparable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strategies_distinguish_reset_remaining_pace_and_unknown() {
        let window = |remaining, reset, minutes| QuotaWindow {
            id: "key".into(),
            label: "key".into(),
            used_percent: None,
            remaining_percent: Some(remaining),
            resets_at: Some(reset),
            duration_mins: Some(minutes),
        };
        let now = 50;
        let week = 7 * 24 * 60;
        let a = window(70., now + 48 * 3600, week);
        let b = window(40., now + 8 * 3600, week);
        let c = window(1., now + 2 * 3600, week);
        let zero = window(0., 80, 5);
        let known = |w: &QuotaWindow| AccountQuotaHint::from_window(w, now);
        let candidates = [
            (true, known(&a)),
            (true, known(&b)),
            (true, known(&c)),
            (false, known(&zero)),
        ];
        assert_eq!(
            order(AccountStrategy::QuotaReset, &candidates),
            (vec![2, 1, 0, 3], true)
        );
        assert_eq!(
            order(AccountStrategy::QuotaRemaining, &candidates),
            (vec![0, 1, 2, 3], true)
        );
        assert_eq!(
            order(AccountStrategy::QuotaPace, &candidates),
            (vec![1, 0, 2, 3], true)
        );
        let tight = AccountQuotaHint::from_hard_windows([&b, &c], now);
        assert_eq!(
            order(
                AccountStrategy::QuotaPace,
                &[(true, tight), (true, known(&a))]
            ),
            (vec![1, 0], true)
        );
        let no_reset = |w: &QuotaWindow| {
            known(&QuotaWindow {
                resets_at: None,
                duration_mins: None,
                ..w.clone()
            })
        };
        assert_eq!(
            order(
                AccountStrategy::QuotaPace,
                &[
                    (true, no_reset(&b)),
                    (true, no_reset(&a)),
                    (true, known(&c))
                ],
            ),
            (vec![0, 1, 2], false)
        );
        let missing = [(true, known(&c)), (true, None), (true, known(&a))];
        for strategy in [
            AccountStrategy::QuotaReset,
            AccountStrategy::QuotaRemaining,
            AccountStrategy::QuotaPace,
        ] {
            assert_eq!(order(strategy, &missing), (vec![0, 1, 2], false));
        }
        let unavailable = [(true, known(&c)), (false, None), (true, known(&a))];
        assert_eq!(
            order(AccountStrategy::QuotaRemaining, &unavailable),
            (vec![2, 0, 1], true)
        );
        let missing_reset = [(true, no_reset(&b)), (true, known(&a))];
        assert_eq!(
            order(AccountStrategy::QuotaReset, &missing_reset),
            (vec![0, 1], false)
        );
        assert_eq!(
            order(AccountStrategy::QuotaRemaining, &missing_reset),
            (vec![1, 0], true)
        );
        assert!(known(&zero).unwrap().exhausted());
        assert!(AccountQuotaHint::from_window(&zero, 80).is_none());
        let included = AccountQuotaHint::from_minimax_windows(
            &[
                QuotaWindow {
                    id: MINIMAX_PLAN_WINDOWS[0].into(),
                    ..a
                },
                QuotaWindow {
                    id: MINIMAX_PLAN_WINDOWS[1].into(),
                    ..zero
                },
            ],
            50,
        )
        .unwrap();
        assert!(included.exhausted());
        assert_eq!(included.remaining, 0.0);
        assert_eq!(
            order(
                AccountStrategy::QuotaRemaining,
                &[(false, Some(included)), (true, None), (true, known(&b))]
            ),
            (vec![1, 2, 0], false)
        );
    }

    #[test]
    fn minimax_requires_both_windows_for_positive_and_timing_evidence() {
        let window = |id: &str, remaining, reset, duration| QuotaWindow {
            id: id.into(),
            label: id.into(),
            used_percent: None,
            remaining_percent: Some(remaining),
            resets_at: reset,
            duration_mins: duration,
        };
        let now = 3600;
        let interval = window(MINIMAX_PLAN_WINDOWS[0], 30.0, Some(now + 3600), Some(300));
        let weekly = window(
            MINIMAX_PLAN_WINDOWS[1],
            40.0,
            Some(now + 28_800),
            Some(10_080),
        );
        assert!(
            AccountQuotaHint::from_minimax_windows(std::slice::from_ref(&interval), now).is_none()
        );
        let expired = QuotaWindow {
            resets_at: Some(now),
            ..interval.clone()
        };
        assert!(AccountQuotaHint::from_minimax_windows(&[expired, weekly.clone()], now).is_none());
        let without_timing = QuotaWindow {
            resets_at: None,
            ..interval.clone()
        };
        let hint =
            AccountQuotaHint::from_minimax_windows(&[without_timing, weekly.clone()], now).unwrap();
        assert_eq!(hint.remaining, 30.0);
        assert!(hint.reset.is_none() && hint.remaining_per_hour.is_none());
        let full =
            AccountQuotaHint::from_minimax_windows(&[interval.clone(), weekly], now).unwrap();
        assert_eq!(full.remaining, 30.0);
        assert_eq!(full.reset, Some(now + 3600));
        assert_eq!(full.remaining_per_hour, Some(5.0));
        let zero = QuotaWindow {
            remaining_percent: Some(0.0),
            ..interval
        };
        let included = AccountQuotaHint::from_minimax_windows(&[zero], now).unwrap();
        assert_eq!(included.remaining, 0.0);
        assert!(included.exhausted());
        assert!(included.reset.is_none() && included.remaining_per_hour.is_none());
    }
}
