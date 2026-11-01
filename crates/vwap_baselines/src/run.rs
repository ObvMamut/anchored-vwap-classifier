//! The Phase 3 stage: VWAP and control events through the same pipeline, class
//! frequencies with session clustered intervals, tests, and the gate.
#![allow(clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use chrono::{DateTime, Datelike};
use serde::Serialize;
use vwap_core::{BaselinesConfig, Class, DataConfig, EventsConfig, LabelsConfig};
use vwap_data::CleanBars;
use vwap_events::{CoarseBars, LevelSource, build, detect_level, event_params};
use vwap_labels::{LabelDrops, LabelParams, Labeled, label_events};

use crate::controls::{PriorSessionVwapCurve, RandomHorizontal, ShiftedAnchorVwap};
use crate::error::BaselinesError;
use crate::stats::{
    Estimates, NCLASS, Row, SessionTables, bootstrap, holm, simultaneous_ci, total_variation, wald,
};

#[derive(Debug, Clone, Serialize)]
pub struct GroupDesc {
    pub name: String,
    pub events: usize,
    pub ambiguous: usize,
    pub class_counts: [usize; NCLASS],
    pub per_year: BTreeMap<i32, usize>,
    pub mean_time_to_barrier_min: f64,
    pub mean_mae: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FamilyResult {
    pub family: String,
    pub width: u32,
    pub desc: GroupDesc,
    pub estimates: Estimates,
    pub vwap_ci_raw: [[f64; 2]; NCLASS],
    pub ctrl_ci_raw: [[f64; 2]; NCLASS],
    pub delta_ci: [[f64; 2]; NCLASS],
    /// 90 percent percentile interval of the rejection gap, for the equivalence verdict.
    pub rejection_gap_ci90: [f64; 2],
    /// EQUIVALENT, DIFFERENT or INCONCLUSIVE (gate 3a).
    pub verdict: String,
    pub total_variation: f64,
    pub wald: f64,
    pub p: f64,
    pub p_holm: f64,
    pub n_eff_vwap_rejection: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct GridCell {
    pub m1: f64,
    pub m2: f64,
    pub horizon: usize,
    pub events: usize,
    pub ambiguous_share: f64,
    pub freq: [f64; NCLASS],
    pub mean_time_to_barrier_min: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct WidthReport {
    pub width: u32,
    pub vwap: GroupDesc,
    pub label_drops: LabelDrops,
    pub vol_tercile_edges: [f64; 2],
    pub families: Vec<FamilyResult>,
    pub grid: Vec<GridCell>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FamilyGate {
    pub family: String,
    pub passes: bool,
    pub widths_significant_and_large: Vec<u32>,
    pub sign_consistent: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct GateReport {
    pub passed: bool,
    pub alpha: f64,
    pub delta_min: f64,
    pub families: Vec<FamilyGate>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BaselineReport {
    /// The window the statistics were computed on, in UTC.
    pub window_start_utc: String,
    pub window_end_utc: String,
    pub m1: f64,
    pub m2: f64,
    pub widths: Vec<WidthReport>,
    pub gate: GateReport,
}

fn year_of(us: i64) -> i32 {
    DateTime::from_timestamp_micros(us)
        .unwrap_or_default()
        .year()
}

fn describe(name: &str, all: &[Labeled]) -> GroupDesc {
    let kept: Vec<&Labeled> = all.iter().filter(|l| !l.ambiguous).collect();
    let mut counts = [0usize; NCLASS];
    let mut per_year = BTreeMap::new();
    for l in &kept {
        if let Some(c) = l.class {
            counts[c.index()] += 1;
        }
        *per_year.entry(year_of(l.event.open_us)).or_default() += 1;
    }
    let n = kept.len().max(1) as f64;
    GroupDesc {
        name: name.to_string(),
        events: kept.len(),
        ambiguous: all.len() - kept.len(),
        class_counts: counts,
        per_year,
        mean_time_to_barrier_min: kept
            .iter()
            .map(|l| f64::from(l.time_to_barrier_min))
            .sum::<f64>()
            / n,
        mean_mae: kept.iter().map(|l| l.mae).sum::<f64>() / n,
    }
}

/// Labels the events of a level series and keeps the development part.
fn labeled_dev(
    coarse: &CoarseBars,
    level: &[f64],
    params: &vwap_events::EventParams,
    lp: &LabelParams,
    one: &CleanBars,
    window: (i64, i64),
) -> (Vec<Labeled>, LabelDrops) {
    let (events, _) = detect_level(coarse, level, params);
    let (mut labeled, drops) = label_events(&events, coarse, one, lp);
    labeled.retain(|l| l.event.close_us >= window.0 && l.t_end_us < window.1);
    (labeled, drops)
}

fn stratum_edges(l: &[Labeled]) -> [f64; 2] {
    let mut v: Vec<f64> = l.iter().map(|x| x.sigma / x.event.close).collect();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if v.is_empty() {
        return [0.0, 0.0];
    }
    [v[v.len() / 3], v[2 * v.len() / 3]]
}

/// Events reduced to (session, stratum, class). Strata are time of day slots of `stratum_minutes`
/// crossed with terciles of the volatility ratio.
fn rows(l: &[Labeled], coarse: &CoarseBars, edges: &[f64; 2], stratum_minutes: usize) -> Vec<Row> {
    l.iter()
        .filter_map(|x| {
            let class = x.class?.index();
            let slot = (usize::from(coarse.minute[x.event.bar]) + coarse.width as usize) % 1440
                / stratum_minutes;
            let r = x.sigma / x.event.close;
            let tercile = usize::from(r > edges[0]) + usize::from(r > edges[1]);
            Some(Row {
                session: x.event.session,
                stratum: slot * 3 + tercile,
                class,
            })
        })
        .collect()
}

fn hash_name(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
    })
}

#[allow(clippy::too_many_arguments)]
fn family_result(
    family: &str,
    width: u32,
    ctrl: &[Labeled],
    vwap_rows: &[Row],
    coarse: &CoarseBars,
    edges: &[f64; 2],
    base: &BaselinesConfig,
) -> FamilyResult {
    let stratum_minutes = base.stratum_minutes as usize;
    let ctrl_rows = rows(ctrl, coarse, edges, stratum_minutes);
    let n_strata = (1440 / stratum_minutes) * 3;
    let tables = SessionTables::build(n_strata, vwap_rows, &ctrl_rows);
    let seed = base.seed ^ hash_name(family) ^ u64::from(width);
    let min_n = u32::try_from(base.min_stratum_events).unwrap_or(30);
    let (point, draws) = bootstrap(&tables, base.bootstrap_draws, seed, min_n);
    let col = |f: &dyn Fn(&Estimates) -> f64| -> Vec<f64> { draws.iter().map(f).collect() };
    let ci = |f: &dyn Fn(&Estimates, usize) -> f64| -> [[f64; 2]; NCLASS] {
        let mut out = [[0.0; 2]; NCLASS];
        for (c, o) in out.iter_mut().enumerate() {
            let (lo, hi) = simultaneous_ci(&col(&|e| f(e, c)), base.alpha);
            *o = [lo, hi];
        }
        out
    };
    let delta_draws: Vec<[f64; NCLASS]> = draws.iter().map(|e| e.delta).collect();
    let (w, p) = wald(&point.delta, &delta_draws);
    let rej: Vec<f64> = col(&|e| e.pi_vwap_raw[0]);
    let m = rej.iter().sum::<f64>() / rej.len().max(1) as f64;
    let var = rej.iter().map(|x| (x - m) * (x - m)).sum::<f64>()
        / rej.len().saturating_sub(1).max(1) as f64;
    let pr = point.pi_vwap_raw[0];
    FamilyResult {
        family: family.to_string(),
        width,
        desc: describe(family, ctrl),
        estimates: point,
        vwap_ci_raw: ci(&|e, c| e.pi_vwap_raw[c]),
        ctrl_ci_raw: ci(&|e, c| e.pi_ctrl_raw[c]),
        delta_ci: ci(&|e, c| e.delta[c]),
        rejection_gap_ci90: {
            let sd = crate::stats::sorted(col(&|e| e.delta[0]));
            [
                crate::stats::percentile(&sd, 0.05),
                crate::stats::percentile(&sd, 0.95),
            ]
        },
        verdict: String::new(),
        total_variation: total_variation(&point.pi_vwap_adj, &point.pi_ctrl_adj),
        wald: w,
        p,
        p_holm: 1.0,
        n_eff_vwap_rejection: if var > 0.0 {
            pr * (1.0 - pr) / var
        } else {
            f64::NAN
        },
    }
}

pub fn run_baselines(
    data: &DataConfig,
    ev: &EventsConfig,
    lab: &LabelsConfig,
    base: &BaselinesConfig,
    window: (i64, i64),
    bars: &CleanBars,
) -> Result<BaselineReport, BaselinesError> {
    let params = event_params(data, ev);
    let mut widths = Vec::new();
    for &w in &ev.timeframes {
        let coarse = build(bars, data, w, ev.atr_length);
        let lp = LabelParams {
            m1: lab.m1,
            m2: lab.m2,
            horizon: lab.horizon(w),
        };
        let (vwap_all, vwap_drops) = labeled_dev(&coarse, &coarse.vwap, &params, &lp, bars, window);
        if vwap_all.len() < 100 {
            return Err(BaselinesError::NotEnoughData(format!(
                "only {} VWAP events on {w} minute bars",
                vwap_all.len()
            )));
        }
        let edges = stratum_edges(&vwap_all);
        let vwap_rows = rows(&vwap_all, &coarse, &edges, base.stratum_minutes as usize);

        let pool = |sources: Vec<Box<dyn LevelSource>>| -> Vec<Labeled> {
            sources
                .iter()
                .flat_map(|s| {
                    labeled_dev(&coarse, &s.levels(&coarse), &params, &lp, bars, window).0
                })
                .collect()
        };
        let random: Vec<Box<dyn LevelSource>> = (0..base.random_levels_per_session)
            .map(|k| {
                Box::new(RandomHorizontal {
                    seed: base.seed,
                    index: k,
                    z_max: base.random_level_z_max,
                }) as Box<dyn LevelSource>
            })
            .collect();
        let shifted: Vec<Box<dyn LevelSource>> = base
            .shifted_anchor_minutes
            .iter()
            .map(|&m| Box::new(ShiftedAnchorVwap { minute: m }) as Box<dyn LevelSource>)
            .collect();
        let prior: Vec<Box<dyn LevelSource>> = vec![Box::new(PriorSessionVwapCurve)];
        let mut families = Vec::new();
        for (name, sources) in [
            ("random_horizontal", random),
            ("shifted_anchor_vwap", shifted),
            ("prior_session_vwap", prior),
        ] {
            let ctrl = pool(sources);
            families.push(family_result(
                name, w, &ctrl, &vwap_rows, &coarse, &edges, base,
            ));
        }

        // Descriptive sensitivity grid on the VWAP events, development data only.
        let horizons = if w == 5 {
            &lab.grid.horizon_5m
        } else {
            &lab.grid.horizon_15m
        };
        let mut grid = Vec::new();
        for &m1 in &lab.grid.m1 {
            for &m2 in &lab.grid.m2 {
                for &h in horizons {
                    let cell = LabelParams { m1, m2, horizon: h };
                    let (l, _) = labeled_dev(&coarse, &coarse.vwap, &params, &cell, bars, window);
                    let d = describe("grid", &l);
                    let n = d.events.max(1) as f64;
                    grid.push(GridCell {
                        m1,
                        m2,
                        horizon: h,
                        events: d.events,
                        ambiguous_share: d.ambiguous as f64
                            / (d.events + d.ambiguous).max(1) as f64,
                        freq: [
                            d.class_counts[0] as f64 / n,
                            d.class_counts[1] as f64 / n,
                            d.class_counts[2] as f64 / n,
                        ],
                        mean_time_to_barrier_min: d.mean_time_to_barrier_min,
                    });
                }
            }
        }
        widths.push(WidthReport {
            width: w,
            vwap: describe("anchored_vwap", &vwap_all),
            label_drops: vwap_drops,
            vol_tercile_edges: edges,
            families,
            grid,
        });
    }

    // Holm correction across every (family, timeframe) test.
    let ps: Vec<f64> = widths
        .iter()
        .flat_map(|w| w.families.iter().map(|f| f.p))
        .collect();
    let adj = holm(&ps);
    let mut it = adj.into_iter();
    for w in &mut widths {
        for f in &mut w.families {
            f.p_holm = it.next().unwrap_or(1.0);
        }
    }
    for w in &mut widths {
        for f in &mut w.families {
            f.verdict = equivalence_verdict(f, base);
        }
    }
    let gate = evaluate_gate(&widths, base);
    let report = BaselineReport {
        window_start_utc: DateTime::from_timestamp_micros(window.0)
            .unwrap_or_default()
            .to_rfc3339(),
        window_end_utc: DateTime::from_timestamp_micros(window.1)
            .unwrap_or_default()
            .to_rfc3339(),
        m1: lab.m1,
        m2: lab.m2,
        widths,
        gate,
    };
    write_outputs(&report, base)?;
    Ok(report)
}

/// Gate 3a verdict from the 90 percent interval of the rejection gap and the margin delta_min.
pub fn equivalence_verdict(f: &FamilyResult, base: &BaselinesConfig) -> String {
    let gap = f.estimates.delta[Class::Rejection.index()];
    let [lo, hi] = f.rejection_gap_ci90;
    if f.p_holm < base.alpha && gap.abs() > base.delta_min {
        "DIFFERENT".into()
    } else if lo > -base.delta_min && hi < base.delta_min {
        "EQUIVALENT".into()
    } else {
        "INCONCLUSIVE".into()
    }
}

/// The first version of the gate, kept for the record: for every control family, at least one timeframe must have
/// a Holm adjusted p value below alpha and a rejection frequency gap above delta_min,
/// and the sign of that gap must agree across timeframes.
pub fn evaluate_gate(widths: &[WidthReport], base: &BaselinesConfig) -> GateReport {
    let names: Vec<String> = widths
        .first()
        .map(|w| w.families.iter().map(|f| f.family.clone()).collect())
        .unwrap_or_default();
    let mut families = Vec::new();
    for name in names {
        let rs: Vec<&FamilyResult> = widths
            .iter()
            .flat_map(|w| w.families.iter().filter(|f| f.family == name))
            .collect();
        let hits: Vec<u32> = rs
            .iter()
            .filter(|f| {
                f.p_holm < base.alpha
                    && f.estimates.delta[Class::Rejection.index()].abs() > base.delta_min
            })
            .map(|f| f.width)
            .collect();
        let signs: Vec<bool> = rs
            .iter()
            .map(|f| f.estimates.delta[Class::Rejection.index()] > 0.0)
            .collect();
        let sign_consistent = signs.windows(2).all(|p| p[0] == p[1]);
        families.push(FamilyGate {
            family: name,
            passes: !hits.is_empty() && sign_consistent,
            widths_significant_and_large: hits,
            sign_consistent,
        });
    }
    GateReport {
        passed: !families.is_empty() && families.iter().all(|f| f.passes),
        alpha: base.alpha,
        delta_min: base.delta_min,
        families,
    }
}

fn pct(x: f64) -> String {
    format!("{:.1} percent", x * 100.0)
}

pub fn render_markdown(r: &BaselineReport) -> String {
    let mut s = String::new();
    s.push_str("# Gate 3a, marginal equivalence report\n\n");
    s.push_str(&format!(
        "Window {} to {} (discovery segment). Label cell: m1 = {}, m2 = {}.\n\n",
        r.window_start_utc, r.window_end_utc, r.m1, r.m2
    ));
    s.push_str("Tier 3a is reported and never decisive. Verdicts use the 90 percent interval of the rejection gap against the margin delta_min: EQUIVALENT if the interval lies inside the margin, DIFFERENT if the Holm adjusted p is below alpha and the gap exceeds the margin, INCONCLUSIVE otherwise. Tier 3b, the research loop, decides whether the project continues.\n");
    for w in &r.widths {
        s.push_str(&format!("\n## {} minute bars\n\n", w.width));
        let n = w.vwap.events.max(1) as f64;
        s.push_str(&format!(
            "Anchored VWAP: {} events, ambiguous {}, rejection {}, breakthrough {}, chop {}. Mean time to barrier {:.1} minutes, mean MAE {:.2} sigma.\n\n",
            w.vwap.events,
            w.vwap.ambiguous,
            pct(w.vwap.class_counts[0] as f64 / n),
            pct(w.vwap.class_counts[1] as f64 / n),
            pct(w.vwap.class_counts[2] as f64 / n),
            w.vwap.mean_time_to_barrier_min,
            w.vwap.mean_mae
        ));
        for f in &w.families {
            let e = &f.estimates;
            s.push_str(&format!(
                "* {} ({} events, strata coverage {}): {}. VWAP rejection {} versus control {}, gap {}, 90 percent interval {} to {}. Wald p {:.4}, Holm adjusted p {:.4}.\n",
                f.family,
                f.desc.events,
                pct(e.coverage),
                f.verdict,
                pct(e.pi_vwap_adj[0]),
                pct(e.pi_ctrl_adj[0]),
                pct(e.delta[0]),
                pct(f.rejection_gap_ci90[0]),
                pct(f.rejection_gap_ci90[1]),
                f.p,
                f.p_holm
            ));
        }
    }
    s.push_str("\nThis tier tests unconditional frequencies, so it never ruled out a feature conditioned effect.\n");
    s
}

fn write_outputs(r: &BaselineReport, base: &BaselinesConfig) -> Result<(), BaselinesError> {
    for d in [&base.output_dir, &base.report_dir] {
        fs::create_dir_all(d).map_err(|source| BaselinesError::Io {
            path: d.clone(),
            source,
        })?;
    }
    let json = Path::new(&base.output_dir).join("baselines_report.json");
    fs::write(&json, serde_json::to_string_pretty(r)?).map_err(|source| BaselinesError::Io {
        path: json.display().to_string(),
        source,
    })?;
    let md = Path::new(&base.report_dir).join("gate_baselines.md");
    fs::write(&md, render_markdown(r)).map_err(|source| BaselinesError::Io {
        path: md.display().to_string(),
        source,
    })?;
    Ok(())
}
