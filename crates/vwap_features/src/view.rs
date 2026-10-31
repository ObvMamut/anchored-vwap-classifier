//! The causal view of the coarse bars at a touch bar.

use vwap_events::CoarseBars;

/// Bars of one session in order: `(session key, first index, one past the last index)`.
#[derive(Debug, Clone, Default)]
pub struct SessionIndex {
    pub spans: Vec<(i32, usize, usize)>,
}

impl SessionIndex {
    pub fn build(b: &CoarseBars) -> Self {
        let mut spans: Vec<(i32, usize, usize)> = Vec::new();
        for (i, &s) in b.session.iter().enumerate() {
            match spans.last_mut() {
                Some(last) if last.0 == s => last.2 = i + 1,
                _ => spans.push((s, i, i + 1)),
            }
        }
        Self { spans }
    }

    /// Position in `spans` of the session containing bar `i`.
    pub fn position_of_bar(&self, i: usize) -> Option<usize> {
        let k = self.spans.partition_point(|s| s.2 <= i);
        self.spans.get(k).filter(|s| s.1 <= i).map(|_| k)
    }
}

/// Read only access to the bars up to and including the touch bar `t`. The underlying
/// bars are private, so a feature cannot index past `t` by construction.
pub struct CausalView<'a> {
    bars: &'a CoarseBars,
    t: usize,
}

impl<'a> CausalView<'a> {
    pub fn new(bars: &'a CoarseBars, t: usize) -> Self {
        Self { bars, t }
    }

    pub fn t(&self) -> usize {
        self.t
    }

    pub fn width(&self) -> u32 {
        self.bars.width
    }

    fn get(&self, col: &[f64], i: usize) -> Option<f64> {
        if i <= self.t {
            col.get(i).copied()
        } else {
            None
        }
    }

    pub fn open(&self, i: usize) -> Option<f64> {
        self.get(&self.bars.open, i)
    }

    pub fn high(&self, i: usize) -> Option<f64> {
        self.get(&self.bars.high, i)
    }

    pub fn low(&self, i: usize) -> Option<f64> {
        self.get(&self.bars.low, i)
    }

    pub fn close(&self, i: usize) -> Option<f64> {
        self.get(&self.bars.close, i)
    }

    pub fn weight(&self, i: usize) -> Option<f64> {
        self.get(&self.bars.weight, i)
    }

    pub fn vwap(&self, i: usize) -> Option<f64> {
        self.get(&self.bars.vwap, i)
    }

    pub fn vwap_sd(&self, i: usize) -> Option<f64> {
        self.get(&self.bars.vwap_sd, i)
    }

    pub fn atr(&self, i: usize) -> Option<f64> {
        self.get(&self.bars.atr, i)
    }

    pub fn session(&self, i: usize) -> Option<i32> {
        if i <= self.t {
            self.bars.session.get(i).copied()
        } else {
            None
        }
    }

    pub fn minute(&self, i: usize) -> Option<u16> {
        if i <= self.t {
            self.bars.minute.get(i).copied()
        } else {
            None
        }
    }

    /// True if bar `i` (at or before `t`) is in the same session as the touch bar.
    pub fn in_touch_session(&self, i: usize) -> bool {
        match (self.session(i), self.session(self.t)) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
    }
}
