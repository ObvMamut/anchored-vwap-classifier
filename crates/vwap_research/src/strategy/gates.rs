//! The promotion gates of a strategy (`docs/MODELING.md` section 6 and `docs/BACKTEST.md`
//! section 6) and the DSR and PBO bookkeeping over the counted trials.

use std::path::Path;

use serde::Serialize;
use vwap_core::ValidationConfig;
use vwap_validation::{dsr, expected_max_sharpe, kurtosis, pbo, sample_sd, sharpe, skewness};

use crate::error::ResearchError;
use crate::ledger::Verdict;
use crate::strategy::eval::StrategyResult;

/// True when `x` is not above `t`, a NaN included: a missing number never passes a gate.
fn not_above(x: f64, t: f64) -> bool {
    x.is_nan() || x <= t
}

/// Conditions that depend on this strategy alone. Returns the failed ones, in words; an empty
/// list means the predictive and economic gates hold.
pub fn own_failures(
    r: &StrategyResult,
    v: &ValidationConfig,
    min_trades_per_week: Option<f64>,
) -> Vec<String> {
    let mut f = Vec::new();
    let n = r.folds.len();
    let more_than_half = |k: usize| 2 * k > n;
    let Some(p) = &r.pooled else {
        f.push("no fold produced predictions".into());
        return f;
    };
    let bss_folds = r
        .folds
        .iter()
        .filter(|x| x.bss.is_some_and(|b| b > 0.0))
        .count();
    if !more_than_half(bss_folds) {
        f.push(format!(
            "BSS above zero in {bss_folds} of {n} folds, more than half are needed"
        ));
    }
    if not_above(p.bss_interval.0, 0.0) {
        f.push(format!(
            "pooled BSS interval [{:.4}, {:.4}] does not exclude zero",
            p.bss_interval.0, p.bss_interval.1
        ));
    }
    let prec_folds = r
        .folds
        .iter()
        .filter(
            |x| matches!((x.precision_at_tau, x.base_rejection_rate), (Some(a), Some(b)) if a > b),
        )
        .count();
    if !more_than_half(prec_folds) {
        f.push(format!(
            "rejection precision above the base rate in {prec_folds} of {n} folds, more than half are needed"
        ));
    }
    if let Some((k, e)) = p
        .ece
        .iter()
        .enumerate()
        .find(|(_, e)| e.is_nan() || **e > v.max_ece)
    {
        f.push(format!(
            "ECE of class {k} is {e:.4}, above {:.4}",
            v.max_ece
        ));
    }
    let pos_folds = r
        .folds
        .iter()
        .filter(|x| x.expectancy_net_ticks.is_some_and(|e| e > 0.0))
        .count();
    if (pos_folds as f64) < v.min_fold_fraction * n as f64 {
        f.push(format!(
            "net expectancy positive in {pos_folds} of {n} folds, at least {:.0} percent are needed",
            100.0 * v.min_fold_fraction
        ));
    }
    if not_above(r.expectancy_interval.0, 0.0) {
        f.push(format!(
            "pooled net expectancy lower bound {:.4} ticks is not above zero",
            r.expectancy_interval.0
        ));
    }
    if let Some(rate) = min_trades_per_week
        && (r.trades_per_week.is_nan() || r.trades_per_week < rate)
    {
        f.push(format!(
            "{:.2} trades per week over {:.0} weeks, at least {rate:.1} are required",
            r.trades_per_week, r.weeks
        ));
    }
    if not_above(r.net_at_2_ticks, 0.0) {
        f.push(format!(
            "net expectancy at 2 ticks of slippage per side is {:.4}, not positive",
            r.net_at_2_ticks
        ));
    }
    f
}

