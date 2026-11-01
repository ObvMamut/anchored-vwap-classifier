//! Prior day, overnight and opening range style levels from the 1 minute bars.
#![allow(clippy::indexing_slicing)]

use vwap_data::CleanBars;

/// Levels of one regular trading hours date.
#[derive(Debug, Clone, Copy)]
pub struct DayLevels {
    /// New York date, days since the Unix epoch.
    pub date: i32,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    /// Overnight range from the previous RTH close to this date's 09:30, complete at 09:30.
    pub on_high: f64,
    pub on_low: f64,
}

#[derive(Debug, Clone, Default)]
pub struct LevelTable {
    pub days: Vec<DayLevels>,
}

/// Levels as seen by an event: prior day values, and the overnight range once it is complete.
#[derive(Debug, Clone, Copy)]
pub struct EventLevels {
    pub pdh: f64,
    pub pdl: f64,
    pub pdc: f64,
    pub onh: f64,
    pub onl: f64,
}

impl LevelTable {
    /// Regular hours come from `cfg` (09:30 to 16:00 New York by default).
    pub fn build(b: &CleanBars, cfg: &vwap_core::FeaturesConfig) -> Self {
        let mut days: Vec<DayLevels> = Vec::new();
        let (mut on_h, mut on_l) = (f64::NAN, f64::NAN);
        let mut cur: Option<DayLevels> = None;
        for k in 0..b.len() {
            let m = b.ny_minute[k];
            if (cfg.rth_open_minute..cfg.rth_close_minute).contains(&m) {
                let date = b.ny_date[k];
                match cur.as_mut() {
                    Some(d) if d.date == date => {
                        d.high = d.high.max(b.high[k]);
                        d.low = d.low.min(b.low[k]);
                        d.close = b.close[k];
                    }
                    _ => {
                        if let Some(d) = cur.take() {
                            days.push(d);
                        }
                        cur = Some(DayLevels {
                            date,
                            high: b.high[k],
                            low: b.low[k],
                            close: b.close[k],
                            on_high: on_h,
                            on_low: on_l,
                        });
                        (on_h, on_l) = (f64::NAN, f64::NAN);
                    }
                }
            } else {
                // outside RTH: overnight accumulation for the next RTH date
                if on_h.is_nan() {
                    (on_h, on_l) = (b.high[k], b.low[k]);
                } else {
                    on_h = on_h.max(b.high[k]);
                    on_l = on_l.min(b.low[k]);
                }
            }
        }
        if let Some(d) = cur {
            days.push(d);
        }
        Self { days }
    }

    /// Levels for an event on trading date `date`. `after_open` says whether the event
    /// closes at or after 09:30 New York of that date, when the overnight range is final.
    pub fn for_event(&self, date: i32, after_open: bool) -> Option<EventLevels> {
        let k = self.days.partition_point(|d| d.date < date);
        let prior = self.days.get(k.checked_sub(1)?)?;
        let today = self.days.get(k).filter(|d| d.date == date);
        let (onh, onl) = match (after_open, today) {
            (true, Some(d)) => (d.on_high, d.on_low),
            _ => (f64::NAN, f64::NAN),
        };
        Some(EventLevels {
            pdh: prior.high,
            pdl: prior.low,
            pdc: prior.close,
            onh,
            onl,
        })
    }
}
