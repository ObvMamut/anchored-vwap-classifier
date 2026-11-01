use vwap_core::Class;

/// One event with its label, features and the fields the statistics, models and backtest need.
#[derive(Debug, Clone, Default)]
pub struct Sample {
    pub width: u32,
    pub event_id: u32,
    pub session: i32,
    pub year: i32,
    pub t_close_us: i64,
    pub t_end_us: i64,
    pub side: i8,
    pub class: Option<Class>,
    pub ambiguous: bool,
    pub outcome_ticks: f64,
    pub sigma_over_price: f64,
    /// Index of the touch bar in the coarse bars of this width.
    pub bar: usize,
    /// Close of the touch bar.
    pub close: f64,
    /// Volatility scale used by the barriers, in price units.
    pub sigma: f64,
    /// Rejection barrier price and breakthrough barrier price, fixed at the touch close.
    pub b_rej: f64,
    pub b_brk: f64,
    /// Return at the horizon in ticks, signed in the rejection direction.
    pub r_t_ticks: f64,
    /// Aligned with `vwap_features::feature_names()`.
    pub features: Vec<f64>,
}
