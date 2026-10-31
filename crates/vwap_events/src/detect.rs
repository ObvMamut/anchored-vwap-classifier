//! Touch detection with the approach side, the re arm rule and the exclusion window.
#![allow(clippy::indexing_slicing)]

use serde::Serialize;
use vwap_core::{SessionAnchor, VwapReference};
use vwap_data::clean::flag;

use crate::bars::CoarseBars;

/// Flags on the touch bar or the previous bar that make an event unusable.
const BLOCKING_FLAGS: u32 = flag::GAP_BEFORE | flag::ROLL_BUFFER | flag::THIN_SESSION;

#[derive(Debug, Clone, Copy)]
pub struct EventParams {
    pub anchor: SessionAnchor,
    pub anchor_minute: u32,
    pub flat_minute: u32,
    pub exclude_minutes: u32,
    pub rearm_k: f64,
    pub reference: VwapReference,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub id: u32,
    pub width: u32,
    /// Index of the touch bar in the coarse bars.
    pub bar: usize,
    pub session: i32,
    pub trading_date: i32,
    pub open_us: i64,
    pub close_us: i64,
    /// +1 when price approached from above, -1 from below.
    pub side: i8,
    /// The VWAP value used in the touch test (depends on `vwap_reference`).
    pub vwap: f64,
    pub vwap_bar: f64,
    pub vwap_prev: f64,
    pub vwap_sd: f64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub atr: f64,
    pub minutes_since_anchor: u32,
    /// Minutes from the close of the touch bar to the session flat time.
    pub minutes_to_flat: i32,
    /// Position among the accepted events of the session, from 1.
    pub ordinal: u32,
    /// Touches before this one in the session after the exclusion window.
    pub raw_touches_before: u32,
    pub flags: u32,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct DropCounts {
    pub touches_total: usize,
    pub accepted: usize,
    pub zero_side: usize,
    pub no_previous_bar: usize,
    pub partial_bar: usize,
    pub atr_undefined: usize,
    pub blocked_by_flags: usize,
    pub after_flat_time: usize,
    pub while_disarmed: usize,
}

fn minutes_since_anchor(minute: u16, width: u32, p: &EventParams) -> u32 {
    let close = u32::from(minute) + width;
    match p.anchor {
        SessionAnchor::Globex => (close + 1440 - p.anchor_minute) % 1440,
        SessionAnchor::Rth => close.saturating_sub(p.anchor_minute),
    }
}

fn minutes_to_flat(minute: u16, width: u32, p: &EventParams) -> i32 {
    let cm = (u32::from(minute) + width) % 1440;
    let flat = p.flat_minute as i32;
    match p.anchor {
        SessionAnchor::Globex if cm >= p.anchor_minute => 1440 - cm as i32 + flat,
        _ => flat - cm as i32,
    }
}

/// Events for the anchored VWAP of the bars.
pub fn detect(bars: &CoarseBars, p: &EventParams) -> (Vec<Event>, DropCounts) {
    detect_level(bars, &bars.vwap, p)
}

/// Events for any level series sampled at the close of each bar. The anchored VWAP and every
/// control level go through exactly this function.
pub fn detect_level(bars: &CoarseBars, level: &[f64], p: &EventParams) -> (Vec<Event>, DropCounts) {
    let mut events = Vec::new();
    let mut counts = DropCounts::default();
    let mut cur_session = i32::MIN;
    let (mut started, mut armed, mut ordinal, mut raw_touches) = (false, false, 0u32, 0u32);
    for i in 0..bars.len() {
        let session = bars.session[i];
        if session != cur_session {
            cur_session = session;
            started = false;
            armed = false;
            ordinal = 0;
            raw_touches = 0;
        }
        let msa = minutes_since_anchor(bars.minute[i], bars.width, p);
        if msa < p.exclude_minutes {
            continue;
        }
        if !started {
            started = true;
            armed = true;
        }
        let prev_ok = i > 0 && bars.session[i - 1] == session;
        let reference = match (p.reference, prev_ok) {
            (VwapReference::BarClose, _) => level[i],
            (VwapReference::PriorBarClose, true) => level[i - 1],
            (VwapReference::PriorBarClose, false) => f64::NAN,
        };
        let is_touch =
            reference.is_finite() && bars.low[i] <= reference && reference <= bars.high[i];
        if is_touch {
            counts.touches_total += 1;
        }
        let touches_before = raw_touches;
        if is_touch {
            raw_touches += 1;
        }
        if armed && is_touch {
            let (ok, side) = qualify(bars, level, i, prev_ok, p, &mut counts);
            if ok {
                ordinal += 1;
                armed = false;
                counts.accepted += 1;
                events.push(Event {
                    id: u32::try_from(events.len()).unwrap_or(u32::MAX),
                    width: bars.width,
                    bar: i,
                    session,
                    trading_date: bars.trading_date[i],
                    open_us: bars.open_us[i],
                    close_us: bars.open_us[i] + i64::from(bars.width) * 60_000_000,
                    side,
                    vwap: reference,
                    vwap_bar: level[i],
                    vwap_prev: level[i - 1],
                    vwap_sd: bars.vwap_sd[i],
                    open: bars.open[i],
                    high: bars.high[i],
                    low: bars.low[i],
                    close: bars.close[i],
                    atr: bars.atr[i],
                    minutes_since_anchor: msa,
                    minutes_to_flat: minutes_to_flat(bars.minute[i], bars.width, p),
                    ordinal,
                    raw_touches_before: touches_before,
                    flags: bars.flags[i] | bars.flags[i - 1],
                });
                continue;
            }
        } else if !armed {
            if is_touch {
                counts.while_disarmed += 1;
            }
            let e = (bars.close[i] - level[i]).abs() / bars.atr[i];
            if e.is_finite() && e >= p.rearm_k {
                armed = true;
            }
        }
    }
    (events, counts)
}

fn qualify(
    bars: &CoarseBars,
    level: &[f64],
    i: usize,
    prev_ok: bool,
    p: &EventParams,
    c: &mut DropCounts,
) -> (bool, i8) {
    if !prev_ok {
        c.no_previous_bar += 1;
        return (false, 0);
    }
    if bars.partial(i) || bars.partial(i - 1) {
        c.partial_bar += 1;
        return (false, 0);
    }
    let diff = bars.close[i - 1] - level[i - 1];
    let side = if diff > 0.0 {
        1
    } else if diff < 0.0 {
        -1
    } else {
        c.zero_side += 1;
        return (false, 0);
    };
    if !bars.atr[i].is_finite() || bars.atr[i] <= 0.0 || !level[i - 1].is_finite() {
        c.atr_undefined += 1;
        return (false, 0);
    }
    if (bars.flags[i] | bars.flags[i - 1]) & BLOCKING_FLAGS != 0 {
        c.blocked_by_flags += 1;
        return (false, 0);
    }
    if minutes_to_flat(bars.minute[i], bars.width, p) < 0 {
        c.after_flat_time += 1;
        return (false, 0);
    }
    (true, side)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> EventParams {
        EventParams {
            anchor: SessionAnchor::Globex,
            anchor_minute: 1080,
            flat_minute: 1015,
            exclude_minutes: 0,
            rearm_k: 1.0,
            reference: VwapReference::BarClose,
        }
    }

    fn fixture(rows: &[(f64, f64, f64, f64)]) -> CoarseBars {
        let n = rows.len();
        CoarseBars {
            width: 5,
            open_us: (0..n as i64).map(|i| i * 300_000_000).collect(),
            open: rows.iter().map(|r| r.0).collect(),
            high: rows.iter().map(|r| r.1).collect(),
            low: rows.iter().map(|r| r.2).collect(),
            close: rows.iter().map(|r| r.3).collect(),
            weight: vec![1.0; n],
            pv: vec![100.0; n],
            vwap: vec![100.0; n],
            vwap_sd: vec![1.0; n],
            session: vec![1; n],
            trading_date: vec![2; n],
            minute: (0..n as u16).map(|i| 1110 + 5 * i).collect(),
            n_minutes: vec![5; n],
            flags: vec![0; n],
            atr: vec![2.0; n],
        }
    }

    #[test]
    fn rearm_state_machine_on_a_hand_built_session() {
        let bars = fixture(&[
            (103.0, 104.0, 102.0, 103.0), // 0 above the VWAP
            (103.0, 103.5, 99.5, 100.5),  // 1 touch from above -> event
            (100.5, 101.0, 99.8, 100.2),  // 2 touch while disarmed (excursion 0.1 ATR)
            (100.2, 103.0, 101.0, 102.5), // 3 closes 1.25 ATR away -> re armed
            (102.5, 102.7, 99.0, 99.5),   // 4 touch from above -> event
            (99.5, 99.9, 97.0, 97.5),     // 5 no touch, 1.25 ATR below -> re armed
            (97.5, 100.5, 97.0, 100.2),   // 6 touch from below -> event
        ]);
        let (events, counts) = detect(&bars, &params());
        let idx: Vec<usize> = events.iter().map(|e| e.bar).collect();
        let sides: Vec<i8> = events.iter().map(|e| e.side).collect();
        assert_eq!(idx, vec![1, 4, 6]);
        assert_eq!(sides, vec![1, 1, -1]);
        assert_eq!(counts.while_disarmed, 1);
        assert_eq!(
            events.iter().map(|e| e.ordinal).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(events[0].raw_touches_before, 0);
        assert_eq!(events[1].raw_touches_before, 2);
    }

    #[test]
    fn exclusion_window_ignores_early_touches_and_starts_armed_afterwards() {
        let mut bars = fixture(&[
            (103.0, 104.0, 102.0, 103.0),
            (103.0, 103.5, 99.5, 100.5), // touch at 10 minutes after the anchor
            (100.5, 103.0, 100.2, 102.9),
            (102.9, 103.0, 99.5, 99.8), // first eligible touch
        ]);
        bars.minute = (0..4u16).map(|i| 1080 + 5 * i).collect();
        let (all, _) = detect(&bars, &params());
        assert_eq!(all.iter().map(|e| e.bar).collect::<Vec<_>>(), vec![1, 3]);
        let mut p = params();
        p.exclude_minutes = 12;
        let (events, _) = detect(&bars, &p);
        assert_eq!(events.iter().map(|e| e.bar).collect::<Vec<_>>(), vec![3]);
    }

    #[test]
    fn blocked_flags_and_partial_bars_drop_the_event_but_keep_the_state_armed() {
        let mut bars = fixture(&[
            (103.0, 104.0, 102.0, 103.0),
            (103.0, 103.5, 99.5, 100.5),
            (100.5, 103.0, 101.0, 102.5),
            (102.5, 102.7, 99.0, 99.5),
        ]);
        bars.flags[1] = flag::THIN_SESSION;
        let (events, counts) = detect(&bars, &params());
        // bar 1 blocked, so the state stays armed and bar 3 is accepted
        assert_eq!(
            events.iter().map(|e| e.bar).collect::<Vec<_>>(),
            vec![2, 3]
                .into_iter()
                .filter(|b| *b == 3)
                .collect::<Vec<_>>()
        );
        assert!(counts.blocked_by_flags >= 1);
        let mut partial = fixture(&[(103.0, 104.0, 102.0, 103.0), (103.0, 103.5, 99.5, 100.5)]);
        partial.n_minutes[1] = 3;
        let (events, counts) = detect(&partial, &params());
        assert!(events.is_empty());
        assert_eq!(counts.partial_bar, 1);
    }

    #[test]
    fn prior_bar_close_reference_uses_the_previous_vwap() {
        let mut bars = fixture(&[
            (103.0, 104.0, 102.0, 103.0),
            (103.0, 103.5, 100.5, 101.0), // range does not contain 100.0
        ]);
        bars.vwap[1] = 101.0; // the bar close VWAP is inside the range, the prior one is not
        let mut p = params();
        let (e_bar, _) = detect(&bars, &p);
        assert_eq!(e_bar.len(), 1);
        p.reference = VwapReference::PriorBarClose;
        let (e_prior, _) = detect(&bars, &p);
        assert!(e_prior.is_empty());
    }
}
