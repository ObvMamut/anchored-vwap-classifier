//! Plots of a strategy run: reliability diagram, equity and drawdown, importance bars with the
//! spread over folds, and a hand drawn beeswarm of the rejection contributions. Every plot is
//! built from a plain data struct by a pure function, which the tests check; the pixels are
//! not tested. See `docs/MODELING.md` section 7 and `docs/BACKTEST.md` section 5.
// The pure builders index vectors whose lengths they have just checked or constructed.
#![allow(clippy::indexing_slicing)]

use std::fs;
use std::path::Path;

use plotters::prelude::*;
use plotters::style::text_anchor::{HPos, Pos, VPos};
use plotters::style::{FontStyle, register_font};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

use crate::error::ReportError;

/// Registers the first readable font as `sans-serif`; the chart text needs one.
pub fn register_fonts(font_paths: &[String]) -> Result<(), ReportError> {
    for path in font_paths {
        if let Ok(bytes) = fs::read(path) {
            let leaked: &'static [u8] = Box::leak(bytes.into_boxed_slice());
            if register_font("sans-serif", FontStyle::Normal, leaked).is_ok() {
                return Ok(());
            }
        }
    }
    Err(ReportError::NoFont(font_paths.join(", ")))
}

fn draw_err(e: impl std::fmt::Display) -> ReportError {
    ReportError::Draw(e.to_string())
}

// ---------------------------------------------------------------------------------------------
// data

