//! Candlestick charts of random development sessions with the anchored VWAP and
//! the detected events, for the visual check in `docs/EVENTS.md`.
#![allow(clippy::indexing_slicing)]

use std::collections::BTreeSet;
use std::fs;
use std::ops::Range;
use std::path::Path;

use chrono::{NaiveDate, TimeDelta};
use plotters::prelude::*;
use rand::SeedableRng;
use rand::seq::SliceRandom;
use rand_chacha::ChaCha8Rng;
use serde::Serialize;
use vwap_events::{CoarseBars, Event};

use crate::error::ReportError;

#[derive(Debug, Serialize)]
pub struct SpotcheckEntry {
    pub session_date: String,
    pub width: u32,
    pub bars: usize,
    pub events: usize,
    pub file: String,
}

/// Index range of the bars of one session.
fn session_range(b: &CoarseBars, session: i32) -> Range<usize> {
    let start = b.session.iter().position(|&s| s == session).unwrap_or(0);
    let end = b.session[start..]
        .iter()
        .position(|&s| s != session)
        .map_or(b.len(), |p| start + p);
    start..end
}

fn date_of(days: i32) -> NaiveDate {
    NaiveDate::from_ymd_opt(1970, 1, 1)
        .and_then(|e| e.checked_add_signed(TimeDelta::days(i64::from(days))))
        .unwrap_or_default()
}

fn clock(minute: i64) -> String {
    let m = minute.rem_euclid(1440);
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// Sessions eligible for the visual check: entirely inside `[start_us, end_us)`, which the CLI
/// sets to the discovery segment so that no chart ever shows confirmation or holdout data, and
/// with a full day of bars.
pub fn eligible_sessions(
    bars: &CoarseBars,
    start_us: i64,
    end_us: i64,
    min_session_minutes: u32,
) -> Vec<i32> {
    let width_us = i64::from(bars.width) * 60_000_000;
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for &s in &bars.session {
        if seen.insert(s) {
            let r = session_range(bars, s);
            let first = bars.open_us.get(r.start).copied().unwrap_or(i64::MAX);
            let last_end = bars
                .open_us
                .get(r.end.saturating_sub(1))
                .map_or(i64::MAX, |t| t + width_us);
            if r.len() as u32 * bars.width >= min_session_minutes
                && first >= start_us
                && last_end < end_us
            {
                out.push(s);
            }
        }
    }
    out
}

/// Draws `n_days` random sessions for the given bars and events. Returns the list
/// of files written. The seed and dates are recorded so the check can be repeated.
pub fn spotcheck(
    bars: &CoarseBars,
    events: &[Event],
    sessions: &[i32],
    exclude_minutes: u32,
    font_paths: &[String],
    dir: &Path,
) -> Result<Vec<SpotcheckEntry>, ReportError> {
    fs::create_dir_all(dir).map_err(|source| ReportError::Io {
        path: dir.display().to_string(),
        source,
    })?;
    crate::plots::register_fonts(font_paths)?;
    let mut out = Vec::new();
    for &s in sessions {
        let r = session_range(bars, s);
        let ev: Vec<&Event> = events.iter().filter(|e| e.session == s).collect();
        let file = dir.join(format!("{}_{}m.png", date_of(s), bars.width));
        draw(bars, r.clone(), &ev, exclude_minutes, &file)
            .map_err(|e| ReportError::Draw(format!("{e}")))?;
        out.push(SpotcheckEntry {
            session_date: date_of(s).to_string(),
            width: bars.width,
            bars: r.len(),
            events: ev.len(),
            file: file.display().to_string(),
        });
    }
    Ok(out)
}

pub fn pick_sessions(eligible: &[i32], n: usize, seed: u64) -> Vec<i32> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut v = eligible.to_vec();
    v.shuffle(&mut rng);
    v.truncate(n);
    v.sort_unstable();
    v
}

fn draw(
    b: &CoarseBars,
    r: Range<usize>,
    events: &[&Event],
    exclude_minutes: u32,
    path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let n = r.len();
    let lo = r.clone().map(|i| b.low[i]).fold(f64::INFINITY, f64::min);
    let hi = r
        .clone()
        .map(|i| b.high[i])
        .fold(f64::NEG_INFINITY, f64::max);
    let pad = (hi - lo) * 0.04;
    let first_minute = i64::from(b.minute[r.start]);
    let width = i64::from(b.width);
    let root = BitMapBackend::new(path, (1500, 760)).into_drawing_area();
    root.fill(&WHITE)?;
    let title = format!(
        "{} anchored VWAP, {} minute bars, {} events",
        date_of(b.session[r.start]),
        b.width,
        events.len()
    );
    let mut chart = ChartBuilder::on(&root)
        .caption(title, ("sans-serif", 22))
        .margin(12)
        .x_label_area_size(36)
        .y_label_area_size(70)
        .build_cartesian_2d(0f64..(n as f64), (lo - pad)..(hi + pad))?;
    chart
        .configure_mesh()
        .x_labels(12)
        .x_label_formatter(&|x| clock(first_minute + (*x as i64) * width))
        .light_line_style(RGBColor(235, 235, 235))
        .draw()?;
    // exclusion window
    let excl_bars = f64::from(exclude_minutes) / f64::from(b.width);
    chart.draw_series(std::iter::once(Rectangle::new(
        [(0.0, lo - pad), (excl_bars, hi + pad)],
        RGBColor(220, 220, 220).mix(0.6).filled(),
    )))?;
    let up = RGBColor(31, 119, 180);
    let down = RGBColor(120, 120, 120);
    chart.draw_series(r.clone().enumerate().map(|(k, i)| {
        CandleStick::new(
            k as f64 + 0.5,
            b.open[i],
            b.high[i],
            b.low[i],
            b.close[i],
            up.filled(),
            down.filled(),
            4,
        )
    }))?;
    let vwap: Vec<(f64, f64)> = r
        .clone()
        .enumerate()
        .filter(|(_, i)| b.vwap[*i].is_finite())
        .map(|(k, i)| (k as f64 + 0.5, b.vwap[i]))
        .collect();
    chart
        .draw_series(LineSeries::new(vwap, RGBColor(214, 39, 40).stroke_width(2)))?
        .label("anchored VWAP")
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], RGBColor(214, 39, 40)));
    let from_above = RGBColor(0, 0, 0);
    let from_below = RGBColor(255, 127, 14);
    for (flag, col, name) in [
        (1i8, from_above, "approach from above"),
        (-1, from_below, "approach from below"),
    ] {
        chart
            .draw_series(events.iter().filter(|e| e.side == flag).map(|e| {
                let k = (e.bar - r.start) as f64 + 0.5;
                Circle::new((k, e.vwap), 7, col.stroke_width(2))
            }))?
            .label(name)
            .legend(move |(x, y)| Circle::new((x + 10, y), 5, col.stroke_width(2)));
    }
    chart
        .configure_series_labels()
        .background_style(WHITE.mix(0.8))
        .border_style(BLACK)
        .position(SeriesLabelPosition::UpperLeft)
        .draw()?;
    root.present()?;
    Ok(())
}
