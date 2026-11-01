//! `vwap report build`: the Markdown summary of the research record and the plots of every
//! strategy summary. The output directory is generated and ignored by git.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use vwap_report::plots;
use vwap_research::strategy::summary::PlotInputs;

use crate::ctx::guard_ledger;

const CLASSES: [&str; 3] = ["rejection", "breakthrough", "chop"];

pub fn build(config: &Path) -> Result<String> {
    let research = vwap_core::load_research_config(config)?;
    guard_ledger(&research)?;
    let rep = vwap_core::load_report_config(config)?;
    let events = vwap_core::load_events_config(config)?;
    let out = Path::new(&rep.output_dir);
    let plot_dir = out.join("plots");
    fs::create_dir_all(&plot_dir).with_context(|| format!("creating {}", plot_dir.display()))?;
    let md = vwap_research::strategy::summary::build(&research)?;
    let summary = out.join("summary.md");
    fs::write(&summary, md).with_context(|| format!("writing {}", summary.display()))?;
    let mut text = format!("wrote {}\n", summary.display());

    let mut files: Vec<_> = match fs::read_dir(&research.strategies_dir) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect(),
        Err(_) => Vec::new(),
    };
    files.sort();
    if files.is_empty() {
        text.push_str("no strategy summaries yet, so no plots\n");
        return Ok(text);
    }
    plots::register_fonts(&events.font_paths)?;
    let size = (rep.plot_width, rep.plot_height);
    for p in files {
        let Some(stem) = p.file_stem().and_then(|s| s.to_str()).map(str::to_string) else {
            continue;
        };
        let v: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&p).with_context(|| format!("reading {}", p.display()))?,
        )?;
        let pi = PlotInputs::from_summary(&v);
        let title = |what: &str| format!("{stem}: {what}");
        let curves = plots::curves(&pi.reliability, &CLASSES);
        plots::draw_reliability(
            &plot_dir.join(format!("{stem}_reliability.png")),
            &title("reliability on the out of fold forecasts"),
            &curves,
            size,
        )?;
        let eq = plots::equity_data(&pi.daily_pnl, &pi.fold_first_day);
        plots::draw_equity(
            &plot_dir.join(format!("{stem}_equity.png")),
            &title(&format!(
                "net equity, maximum drawdown {:.0} USD",
                eq.max_drawdown
            )),
            &eq,
            size,
        )?;
        let mut drawn = 2;
        if !pi.importance_by_fold.is_empty() {
            let bars =
                plots::importance_bars(&pi.feature_names, &pi.importance_by_fold, rep.top_features);
            plots::draw_importance(
                &plot_dir.join(format!("{stem}_importance.png")),
                &title("feature importance and its spread over folds"),
                &bars,
                size,
            )?;
            drawn += 1;
        }
        if !pi.shap_phi.is_empty() {
            let rows = plots::beeswarm_rows(
                &pi.feature_names,
                &pi.shap_values,
                &pi.shap_phi,
                rep.top_features,
                0,
            );
            plots::draw_beeswarm(
                &plot_dir.join(format!("{stem}_beeswarm.png")),
                &title("contributions to the rejection score"),
                &rows,
                size,
            )?;
            drawn += 1;
        }
        text.push_str(&format!("{stem}: {drawn} plots\n"));
    }
    Ok(text)
}
