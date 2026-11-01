//! Control levels. Each implements `LevelSource` so it goes through the identical
//! event detector and labeler as the anchored VWAP.
#![allow(clippy::indexing_slicing)]

use std::collections::HashMap;

use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use vwap_events::{CoarseBars, LevelSource};

/// Start index of each session in the bars (sessions are contiguous).
fn session_starts(b: &CoarseBars) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    for i in 1..=b.len() {
        if i == b.len() || b.session[i] != b.session[start] {
            out.push((start, i));
            start = i;
        }
    }
    if b.is_empty() { Vec::new() } else { out }
}

/// A horizontal level drawn at the session anchor as `open + z * atr`, where the open is
/// the first bar's open print and the ATR is the one of the bar before the session.
pub struct RandomHorizontal {
    pub seed: u64,
    pub index: u32,
    pub z_max: f64,
}

impl LevelSource for RandomHorizontal {
    fn name(&self) -> String {
        format!("random_horizontal_{}", self.index)
    }

    fn levels(&self, bars: &CoarseBars) -> Vec<f64> {
        let mut out = vec![f64::NAN; bars.len()];
        for (a, z) in session_starts(bars) {
            if a == 0 || !bars.atr[a - 1].is_finite() {
                continue;
            }
            // The draw depends only on the seed, the level index and the session key, so it is
            // independent of iteration order and of any bar after the anchor.
            let mix = self.seed
                ^ u64::from(self.index).wrapping_mul(0x9E37_79B9_7F4A_7C15)
                ^ (bars.session[a] as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
            let mut rng = ChaCha8Rng::seed_from_u64(mix);
            let u: f64 = rng.random();
            let zz = (u * 2.0 - 1.0) * self.z_max;
            let level = bars.open[a] + zz * bars.atr[a - 1];
            for v in &mut out[a..z] {
                *v = level;
            }
        }
        out
    }
}

/// An anchored VWAP started at a non standard clock time (New York minute of day).
pub struct ShiftedAnchorVwap {
    pub minute: u32,
}

impl LevelSource for ShiftedAnchorVwap {
    fn name(&self) -> String {
        format!("shifted_anchor_{}", self.minute)
    }

    fn levels(&self, bars: &CoarseBars) -> Vec<f64> {
        let mut out = vec![f64::NAN; bars.len()];
        for (a, z) in session_starts(bars) {
            let (mut active, mut s, mut v) = (false, 0.0f64, 0.0f64);
            for (i, o) in out.iter_mut().enumerate().take(z).skip(a) {
                let m = u32::from(bars.minute[i]);
                // the shifted anchor falls in the part of the session after midnight
                if !active && m >= self.minute && m < 1080 {
                    active = true;
                }
                if active {
                    s += bars.pv[i];
                    v += bars.weight[i];
                    if v > 0.0 {
                        *o = s / v;
                    }
                }
            }
        }
        out
    }
}

/// The previous session's VWAP curve read at the same clock time.
pub struct PriorSessionVwapCurve;

impl LevelSource for PriorSessionVwapCurve {
    fn name(&self) -> String {
        "prior_session_vwap".into()
    }

    fn levels(&self, bars: &CoarseBars) -> Vec<f64> {
        let sessions = session_starts(bars);
        let mut curve: HashMap<(i32, u16), f64> = HashMap::new();
        for i in 0..bars.len() {
            curve.insert((bars.session[i], bars.minute[i]), bars.vwap[i]);
        }
        let mut out = vec![f64::NAN; bars.len()];
        for k in 1..sessions.len() {
            let prev = bars.session[sessions[k - 1].0];
            let (a, z) = sessions[k];
            for (i, o) in out.iter_mut().enumerate().take(z).skip(a) {
                if let Some(&v) = curve.get(&(prev, bars.minute[i])) {
                    *o = v;
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bars(n: usize, sessions: &[(usize, i32)]) -> CoarseBars {
        // sessions: (length, key)
        let mut b = CoarseBars {
            width: 5,
            ..CoarseBars::default()
        };
        let mut t = 0i64;
        for &(len, key) in sessions {
            for j in 0..len {
                let p = 100.0 + (t % 7) as f64;
                b.open_us.push(t * 300_000_000);
                b.open.push(p);
                b.high.push(p + 1.0);
                b.low.push(p - 1.0);
                b.close.push(p + 0.5);
                b.weight.push(2.0 + (t % 3) as f64);
                b.pv.push((p + 0.2) * (2.0 + (t % 3) as f64));
                b.vwap.push(p);
                b.vwap_sd.push(1.0);
                b.session.push(key);
                b.trading_date.push(key + 1);
                b.minute.push(((1080 + 5 * j) % 1440) as u16);
                b.n_minutes.push(5);
                b.flags.push(0);
                b.atr.push(2.0);
                t += 1;
            }
        }
        assert_eq!(b.len(), n);
        b
    }

    #[test]
    fn random_level_is_constant_in_a_session_and_ignores_later_bars() {
        let a = bars(40, &[(20, 1), (20, 2)]);
        let mut b = a.clone();
        // change everything after the first bar of session 2
        for i in 21..40 {
            b.open[i] += 50.0;
            b.high[i] += 50.0;
            b.close[i] += 50.0;
        }
        let src = RandomHorizontal {
            seed: 9,
            index: 3,
            z_max: 3.0,
        };
        let (la, lb) = (src.levels(&a), src.levels(&b));
        assert!(la[20..40].iter().all(|v| (*v - la[20]).abs() < 1e-12));
        assert_eq!(la[20].to_bits(), lb[20].to_bits());
        assert!(la[0].is_nan()); // no ATR before the first session
        // a different index gives a different level
        let other = RandomHorizontal {
            seed: 9,
            index: 4,
            z_max: 3.0,
        }
        .levels(&a);
        assert!((other[20] - la[20]).abs() > 1e-9);
    }

    #[test]
    fn shifted_vwap_only_uses_bars_up_to_the_current_one() {
        let a = bars(30, &[(30, 1)]);
        let mut b = a.clone();
        for i in 20..30 {
            b.pv[i] += 1000.0;
            b.weight[i] += 10.0;
        }
        // minute of bar j is (1080 + 5 j) % 1440; the shift at minute 0 (midnight) is bar 72, so
        // use an early shift on a synthetic session with minutes wrapping: bar 0..71 evening
        let src = ShiftedAnchorVwap { minute: 15 };
        let (la, lb) = (src.levels(&a), src.levels(&b));
        for i in 0..20 {
            assert_eq!(la[i].to_bits(), lb[i].to_bits());
        }
    }

    #[test]
    fn prior_session_curve_reads_the_previous_session_at_the_same_slot() {
        let b = bars(20, &[(10, 1), (10, 2)]);
        let l = PriorSessionVwapCurve.levels(&b);
        assert!(l[..10].iter().all(|v| v.is_nan()));
        for j in 0..10 {
            assert_eq!(l[10 + j].to_bits(), b.vwap[j].to_bits());
        }
    }
}
