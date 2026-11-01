//! Builds labeled, featured samples for every event of every timeframe. The result covers
//! all segments; the split views decide what each step may read.

use chrono::{DateTime, Datelike};
use vwap_core::{DataConfig, EventsConfig, FeaturesConfig, LabelsConfig};
use vwap_data::CleanBars;
use vwap_events::{build, detect, event_params};
use vwap_features::{LevelTable, SessionIndex, build_features};
use vwap_labels::{LabelParams, label_events};

use crate::sample::Sample;

pub fn assemble(
    data: &DataConfig,
    ev: &EventsConfig,
    lab: &LabelsConfig,
    feat: &FeaturesConfig,
    bars: &CleanBars,
) -> Vec<Sample> {
    let params = event_params(data, ev);
    let levels = LevelTable::build(bars, feat);
    let mut out = Vec::new();
    for &w in &ev.timeframes {
        let coarse = build(bars, data, w, ev.atr_length);
        let (events, _) = detect(&coarse, &params);
        let lp = LabelParams {
            m1: lab.m1,
            m2: lab.m2,
            horizon: lab.horizon(w),
        };
        let (labeled, _) = label_events(&events, &coarse, bars, &lp);
        let evs: Vec<_> = labeled.iter().map(|l| l.event.clone()).collect();
        let sessions = SessionIndex::build(&coarse);
        let table = build_features(&evs, &coarse, &levels, &sessions, feat);
        for (i, l) in labeled.iter().enumerate() {
            out.push(Sample {
                width: w,
                event_id: l.event.id,
                session: l.event.session,
                year: DateTime::from_timestamp_micros(l.event.close_us)
                    .unwrap_or_default()
                    .year(),
                t_close_us: l.event.close_us,
                t_end_us: l.t_end_us,
                side: l.event.side,
                class: l.class,
                ambiguous: l.ambiguous,
                outcome_ticks: l.outcome_ticks,
                sigma_over_price: l.sigma / l.event.close,
                bar: l.event.bar,
                close: l.event.close,
                sigma: l.sigma,
                b_rej: l.b_rej,
                b_brk: l.b_brk,
                r_t_ticks: l.r_t_ticks,
                features: table
                    .cols
                    .iter()
                    .map(|c| c.get(i).copied().unwrap_or(f64::NAN))
                    .collect(),
            });
        }
    }
    out
}
