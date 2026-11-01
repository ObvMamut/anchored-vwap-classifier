//! Purged walk forward splits with an embargo, and the inner blocked expanding splits used by
//! the nested scheme. A training event is usable for a test window starting at $a$ only if its
//! label span, plus the embargo, ended before $a$.
#![allow(clippy::indexing_slicing)]

use chrono::{DateTime, Months, Utc};

/// The time span of one event: when it closed (when it became known) and when its label ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventSpan {
    pub t_close_us: i64,
    pub t_end_us: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A fixed length training window that moves forward.
    Rolling,
    /// A training window that starts at the beginning of the range and grows.
    Expanding,
}

#[derive(Debug, Clone)]
pub struct WalkForwardConfig {
    pub train_months: u32,
    pub test_months: u32,
    pub step_months: u32,
    pub mode: Mode,
    /// Embargo in microseconds, at least the label horizon.
    pub embargo_us: i64,
}

/// One outer fold. Indices refer to the `spans` slice passed to `walk_forward`.
#[derive(Debug, Clone)]
pub struct Split {
    pub fold: usize,
    pub train_start_us: i64,
    pub test_start_us: i64,
    pub test_end_us: i64,
    pub train: Vec<usize>,
    pub test: Vec<usize>,
    /// Events in the training window removed because their label span plus the embargo reaches
    /// the test start.
    pub purged: Vec<usize>,
}

/// Shifts a microsecond timestamp by whole calendar months (negative goes back).
pub fn shift_months(us: i64, months: i64) -> Option<i64> {
    let dt = DateTime::<Utc>::from_timestamp_micros(us)?;
    let m = Months::new(u32::try_from(months.unsigned_abs()).ok()?);
    let out = if months >= 0 {
        dt.checked_add_months(m)?
    } else {
        dt.checked_sub_months(m)?
    };
    Some(out.timestamp_micros())
}

/// Walk forward folds over `range = [start, end)`. Test windows are full windows only: the first
/// starts `train_months` after `start`, each next one `step_months` later, and a window that
/// would end after `end` is not produced.
pub fn walk_forward(spans: &[EventSpan], range: (i64, i64), cfg: &WalkForwardConfig) -> Vec<Split> {
    let mut out = Vec::new();
    let Some(first) = shift_months(range.0, i64::from(cfg.train_months)) else {
        return out;
    };
    for fold in 0.. {
        let Some(a) = shift_months(first, i64::from(cfg.step_months) * fold) else {
            break;
        };
        let Some(b) = shift_months(a, i64::from(cfg.test_months)) else {
            break;
        };
        if b > range.1 {
            break;
        }
        let train_start = match cfg.mode {
            Mode::Rolling => shift_months(a, -i64::from(cfg.train_months)).unwrap_or(range.0),
            Mode::Expanding => range.0,
        };
        let (mut train, mut purged, mut test) = (Vec::new(), Vec::new(), Vec::new());
        for (i, s) in spans.iter().enumerate() {
            if s.t_close_us >= train_start && s.t_close_us < a {
                if s.t_end_us + cfg.embargo_us >= a {
                    purged.push(i);
                } else {
                    train.push(i);
                }
            } else if s.t_close_us >= a && s.t_close_us < b {
                test.push(i);
            }
        }
        out.push(Split {
            fold: fold as usize,
            train_start_us: train_start,
            test_start_us: a,
            test_end_us: b,
            train,
            test,
            purged,
        });
    }
    out
}

/// One inner fold of the blocked expanding scheme.
#[derive(Debug, Clone)]
pub struct InnerFold {
    /// Index of the block that is predicted (1 to `n_blocks`; block 0 is only ever training data).
    pub block: usize,
    pub train: Vec<usize>,
    pub test: Vec<usize>,
}

