//! Parquet output of coarse bars and events, and the stage driver.
#![allow(clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::Path;

use chrono::{DateTime, Datelike};
use polars::prelude::*;
use serde::Serialize;
use vwap_core::{DataConfig, EventsConfig};

use crate::bars::{CoarseBars, anchor_minute, build, flat_minute};
use crate::detect::{DropCounts, Event, EventParams, detect};
use crate::error::EventsError;

fn io_err(path: &Path) -> impl Fn(std::io::Error) -> EventsError + '_ {
    move |source| EventsError::Io {
        path: path.display().to_string(),
        source,
    }
}

fn ts_series(name: &str, v: Vec<i64>) -> Result<Series, EventsError> {
    Ok(Series::new(name.into(), v).cast(&DataType::Datetime(
        TimeUnit::Microseconds,
        Some(TimeZone::UTC),
    ))?)
}

pub fn bars_frame(b: &CoarseBars) -> Result<DataFrame, EventsError> {
    let f = |n: &str, v: &[f64]| Series::new(n.into(), v.to_vec());
    let i = |n: &str, v: Vec<i64>| Series::new(n.into(), v);
    Ok(DataFrame::new_infer_height(vec![
        ts_series("open_ts", b.open_us.clone())?.into(),
        f("open", &b.open).into(),
        f("high", &b.high).into(),
        f("low", &b.low).into(),
        f("close", &b.close).into(),
        f("weight", &b.weight).into(),
        f("vwap", &b.vwap).into(),
        f("vwap_sd", &b.vwap_sd).into(),
        f("atr", &b.atr).into(),
        i("session", b.session.iter().map(|&x| i64::from(x)).collect()).into(),
        i(
            "trading_date",
            b.trading_date.iter().map(|&x| i64::from(x)).collect(),
        )
        .into(),
        i("minute", b.minute.iter().map(|&x| i64::from(x)).collect()).into(),
        i(
            "n_minutes",
            b.n_minutes.iter().map(|&x| i64::from(x)).collect(),
        )
        .into(),
        i("flags", b.flags.iter().map(|&x| i64::from(x)).collect()).into(),
    ])?)
}

pub fn events_frame(e: &[Event]) -> Result<DataFrame, EventsError> {
    let f = |n: &str, g: &dyn Fn(&Event) -> f64| {
        Series::new(n.into(), e.iter().map(g).collect::<Vec<f64>>())
    };
    let i = |n: &str, g: &dyn Fn(&Event) -> i64| {
        Series::new(n.into(), e.iter().map(g).collect::<Vec<i64>>())
    };
    Ok(DataFrame::new_infer_height(vec![
        i("id", &|x| i64::from(x.id)).into(),
        i("width", &|x| i64::from(x.width)).into(),
        i("bar", &|x| x.bar as i64).into(),
        i("session", &|x| i64::from(x.session)).into(),
        i("trading_date", &|x| i64::from(x.trading_date)).into(),
        ts_series("open_ts", e.iter().map(|x| x.open_us).collect())?.into(),
        ts_series("close_ts", e.iter().map(|x| x.close_us).collect())?.into(),
        i("side", &|x| i64::from(x.side)).into(),
        f("vwap", &|x| x.vwap).into(),
        f("vwap_bar", &|x| x.vwap_bar).into(),
        f("vwap_prev", &|x| x.vwap_prev).into(),
        f("vwap_sd", &|x| x.vwap_sd).into(),
        f("open", &|x| x.open).into(),
        f("high", &|x| x.high).into(),
        f("low", &|x| x.low).into(),
        f("close", &|x| x.close).into(),
        f("atr", &|x| x.atr).into(),
        i("minutes_since_anchor", &|x| {
            i64::from(x.minutes_since_anchor)
        })
        .into(),
        i("minutes_to_flat", &|x| i64::from(x.minutes_to_flat)).into(),
        i("ordinal", &|x| i64::from(x.ordinal)).into(),
        i("raw_touches_before", &|x| i64::from(x.raw_touches_before)).into(),
        i("flags", &|x| i64::from(x.flags)).into(),
    ])?)
}

fn write_parquet(df: &mut DataFrame, path: &Path) -> Result<(), EventsError> {
    let file = File::create(path).map_err(io_err(path))?;
    ParquetWriter::new(file).finish(df)?;
    Ok(())
}

#[derive(Debug, Serialize)]
pub struct EventsSummary {
    pub width: u32,
    pub bars: usize,
    pub events: usize,
    pub sessions_with_events: usize,
    pub per_year: BTreeMap<i32, usize>,
    pub above: usize,
    pub below: usize,
    pub drops: DropCounts,
}

pub fn event_params(data: &DataConfig, ev: &EventsConfig) -> EventParams {
    EventParams {
        anchor: data.session_anchor,
        anchor_minute: anchor_minute(data.session_anchor),
        flat_minute: flat_minute(data),
        exclude_minutes: ev.exclude_minutes,
        rearm_k: ev.rearm_k,
        reference: ev.vwap_reference,
    }
}

/// The Phase 2 stage: coarse bars, VWAP, ATR and events for each timeframe.
pub fn run_events(
    data: &DataConfig,
    ev: &EventsConfig,
    bars: &vwap_data::CleanBars,
) -> Result<Vec<EventsSummary>, EventsError> {
    let out = Path::new(&ev.output_dir);
    fs::create_dir_all(out).map_err(io_err(out))?;
    let params = event_params(data, ev);
    let mut summaries = Vec::new();
    for &w in &ev.timeframes {
        let coarse = build(bars, data, w, ev.atr_length);
        let (events, drops) = detect(&coarse, &params);
        write_parquet(
            &mut bars_frame(&coarse)?,
            &out.join(format!("bars_{w}m.parquet")),
        )?;
        write_parquet(
            &mut events_frame(&events)?,
            &out.join(format!("events_{w}m.parquet")),
        )?;
        let mut per_year = BTreeMap::new();
        let mut sessions = std::collections::BTreeSet::new();
        for e in &events {
            let y = DateTime::from_timestamp_micros(e.open_us)
                .unwrap_or_default()
                .year();
            *per_year.entry(y).or_default() += 1;
            sessions.insert(e.session);
        }
        summaries.push(EventsSummary {
            width: w,
            bars: coarse.len(),
            events: events.len(),
            sessions_with_events: sessions.len(),
            per_year,
            above: events.iter().filter(|e| e.side > 0).count(),
            below: events.iter().filter(|e| e.side < 0).count(),
            drops,
        });
    }
    let report = out.join("events_report.json");
    fs::write(&report, serde_json::to_string_pretty(&summaries)?).map_err(io_err(&report))?;
    Ok(summaries)
}
