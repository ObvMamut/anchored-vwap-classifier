//! Levels that touches are detected against. The anchored VWAP and every control
//! level implement the same trait, so controls run through the identical detector.

use crate::bars::CoarseBars;

pub trait LevelSource {
    fn name(&self) -> String;
    /// The level at the close of each bar, `NAN` where it is undefined. The value at
    /// index `i` may use only bars up to and including `i`, or earlier sessions.
    fn levels(&self, bars: &CoarseBars) -> Vec<f64>;
}

/// The session anchored VWAP built by `build`.
pub struct AnchoredVwap;

impl LevelSource for AnchoredVwap {
    fn name(&self) -> String {
        "anchored_vwap".into()
    }

    fn levels(&self, bars: &CoarseBars) -> Vec<f64> {
        bars.vwap.clone()
    }
}