/// One class of the reliability diagram.
#[derive(Debug, Clone, PartialEq)]
pub struct Curve {
    pub label: String,
    /// `(mean predicted probability, observed frequency, events)` per bin.
    pub points: Vec<(f64, f64, usize)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EquityData {
    /// Cumulative net USD per contract after each day.
    pub equity: Vec<f64>,
    /// Running peak minus equity, at least zero.
    pub drawdown: Vec<f64>,
    /// Day indices where a test fold begins.
    pub boundaries: Vec<usize>,
    pub max_drawdown: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImportanceBar {
    pub feature: String,
    pub mean: f64,
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SwarmPoint {
    pub phi: f64,
    /// Relative feature value in `[0, 1]`; NaN for a missing value.
    pub color: f64,
    /// Vertical offset inside the row, in `[-0.35, 0.35]`.
    pub jitter: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SwarmRow {
    pub feature: String,
    pub points: Vec<SwarmPoint>,
}

/// One bar of the win rate plot: the win rate of a group of trades and the win rate it needs to
/// break even after costs.
#[derive(Debug, Clone, PartialEq)]
pub struct WinBar {
    pub label: String,
    pub win: f64,
    pub breakeven: f64,
    pub trades: usize,
}

// ---------------------------------------------------------------------------------------------
// pure builders

/// Bars from `(label, win rate, break even win rate, trades)`; groups without a finite value are
/// left out.
pub fn winrate_bars(rows: &[(String, f64, f64, usize)]) -> Vec<WinBar> {
    rows.iter()
        .filter(|r| r.1.is_finite() && r.2.is_finite())
        .map(|r| WinBar {
            label: r.0.clone(),
            win: r.1,
            breakeven: r.2,
            trades: r.3,
        })
        .collect()
}

pub fn curves(bins_per_class: &[Vec<(f64, f64, usize)>], names: &[&str]) -> Vec<Curve> {
    bins_per_class
        .iter()
        .zip(names)
        .map(|(b, n)| Curve {
            label: (*n).to_string(),
            points: b.clone(),
        })
        .collect()
}

/// Cumulative equity and drawdown of a daily USD series.
pub fn equity_data(daily: &[f64], fold_first_day: &[usize]) -> EquityData {
    let mut equity = Vec::with_capacity(daily.len());
    let mut drawdown = Vec::with_capacity(daily.len());
    let (mut e, mut peak, mut worst) = (0.0f64, 0.0f64, 0.0f64);
    for d in daily {
        e += d;
        peak = peak.max(e);
        equity.push(e);
        drawdown.push(peak - e);
        worst = worst.max(peak - e);
    }
    EquityData {
        equity,
        drawdown,
        boundaries: fold_first_day.to_vec(),
        max_drawdown: worst,
    }
}

/// The `top` features by mean importance over the folds, with the fold minimum and maximum.
/// `per_fold[k][f]` is the importance of feature `f` in fold `k`.
pub fn importance_bars(names: &[String], per_fold: &[Vec<f64>], top: usize) -> Vec<ImportanceBar> {
    let mut bars: Vec<ImportanceBar> = names
        .iter()
        .enumerate()
        .filter_map(|(f, n)| {
            let v: Vec<f64> = per_fold.iter().filter_map(|k| k.get(f).copied()).collect();
            if v.is_empty() {
                return None;
            }
            Some(ImportanceBar {
                feature: n.clone(),
                mean: v.iter().sum::<f64>() / v.len() as f64,
                min: v.iter().copied().fold(f64::INFINITY, f64::min),
                max: v.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            })
        })
        .collect();
    bars.sort_by(|a, b| b.mean.total_cmp(&a.mean).then(a.feature.cmp(&b.feature)));
    bars.truncate(top);
    bars
}

/// Rows of the beeswarm for the `top` features by mean absolute contribution. Feature values
/// are scaled to `[0, 1]` between their 5th and 95th percentile; the vertical jitter is drawn
/// from a seeded generator, so the picture is reproducible.
pub fn beeswarm_rows(
    names: &[String],
    values: &[Vec<f64>],
    phis: &[Vec<f64>],
    top: usize,
    seed: u64,
) -> Vec<SwarmRow> {
    let n = values.len().min(phis.len());
    let mut order: Vec<(usize, f64)> = (0..names.len())
        .map(|f| {
            let m = (0..n)
                .map(|i| phis[i].get(f).copied().unwrap_or(0.0).abs())
                .sum::<f64>()
                / n.max(1) as f64;
            (f, m)
        })
        .collect();
    order.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    order.truncate(top);
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    order
        .into_iter()
        .map(|(f, _)| {
            let col: Vec<f64> = (0..n)
                .map(|i| values[i].get(f).copied().unwrap_or(f64::NAN))
                .collect();
            let mut finite: Vec<f64> = col.iter().copied().filter(|v| v.is_finite()).collect();
            finite.sort_by(f64::total_cmp);
            let q = |p: f64| {
                if finite.is_empty() {
                    0.0
                } else {
                    finite[((finite.len() - 1) as f64 * p).round() as usize]
                }
            };
            let (lo, hi) = (q(0.05), q(0.95));
            let points = (0..n)
                .map(|i| {
                    let v = col[i];
                    let color = if !v.is_finite() {
                        f64::NAN
                    } else if hi > lo {
                        ((v - lo) / (hi - lo)).clamp(0.0, 1.0)
                    } else {
                        0.5
                    };
                    SwarmPoint {
                        phi: phis[i].get(f).copied().unwrap_or(0.0),
                        color,
                        jitter: (rng.random::<f64>() - 0.5) * 0.7,
                    }
                })
                .collect();
            SwarmRow {
                feature: names.get(f).cloned().unwrap_or_default(),
                points,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// drawing

const BLUE: RGBColor = RGBColor(31, 119, 180);
const ORANGE: RGBColor = RGBColor(255, 127, 14);
const GREEN: RGBColor = RGBColor(44, 160, 44);
const RED: RGBColor = RGBColor(214, 39, 40);
const GREY: RGBColor = RGBColor(140, 140, 140);

pub fn draw_reliability(
    path: &Path,
    title: &str,
    curves: &[Curve],
    size: (u32, u32),
) -> Result<(), ReportError> {
    let root = BitMapBackend::new(path, size).into_drawing_area();
    root.fill(&WHITE).map_err(draw_err)?;
    let mut chart = ChartBuilder::on(&root)
        .caption(title, ("sans-serif", 22))
        .margin(14)
        .x_label_area_size(40)
        .y_label_area_size(55)
        .build_cartesian_2d(0f64..1f64, 0f64..1f64)
        .map_err(draw_err)?;
    chart
        .configure_mesh()
        .x_desc("predicted probability")
        .y_desc("observed frequency")
        .light_line_style(RGBColor(235, 235, 235))
        .draw()
        .map_err(draw_err)?;
    chart
        .draw_series(LineSeries::new(
            vec![(0.0, 0.0), (1.0, 1.0)],
            GREY.stroke_width(1),
        ))
        .map_err(draw_err)?
        .label("perfect")
        .legend(|(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], GREY));
    for (c, col) in curves.iter().zip([BLUE, ORANGE, GREEN]) {
        chart
            .draw_series(LineSeries::new(
                c.points.iter().map(|p| (p.0, p.1)),
                col.stroke_width(2),
            ))
            .map_err(draw_err)?
            .label(c.label.clone())
            .legend(move |(x, y)| PathElement::new(vec![(x, y), (x + 20, y)], col.stroke_width(3)));
        chart
            .draw_series(
                c.points
                    .iter()
                    .map(|p| Circle::new((p.0, p.1), 4, col.filled())),
            )
            .map_err(draw_err)?;
    }
    chart
        .configure_series_labels()
        .background_style(WHITE.mix(0.8))
        .border_style(BLACK)
        .position(SeriesLabelPosition::UpperLeft)
        .draw()
        .map_err(draw_err)?;
    root.present().map_err(draw_err)
}

pub fn draw_equity(
    path: &Path,
    title: &str,
    d: &EquityData,
    size: (u32, u32),
) -> Result<(), ReportError> {
    let root = BitMapBackend::new(path, size).into_drawing_area();
    root.fill(&WHITE).map_err(draw_err)?;
    let (top, bottom) = root.split_vertically(size.1 * 2 / 3);
    let n = d.equity.len().max(2) as f64;
    let (lo, hi) = d
        .equity
        .iter()
        .fold((0.0f64, 0.0f64), |(l, h), &e| (l.min(e), h.max(e)));
    let pad = ((hi - lo) * 0.05).max(1.0);
    let mut chart = ChartBuilder::on(&top)
        .caption(title, ("sans-serif", 22))
        .margin(14)
        .y_label_area_size(70)
        .build_cartesian_2d(0f64..n, (lo - pad)..(hi + pad))
        .map_err(draw_err)?;
    chart
        .configure_mesh()
        .y_desc("net USD per contract")
        .light_line_style(RGBColor(235, 235, 235))
        .draw()
        .map_err(draw_err)?;
    chart
        .draw_series(LineSeries::new(
            d.equity.iter().enumerate().map(|(i, e)| (i as f64, *e)),
            BLUE.stroke_width(2),
        ))
        .map_err(draw_err)?;
    for &b in &d.boundaries {
        chart
            .draw_series(LineSeries::new(
                vec![(b as f64, lo - pad), (b as f64, hi + pad)],
                GREY.stroke_width(1),
            ))
            .map_err(draw_err)?;
    }
    let worst = d.drawdown.iter().copied().fold(0.0f64, f64::max).max(1.0);
    let mut dd = ChartBuilder::on(&bottom)
        .margin(14)
        .x_label_area_size(36)
        .y_label_area_size(70)
        .build_cartesian_2d(0f64..n, worst * 1.05..0f64)
        .map_err(draw_err)?;
    dd.configure_mesh()
        .x_desc("test day")
        .y_desc("drawdown, USD")
        .light_line_style(RGBColor(235, 235, 235))
        .draw()
        .map_err(draw_err)?;
    dd.draw_series(AreaSeries::new(
        d.drawdown.iter().enumerate().map(|(i, e)| (i as f64, *e)),
        0.0,
        RED.mix(0.35),
    ))
    .map_err(draw_err)?;
    root.present().map_err(draw_err)
}

pub fn draw_winrate(
    path: &Path,
    title: &str,
    bars: &[WinBar],
    size: (u32, u32),
) -> Result<(), ReportError> {
    let root = BitMapBackend::new(path, size).into_drawing_area();
    root.fill(&WHITE).map_err(draw_err)?;
    let n = bars.len().max(1);
    let lo = bars
        .iter()
        .flat_map(|b| [b.win, b.breakeven])
        .fold(0.5f64, f64::min)
        - 0.04;
    let hi = bars
        .iter()
        .flat_map(|b| [b.win, b.breakeven])
        .fold(0.5f64, f64::max)
        + 0.04;
    let (lo, hi) = (lo.max(0.0), hi.min(1.0));
    let mut chart = ChartBuilder::on(&root)
        .caption(title, ("sans-serif", 22))
        .margin(14)
        .x_label_area_size(46)
        .y_label_area_size(60)
        .build_cartesian_2d(0f64..n as f64, lo..hi)
        .map_err(draw_err)?;
    chart
        .configure_mesh()
        .disable_x_mesh()
        .disable_x_axis()
        .y_desc("win rate")
        .y_label_formatter(&|y| format!("{:.0}%", 100.0 * y))
        .light_line_style(RGBColor(235, 235, 235))
        .draw()
        .map_err(draw_err)?;
    // the coin flip line
    chart
        .draw_series(LineSeries::new(
            vec![(0.0, 0.5), (n as f64, 0.5)],
            GREY.stroke_width(1),
        ))
        .map_err(draw_err)?;
    for (i, b) in bars.iter().enumerate() {
        let x0 = i as f64 + 0.15;
        let x1 = i as f64 + 0.85;
        chart
            .draw_series(std::iter::once(Rectangle::new(
                [(x0, lo), (x1, b.win)],
                BLUE.mix(0.75).filled(),
            )))
            .map_err(draw_err)?;
        // the break even win rate as a red mark across the bar
        chart
            .draw_series(LineSeries::new(
                vec![(x0 - 0.05, b.breakeven), (x1 + 0.05, b.breakeven)],
                RED.stroke_width(3),
            ))
            .map_err(draw_err)?;
        let (px, py) = chart.backend_coord(&(i as f64 + 0.5, lo));
        let style = ("sans-serif", 14)
            .into_font()
            .color(&BLACK)
            .pos(Pos::new(HPos::Center, VPos::Top));
        root.draw(&Text::new(b.label.clone(), (px, py + 6), style.clone()))
            .map_err(draw_err)?;
        root.draw(&Text::new(format!("n {}", b.trades), (px, py + 22), style))
            .map_err(draw_err)?;
    }
    root.present().map_err(draw_err)
}

pub fn draw_importance(
    path: &Path,
    title: &str,
    bars: &[ImportanceBar],
    size: (u32, u32),
) -> Result<(), ReportError> {
    let root = BitMapBackend::new(path, size).into_drawing_area();
    root.fill(&WHITE).map_err(draw_err)?;
    let n = bars.len().max(1);
    let hi = bars.iter().map(|b| b.max).fold(0.0f64, f64::max).max(1e-9) * 1.1;
    let mut chart = ChartBuilder::on(&root)
        .caption(title, ("sans-serif", 22))
        .margin(14)
        .x_label_area_size(40)
        .y_label_area_size(190)
        .build_cartesian_2d(0f64..hi, 0f64..n as f64)
        .map_err(draw_err)?;
    chart
        .configure_mesh()
        .disable_y_mesh()
        .y_labels(n)
        .y_label_formatter(&|y| {
            // the first bar is drawn at the top
            let k = n as f64 - 0.5 - *y;
            let i = k.round();
            if (k - i).abs() < 0.25 && i >= 0.0 {
                bars.get(i as usize)
                    .map(|b| b.feature.clone())
                    .unwrap_or_default()
            } else {
                String::new()
            }
        })
        .x_desc("mean absolute contribution (spread: fold minimum to maximum)")
        .light_line_style(RGBColor(235, 235, 235))
        .draw()
        .map_err(draw_err)?;
    for (i, b) in bars.iter().enumerate() {
        let y = n as f64 - 1.0 - i as f64;
        chart
            .draw_series(std::iter::once(Rectangle::new(
                [(0.0, y + 0.15), (b.mean, y + 0.85)],
                BLUE.mix(0.7).filled(),
            )))
            .map_err(draw_err)?;
        chart
            .draw_series(LineSeries::new(
                vec![(b.min, y + 0.5), (b.max, y + 0.5)],
                BLACK.stroke_width(2),
            ))
            .map_err(draw_err)?;
    }
    // the feature names, right aligned left of the plot area
    let style = ("sans-serif", 16)
        .into_font()
        .color(&BLACK)
        .pos(Pos::new(HPos::Right, VPos::Center));
    for (i, b) in bars.iter().enumerate() {
        let (px, py) = chart.backend_coord(&(0.0, n as f64 - 0.5 - i as f64));
        root.draw(&Text::new(b.feature.clone(), (px - 8, py), style.clone()))
            .map_err(draw_err)?;
    }
    root.present().map_err(draw_err)
}

pub fn draw_beeswarm(
    path: &Path,
    title: &str,
    rows: &[SwarmRow],
    size: (u32, u32),
) -> Result<(), ReportError> {
    let root = BitMapBackend::new(path, size).into_drawing_area();
    root.fill(&WHITE).map_err(draw_err)?;
    let n = rows.len().max(1);
    let m = rows
        .iter()
        .flat_map(|r| r.points.iter().map(|p| p.phi.abs()))
        .fold(0.0f64, f64::max)
        .max(1e-9)
        * 1.1;
    let mut chart = ChartBuilder::on(&root)
        .caption(title, ("sans-serif", 22))
        .margin(14)
        .x_label_area_size(40)
        .y_label_area_size(190)
        .build_cartesian_2d(-m..m, 0f64..n as f64)
        .map_err(draw_err)?;
    chart
        .configure_mesh()
        .disable_y_mesh()
        .y_labels(n)
        .y_label_formatter(&|y| {
            let k = n as f64 - 0.5 - *y;
            let i = k.round();
            if (k - i).abs() < 0.25 && i >= 0.0 {
                rows.get(i as usize)
                    .map(|r| r.feature.clone())
                    .unwrap_or_default()
            } else {
                String::new()
            }
        })
        .x_desc("contribution to the rejection score (blue low, red high feature value)")
        .light_line_style(RGBColor(235, 235, 235))
        .draw()
        .map_err(draw_err)?;
    for (i, r) in rows.iter().enumerate() {
        let y = n as f64 - 0.5 - i as f64;
        chart
            .draw_series(r.points.iter().map(|p| {
                let col = if p.color.is_nan() {
                    GREY
                } else {
                    RGBColor((255.0 * p.color) as u8, 60, (255.0 * (1.0 - p.color)) as u8)
                };
                Circle::new((p.phi, y + p.jitter), 3, col.filled())
            }))
            .map_err(draw_err)?;
    }
    let style = ("sans-serif", 16)
        .into_font()
        .color(&BLACK)
        .pos(Pos::new(HPos::Right, VPos::Center));
    for (i, r) in rows.iter().enumerate() {
        let (px, py) = chart.backend_coord(&(-m, n as f64 - 0.5 - i as f64));
        root.draw(&Text::new(r.feature.clone(), (px - 8, py), style.clone()))
            .map_err(draw_err)?;
    }
    root.present().map_err(draw_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equity_and_drawdown_match_a_hand_series() {
        let d = equity_data(&[10.0, -5.0, -10.0, 20.0, -2.0], &[0, 3]);
        assert_eq!(d.equity, vec![10.0, 5.0, -5.0, 15.0, 13.0]);
        assert_eq!(d.drawdown, vec![0.0, 5.0, 15.0, 0.0, 2.0]);
        assert_eq!(d.max_drawdown, 15.0);
        assert_eq!(d.boundaries, vec![0, 3]);
        let e = equity_data(&[], &[]);
        assert!(e.equity.is_empty() && e.max_drawdown == 0.0);
    }

    #[test]
    fn importance_bars_rank_by_mean_and_keep_the_fold_spread() {
        let names: Vec<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
        let per_fold = vec![
            vec![1.0, 5.0, 2.0],
            vec![3.0, 4.0, 2.0],
            vec![2.0, 6.0, 2.0],
        ];
        let bars = importance_bars(&names, &per_fold, 2);
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].feature, "b");
        assert!((bars[0].mean - 5.0).abs() < 1e-12);
        assert_eq!((bars[0].min, bars[0].max), (4.0, 6.0));
        // a and c tie on the mean 2.0; the name breaks the tie
        assert_eq!(bars[1].feature, "a");
    }

    #[test]
    fn the_beeswarm_orders_features_scales_values_and_is_reproducible() {
        let names: Vec<String> = ["small", "big", "flat"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let values = vec![
            vec![0.0, 10.0, 1.0],
            vec![1.0, 20.0, 1.0],
            vec![2.0, f64::NAN, 1.0],
            vec![3.0, 40.0, 1.0],
        ];
        let phis = vec![
            vec![0.1, -3.0, 0.0],
            vec![-0.1, 2.0, 0.0],
            vec![0.2, 1.0, 0.0],
            vec![0.0, -4.0, 0.0],
        ];
        let rows = beeswarm_rows(&names, &values, &phis, 2, 9);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].feature, "big");
        assert_eq!(rows[1].feature, "small");
        assert!(rows[0].points[2].color.is_nan());
        assert!(
            rows[0]
                .points
                .iter()
                .all(|p| p.color.is_nan() || (0.0..=1.0).contains(&p.color))
        );
        assert!(rows[0].points.iter().all(|p| p.jitter.abs() <= 0.35));
        // the lowest value of a feature is at the cold end, the highest at the hot end
        assert!(rows[1].points[0].color < rows[1].points[3].color);
        let again = beeswarm_rows(&names, &values, &phis, 2, 9);
        for (a, b) in rows.iter().zip(&again) {
            assert_eq!(a.feature, b.feature);
            for (p, q) in a.points.iter().zip(&b.points) {
                assert_eq!(p.phi.to_bits(), q.phi.to_bits());
                assert_eq!(p.color.to_bits(), q.color.to_bits());
                assert_eq!(p.jitter.to_bits(), q.jitter.to_bits());
            }
        }
        // a constant feature does not divide by zero
        let flat = beeswarm_rows(&names, &values, &phis, 3, 9);
        assert!(
            flat.iter()
                .find(|r| r.feature == "flat")
                .unwrap()
                .points
                .iter()
                .all(|p| p.color == 0.5)
        );
    }

    #[test]
    fn winrate_bars_drop_groups_without_a_value() {
        let rows = vec![
            ("5m fold 1".to_string(), 0.488, 0.555, 1077),
            ("empty".to_string(), f64::NAN, 0.53, 0),
            ("15m all".to_string(), 0.48, 0.519, 2764),
        ];
        let b = winrate_bars(&rows);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].label, "5m fold 1");
        assert_eq!(b[1].trades, 2764);
        assert!(winrate_bars(&[]).is_empty());
    }

    #[test]
    fn curves_pair_bins_with_class_names() {
        let c = curves(
            &[vec![(0.1, 0.2, 5)], vec![]],
            &["rejection", "breakthrough"],
        );
        assert_eq!(c[0].label, "rejection");
        assert_eq!(c[0].points, vec![(0.1, 0.2, 5)]);
        assert!(c[1].points.is_empty());
    }

    #[test]
    fn all_four_plots_are_written_when_a_font_exists() {
        let fonts: Vec<String> = [
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        if register_fonts(&fonts).is_err() {
            eprintln!("no font on this machine: drawing test skipped");
            return;
        }
        let dir = std::env::temp_dir().join(format!("vwap_plots_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let size = (600, 400);
        let cv = curves(&[vec![(0.2, 0.25, 40), (0.6, 0.5, 40)]], &["rejection"]);
        draw_reliability(&dir.join("r.png"), "reliability", &cv, size).unwrap();
        let eq = equity_data(&[1.0, -2.0, 3.0, 4.0], &[0, 2]);
        draw_equity(&dir.join("e.png"), "equity", &eq, size).unwrap();
        let names: Vec<String> = vec!["a".into(), "b".into()];
        let bars = importance_bars(&names, &[vec![1.0, 2.0], vec![1.5, 1.0]], 2);
        draw_importance(&dir.join("i.png"), "importance", &bars, size).unwrap();
        let sw = beeswarm_rows(
            &names,
            &[vec![1.0, 2.0], vec![2.0, 1.0]],
            &[vec![0.5, -0.2], vec![-0.1, 0.4]],
            2,
            1,
        );
        draw_beeswarm(&dir.join("b.png"), "beeswarm", &sw, size).unwrap();
        let wb = winrate_bars(&[
            ("a".to_string(), 0.48, 0.53, 100),
            ("b".to_string(), 0.5, 0.52, 90),
        ]);
        draw_winrate(&dir.join("w.png"), "win rate", &wb, size).unwrap();
        for f in ["r", "e", "i", "b", "w"] {
            assert!(
                std::fs::metadata(dir.join(format!("{f}.png")))
                    .unwrap()
                    .len()
                    > 500
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