/// Cuts the events `idx` (indices into `spans`) into `n_blocks + 1` contiguous blocks of about
/// equal size in time order. Fold `k` predicts block `k` from the events of the earlier blocks
/// whose label span plus the embargo ended before block `k` starts.
pub fn blocked_expanding(
    spans: &[EventSpan],
    idx: &[usize],
    n_blocks: usize,
    embargo_us: i64,
) -> Vec<InnerFold> {
    let mut order: Vec<usize> = idx.to_vec();
    order.sort_by_key(|&i| (spans[i].t_close_us, i));
    let n = order.len();
    let parts = n_blocks + 1;
    if n_blocks == 0 || n < parts {
        return Vec::new();
    }
    let size = n / parts;
    let bounds: Vec<usize> = (0..=parts)
        .map(|k| if k == parts { n } else { k * size })
        .collect();
    let mut folds = Vec::new();
    for k in 1..=n_blocks {
        let test: Vec<usize> = order[bounds[k]..bounds[k + 1]].to_vec();
        let start = spans[test[0]].t_close_us;
        let train: Vec<usize> = order[..bounds[k]]
            .iter()
            .copied()
            .filter(|&i| spans[i].t_end_us + embargo_us < start)
            .collect();
        folds.push(InnerFold {
            block: k,
            train,
            test,
        });
    }
    folds
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const DAY: i64 = 86_400_000_000;

    fn us(y: i32, m: u32, d: u32) -> i64 {
        chrono::NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp_micros()
    }

    fn cfg(embargo_days: i64) -> WalkForwardConfig {
        WalkForwardConfig {
            train_months: 24,
            test_months: 6,
            step_months: 6,
            mode: Mode::Rolling,
            embargo_us: embargo_days * DAY,
        }
    }

    #[test]
    fn the_discovery_range_gives_five_folds_of_six_month_tests() {
        let range = (us(2016, 11, 15), us(2021, 10, 1));
        let spans: Vec<EventSpan> = (0..2000)
            .map(|i| {
                let t = range.0 + i * 1_000_000_000 * 8;
                EventSpan {
                    t_close_us: t,
                    t_end_us: t + DAY / 4,
                }
            })
            .collect();
        let f = walk_forward(&spans, range, &cfg(1));
        assert_eq!(f.len(), 5);
        assert_eq!(f[0].test_start_us, us(2018, 11, 15));
        assert_eq!(f[0].test_end_us, us(2019, 5, 15));
        assert_eq!(f[4].test_end_us, us(2021, 5, 15));
        for w in f.windows(2) {
            assert_eq!(
                w[1].test_start_us, w[0].test_end_us,
                "step equals the test length here"
            );
        }
        // rolling training windows are 24 months long
        assert_eq!(
            f[2].train_start_us,
            shift_months(f[2].test_start_us, -24).unwrap()
        );
    }

    #[test]
    fn expanding_mode_keeps_the_start_of_the_range() {
        let range = (us(2016, 11, 15), us(2021, 10, 1));
        let mut c = cfg(1);
        c.mode = Mode::Expanding;
        let f = walk_forward(&[], range, &c);
        assert!(f.iter().all(|s| s.train_start_us == range.0));
    }

    #[test]
    fn a_label_reaching_the_test_start_is_purged_and_the_embargo_widens_the_purge() {
        let a = us(2018, 11, 15);
        let range = (us(2016, 11, 15), us(2021, 10, 1));
        let spans = [
            EventSpan {
                t_close_us: a - 10 * DAY,
                t_end_us: a - 9 * DAY,
            }, // clean
            EventSpan {
                t_close_us: a - 2 * DAY,
                t_end_us: a + DAY,
            }, // label crosses the start
            EventSpan {
                t_close_us: a - 3 * DAY,
                t_end_us: a - 2 * DAY,
            }, // ends 2 days before
            EventSpan {
                t_close_us: a + DAY,
                t_end_us: a + 2 * DAY,
            }, // a test event
        ];
        let f = walk_forward(&spans, range, &cfg(0));
        assert_eq!(
            (f[0].train.clone(), f[0].purged.clone(), f[0].test.clone()),
            (vec![0, 2], vec![1], vec![3])
        );
        // an embargo of 3 days also removes the event that ended 2 days before the start
        let f = walk_forward(&spans, range, &cfg(3));
        assert_eq!(f[0].train, vec![0]);
        assert_eq!(f[0].purged, vec![1, 2]);
    }

    proptest! {
        #[test]
        fn folds_never_leak_and_account_for_every_event(
            times in proptest::collection::vec((0i64..1_800, 1i64..120), 50..400),
            embargo_days in 0i64..40,
        ) {
            let range = (us(2016, 11, 15), us(2021, 10, 1));
            let spans: Vec<EventSpan> = times
                .iter()
                .map(|&(d, len)| EventSpan { t_close_us: range.0 + d * DAY, t_end_us: range.0 + (d + len) * DAY })
                .collect();
            let c = cfg(embargo_days);
            let folds = walk_forward(&spans, range, &c);
            for w in folds.windows(2) {
                prop_assert!(w[1].test_start_us > w[0].test_start_us);
            }
            for f in &folds {
                for &i in &f.train {
                    prop_assert!(spans[i].t_end_us + c.embargo_us < f.test_start_us);
                    prop_assert!(spans[i].t_close_us < f.test_start_us && spans[i].t_close_us >= f.train_start_us);
                }
                for &i in &f.purged {
                    prop_assert!(spans[i].t_end_us + c.embargo_us >= f.test_start_us);
                }
                for &i in &f.test {
                    prop_assert!(spans[i].t_close_us >= f.test_start_us && spans[i].t_close_us < f.test_end_us);
                }
                // train, purged and test are disjoint
                let mut all: Vec<usize> = f.train.iter().chain(&f.purged).chain(&f.test).copied().collect();
                let n = all.len();
                all.sort_unstable();
                all.dedup();
                prop_assert_eq!(all.len(), n);
                // every event closing in the training window is either trained on or purged
                let in_window = spans
                    .iter()
                    .filter(|s| s.t_close_us >= f.train_start_us && s.t_close_us < f.test_start_us)
                    .count();
                prop_assert_eq!(in_window, f.train.len() + f.purged.len());
            }
        }

        #[test]
        fn inner_folds_are_chronological_and_purged(
            times in proptest::collection::vec((0i64..1_000, 1i64..60), 30..300),
            n_blocks in 2usize..7,
            embargo_days in 0i64..20,
        ) {
            let spans: Vec<EventSpan> = times
                .iter()
                .map(|&(d, len)| EventSpan { t_close_us: d * DAY, t_end_us: (d + len) * DAY })
                .collect();
            let idx: Vec<usize> = (0..spans.len()).collect();
            let folds = blocked_expanding(&spans, &idx, n_blocks, embargo_days * DAY);
            prop_assert_eq!(folds.len(), n_blocks);
            let mut seen_test: Vec<usize> = Vec::new();
            for f in &folds {
                prop_assert!(!f.test.is_empty());
                let start = f.test.iter().map(|&i| spans[i].t_close_us).min().unwrap();
                for &i in &f.train {
                    prop_assert!(spans[i].t_end_us + embargo_days * DAY < start);
                    prop_assert!(!f.test.contains(&i));
                }
                seen_test.extend(&f.test);
            }
            // each event is predicted at most once, and block 0 never is
            let mut sorted = seen_test.clone();
            sorted.sort_unstable();
            sorted.dedup();
            prop_assert_eq!(sorted.len(), seen_test.len());
        }
    }
}
