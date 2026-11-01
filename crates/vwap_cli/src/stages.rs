//! The data, events, labels, baselines and spotcheck stages. Each returns the text to print.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;

use crate::ctx::{load_clean, write_manifest};

pub fn data_clean(config: &Path) -> Result<String> {
    let cfg = vwap_core::load_data_config(config)?;
    let s = vwap_data::run_clean(&cfg)?;
    // `run_clean` wrote artifacts/data/manifest.json itself, including the clean_fingerprint
    // that every later stage verifies, so no second manifest is written over it.
    Ok(serde_json::to_string_pretty(&s)?)
}

pub fn data_levels(config: &Path) -> Result<String> {
    let cfg = vwap_core::load_data_config(config)?;
    let b = load_clean(&cfg)?.bars;
    let mut out = String::new();
    let mut last_year = i32::MIN;
    for (i, t) in b.ts_us.iter().enumerate() {
        let y = vwap_data::diag::utc(*t)
            .format("%Y")
            .to_string()
            .parse::<i32>()
            .unwrap_or(0);
        if y != last_year {
            out.push_str(&format!(
                "{y} first close {}\n",
                b.close.get(i).copied().unwrap_or(0.0)
            ));
            last_year = y;
        }
    }
    out.push_str(&format!(
        "rows {} last close {:?}\n",
        b.len(),
        b.close.last()
    ));
    Ok(out)
}

pub fn data_inspect(config: &Path) -> Result<String> {
    let cfg = vwap_core::load_data_config(config)?;
    let raw = vwap_data::read_raw(Path::new(&cfg.input_path))?;
    Ok(serde_json::to_string_pretty(
        &vwap_data::diag::raw_diagnostics(&raw),
    )?)
}

pub fn events_detect(config: &Path) -> Result<String> {
    let data = vwap_core::load_data_config(config)?;
    let ev = vwap_core::load_events_config(config)?;
    let clean = load_clean(&data)?;
    let s = vwap_events::run_events(&data, &ev, &clean.bars)?;
    let rows: Vec<(String, u64)> = s
        .iter()
        .map(|x| (format!("events_{}m", x.width), x.events as u64))
        .collect();
    let rows_ref: Vec<(&str, u64)> = rows.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    write_manifest(
        config,
        &ev.output_dir,
        "events_detect",
        &["data.toml", "events.toml"],
        Some(&clean.fingerprint),
        &rows_ref,
    )?;
    Ok(serde_json::to_string_pretty(&s)?)
}

pub fn labels_build(config: &Path) -> Result<String> {
    let data = vwap_core::load_data_config(config)?;
    let ev = vwap_core::load_events_config(config)?;
    let lab = vwap_core::load_labels_config(config)?;
    let clean = load_clean(&data)?;
    let bars = &clean.bars;
    let params = vwap_events::event_params(&data, &ev);
    let mut out = String::new();
    let mut rows: Vec<(String, u64)> = Vec::new();
    for &w in &ev.timeframes {
        let coarse = vwap_events::build(bars, &data, w, ev.atr_length);
        let (events, _) = vwap_events::detect(&coarse, &params);
        let lp = vwap_labels::LabelParams {
            m1: lab.m1,
            m2: lab.m2,
            horizon: lab.horizon(w),
        };
        let (labeled, drops) = vwap_labels::label_events(&events, &coarse, bars, &lp);
        let mut counts = [0usize; 3];
        let mut by_year: BTreeMap<i32, (usize, usize)> = BTreeMap::new();
        for l in &labeled {
            if let Some(x) = l.class.and_then(|c| counts.get_mut(c.index())) {
                *x += 1;
            }
            let year = vwap_data::diag::utc(l.event.open_us)
                .format("%Y")
                .to_string()
                .parse::<i32>()
                .unwrap_or(0);
            let e = by_year.entry(year).or_default();
            e.0 += 1;
            e.1 += usize::from(l.ambiguous);
        }
        let report = vwap_labels::LabelReport {
            width: w,
            m1: lab.m1,
            m2: lab.m2,
            horizon: lab.horizon(w),
            drops,
            class_counts: counts,
            ambiguous_by_year: by_year,
        };
        vwap_labels::write_labels(&labeled, &report, Path::new(&lab.output_dir))?;
        rows.push((format!("labeled_{w}m"), labeled.len() as u64));
        out.push_str(&serde_json::to_string(&report)?);
        out.push('\n');
    }
    let rows_ref: Vec<(&str, u64)> = rows.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    write_manifest(
        config,
        &lab.output_dir,
        "labels_build",
        &["data.toml", "events.toml", "labels.toml"],
        Some(&clean.fingerprint),
        &rows_ref,
    )?;
    Ok(out)
}

pub fn baselines_run(config: &Path) -> Result<String> {
    let data = vwap_core::load_data_config(config)?;
    let ev = vwap_core::load_events_config(config)?;
    let lab = vwap_core::load_labels_config(config)?;
    let base = vwap_core::load_baselines_config(config)?;
    let res = vwap_core::load_research_config(config)?;
    let clean = load_clean(&data)?;
    // gate 3a is computed on the discovery segment only
    let window = (
        vwap_core::ResearchConfig::date_us(&res.discovery_start),
        vwap_core::ResearchConfig::date_us(&res.confirm_start),
    );
    let r = vwap_baselines::run_baselines(&data, &ev, &lab, &base, window, &clean.bars)?;
    write_manifest(
        config,
        &base.output_dir,
        "baselines_run",
        &[
            "data.toml",
            "events.toml",
            "labels.toml",
            "baselines.toml",
            "research.toml",
        ],
        Some(&clean.fingerprint),
        &[("widths", r.widths.len() as u64)],
    )?;
    Ok(vwap_baselines::run::render_markdown(&r))
}

pub fn spotcheck(config: &Path) -> Result<String> {
    let data = vwap_core::load_data_config(config)?;
    let ev = vwap_core::load_events_config(config)?;
    let res = vwap_core::load_research_config(config)?;
    let clean = load_clean(&data)?;
    let params = vwap_events::event_params(&data, &ev);
    let (start, end) = (
        vwap_core::ResearchConfig::date_us(&res.discovery_start),
        vwap_core::ResearchConfig::date_us(&res.confirm_start),
    );
    let mut all = Vec::new();
    let mut chosen: Option<Vec<i32>> = None;
    for &w in &ev.timeframes {
        let coarse = vwap_events::build(&clean.bars, &data, w, ev.atr_length);
        let (events, _) = vwap_events::detect(&coarse, &params);
        let sessions = match &chosen {
            Some(s) => s.clone(),
            None => {
                let eligible = vwap_report::spotcheck::eligible_sessions(
                    &coarse,
                    start,
                    end,
                    ev.spotcheck_min_session_minutes,
                );
                let s = vwap_report::spotcheck::pick_sessions(
                    &eligible,
                    ev.spotcheck_days,
                    ev.spotcheck_seed,
                );
                chosen = Some(s.clone());
                s
            }
        };
        all.extend(vwap_report::spotcheck::spotcheck(
            &coarse,
            &events,
            &sessions,
            ev.exclude_minutes,
            &ev.font_paths,
            Path::new(&ev.spotcheck_dir),
        )?);
    }
    Ok(serde_json::to_string_pretty(&all)?)
}