/// A daily net PnL series of one counted trial.
#[derive(Debug, Clone)]
pub struct Series {
    pub key: String,
    pub daily_pnl: Vec<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DsrPbo {
    /// Counted trials of the ledger (hypothesis cards included, which have no PnL series).
    pub n_trials: usize,
    /// Trials with a PnL series.
    pub n_series: usize,
    pub sharpe_daily: f64,
    pub variance_of_sharpes: f64,
    pub expected_max_sharpe: f64,
    pub dsr: Option<f64>,
    pub pbo: Option<f64>,
    pub notes: Vec<String>,
}

/// DSR of `own` and the PBO of all series, with the trial count of the ledger. The variance of
/// the Sharpe ratios across trials comes from the series; with a single series it falls back to
/// the null variance $1/(T-1)$.
pub fn dsr_pbo(own: &Series, others: &[Series], n_trials: usize, v: &ValidationConfig) -> DsrPbo {
    let mut notes = Vec::new();
    let t = own.daily_pnl.len();
    let sr = sharpe(&own.daily_pnl);
    let mut srs: Vec<f64> = others
        .iter()
        .filter(|s| s.key != own.key)
        .map(|s| sharpe(&s.daily_pnl))
        .collect();
    srs.push(sr);
    let n_series = srs.len();
    let var = if srs.len() >= 2 {
        let m = srs.iter().sum::<f64>() / srs.len() as f64;
        srs.iter().map(|x| (x - m) * (x - m)).sum::<f64>() / (srs.len() - 1) as f64
    } else {
        notes.push(
            "a single series: the variance of Sharpe ratios is the null value 1/(T-1)".into(),
        );
        1.0 / (t.max(2) - 1) as f64
    };
    let n = n_trials.max(n_series);
    let d = if t >= 2 {
        Some(dsr(
            sr,
            var,
            n,
            t,
            skewness(&own.daily_pnl),
            kurtosis(&own.daily_pnl),
        ))
    } else {
        notes.push("fewer than 2 days: DSR undefined".into());
        None
    };
    // PBO over the series that share the calendar of `own`. Series that abstained throughout are
    // genuine trials with zero PnL and stay in. A strategy that itself never traded has nothing
    // to assess: its zeros tie with every other idle series and the ranks would be arbitrary.
    let own_traded = sample_sd(&own.daily_pnl) > 0.0;
    let mut cols: Vec<&Series> = others
        .iter()
        .filter(|s| s.key != own.key && s.daily_pnl.len() == t)
        .collect();
    cols.push(own);
    let skipped = n_series - cols.len();
    if skipped > 0 {
        notes.push(format!(
            "{skipped} series on a different calendar were left out of the PBO"
        ));
    }
    let matrix: Vec<Vec<f64>> = (0..t)
        .map(|i| {
            cols.iter()
                .map(|s| s.daily_pnl.get(i).copied().unwrap_or(0.0))
                .collect()
        })
        .collect();
    let pbo_value = if !own_traded {
        notes.push("PBO undefined: this strategy made no trades".into());
        None
    } else if cols.len() < v.pbo_min_trials {
        notes.push(format!(
            "PBO undefined: {} series, at least {} are needed",
            cols.len(),
            v.pbo_min_trials
        ));
        None
    } else {
        let r = pbo(&matrix, v.pbo_blocks);
        if r.is_none() {
            notes.push("PBO undefined: too few days for the number of blocks".into());
        }
        r.map(|x| x.pbo)
    };
    DsrPbo {
        n_trials: n,
        n_series,
        sharpe_daily: sr,
        variance_of_sharpes: var,
        expected_max_sharpe: expected_max_sharpe(var, n),
        dsr: d,
        pbo: pbo_value,
        notes,
    }
}

/// PROMOTE only if no gate failed and the DSR and PBO limits hold.
pub fn decide(
    mut failures: Vec<String>,
    d: &DsrPbo,
    v: &ValidationConfig,
) -> (Verdict, Vec<String>) {
    match d.dsr {
        Some(x) if x >= v.dsr_min => {}
        Some(x) => failures.push(format!("DSR {x:.4} is below {:.2}", v.dsr_min)),
        None => failures.push("DSR is undefined".into()),
    }
    match d.pbo {
        Some(x) if x <= v.pbo_max => {}
        Some(x) => failures.push(format!("PBO {x:.4} is above {:.2}", v.pbo_max)),
        None => failures.push("PBO is undefined".into()),
    }
    let verdict = if failures.is_empty() {
        Verdict::Promote
    } else {
        Verdict::Drop
    };
    (verdict, failures)
}

/// Reads the daily PnL series out of the summaries `<dir>/<id>_<w>m.json`, in file name order.
/// A summary is `{ "result": { "daily_pnl": [...] }, ... }`.
pub fn read_series(dir: &Path) -> Result<Vec<Series>, ResearchError> {
    let mut files: Vec<std::path::PathBuf> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect(),
        Err(_) => Vec::new(),
    };
    files.sort();
    let mut out = Vec::new();
    for p in files {
        let text = std::fs::read_to_string(&p).map_err(|source| ResearchError::Io {
            path: p.display().to_string(),
            source,
        })?;
        let v: serde_json::Value = serde_json::from_str(&text)?;
        let pnl: Vec<f64> = v
            .pointer("/result/daily_pnl")
            .and_then(serde_json::Value::as_array)
            .map(|a| a.iter().filter_map(serde_json::Value::as_f64).collect())
            .unwrap_or_default();
        if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
            out.push(Series {
                key: stem.to_string(),
                daily_pnl: pnl,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{Rng, SeedableRng};
    use rand_chacha::ChaCha8Rng;

    fn v() -> ValidationConfig {
        vwap_core::load_validation_config(Path::new("../../config")).unwrap()
    }

    fn noise(seed: u64, n: usize, drift: f64) -> Vec<f64> {
        let mut r = ChaCha8Rng::seed_from_u64(seed);
        (0..n)
            .map(|_| drift + (r.random::<f64>() - 0.5) * 100.0)
            .collect()
    }

    fn series(key: &str, x: Vec<f64>) -> Series {
        Series {
            key: key.into(),
            daily_pnl: x,
        }
    }

    #[test]
    fn a_single_noise_series_cannot_pass() {
        let own = series("S001_15m", noise(1, 600, 0.0));
        let d = dsr_pbo(&own, &[], 8, &v());
        assert!(d.dsr.unwrap() < 0.95);
        assert!(d.pbo.is_none());
        let (verdict, why) = decide(vec![], &d, &v());
        assert_eq!(verdict, Verdict::Drop);
        assert!(why.iter().any(|w| w.contains("PBO is undefined")));
    }

    #[test]
    fn a_dominant_series_among_noise_passes_both_limits() {
        let mut others: Vec<Series> = (0..5)
            .map(|k| series(&format!("S00{k}_15m"), noise(10 + k as u64, 600, 0.0)))
            .collect();
        let own = series("S009_15m", noise(99, 600, 40.0));
        others.push(own.clone());
        let d = dsr_pbo(&own, &others, 9, &v());
        assert!(d.dsr.unwrap() >= 0.95, "{d:?}");
        assert!(d.pbo.unwrap() <= 0.2, "{d:?}");
        let (verdict, why) = decide(vec![], &d, &v());
        assert_eq!(verdict, Verdict::Promote, "{why:?}");
    }

    #[test]
    fn more_trials_make_the_dsr_harsher() {
        let own = series("S009_15m", noise(7, 600, 6.0));
        let few = dsr_pbo(&own, &[], 2, &v()).dsr.unwrap();
        let many = dsr_pbo(&own, &[], 200, &v()).dsr.unwrap();
        assert!(many < few);
    }

    #[test]
    fn series_on_a_different_calendar_are_left_out_of_the_pbo_with_a_note() {
        let own = series("S009_15m", noise(7, 600, 0.0));
        let mut others: Vec<Series> = (0..4)
            .map(|k| series(&format!("S00{k}_15m"), noise(20 + k as u64, 600, 0.0)))
            .collect();
        others.push(series("S100_5m", noise(5, 300, 0.0)));
        let d = dsr_pbo(&own, &others, 6, &v());
        assert!(d.notes.iter().any(|n| n.contains("different calendar")));
        assert!(d.pbo.is_some());
    }

    #[test]
    fn idle_series_stay_in_the_pbo_but_an_idle_strategy_has_no_pbo() {
        // one trading strategy among three that abstained throughout: a real trial set
        let own = series("S009_15m", noise(7, 600, 30.0));
        let mut others: Vec<Series> = (0..3)
            .map(|k| series(&format!("S00{k}_15m"), vec![0.0; 600]))
            .collect();
        others.push(own.clone());
        let d = dsr_pbo(&own, &others, 6, &v());
        assert!(d.pbo.unwrap() <= 0.2, "{d:?}");
        // a strategy with no trades has no PBO whatever the others did
        let idle = series("S010_15m", vec![0.0; 600]);
        let many: Vec<Series> = (0..5)
            .map(|k| series(&format!("S00{k}_15m"), noise(30 + k as u64, 600, 0.0)))
            .collect();
        let d = dsr_pbo(&idle, &many, 6, &v());
        assert!(d.pbo.is_none());
        assert!(d.notes.iter().any(|n| n.contains("made no trades")));
    }

    #[test]
    fn read_series_follows_file_name_order_and_ignores_other_files() {
        let dir = std::env::temp_dir().join(format!("vwap_series_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("S002_5m.json"),
            r#"{"result":{"daily_pnl":[1.0,2.0]}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("S001_15m.json"),
            r#"{"result":{"daily_pnl":[3.0]}}"#,
        )
        .unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        let s = read_series(&dir).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].key, "S001_15m");
        assert_eq!(s[1].daily_pnl, vec![1.0, 2.0]);
        assert!(read_series(&dir.join("missing")).unwrap().is_empty());
    }
}
