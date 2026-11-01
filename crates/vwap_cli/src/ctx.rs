//! Helpers shared by the stage commands: git, the clean data check, manifests.

use std::path::Path;

use anyhow::{Context, Result, ensure};
use vwap_core::{DataConfig, ResearchConfig};
use vwap_data::CleanBars;

/// Runs git and returns trimmed standard output.
pub fn git(args: &[&str]) -> Result<String> {
    let out = std::process::Command::new("git")
        .args(args)
        .output()
        .context("cannot run git")?;
    ensure!(out.status.success(), "git {:?} failed", args);
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The current commit, or `unknown` outside a repository.
pub fn head() -> String {
    git(&["rev-parse", "HEAD"]).unwrap_or_else(|_| "unknown".to_string())
}

/// Refuses to continue if the ledger or the report directory would be ignored by git, because an
/// untracked audit trail disappears on a fresh clone and silently resets the alpha budget.
pub fn guard_ledger(cfg: &ResearchConfig) -> Result<()> {
    if git(&["rev-parse", "--git-dir"]).is_err() {
        return Ok(()); // not a repository, nothing to protect
    }
    for p in [
        cfg.ledger_path.as_str(),
        cfg.report_dir.as_str(),
        cfg.strategies_dir.as_str(),
    ] {
        let ignored = std::process::Command::new("git")
            .args(["check-ignore", "-q", p])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        ensure!(
            !ignored,
            "{p} is ignored by git, so the research audit trail would not be saved; fix .gitignore"
        );
    }
    Ok(())
}

/// Cleaned bars together with the fingerprint that was verified against the `data clean` manifest.
pub struct Clean {
    pub bars: CleanBars,
    pub fingerprint: String,
}

/// Reads the cleaned partitions and checks them against the manifest of the stage that wrote
/// them. A mismatch means the data changed after `data clean` and the stage must be rerun.
pub fn load_clean(data: &DataConfig) -> Result<Clean> {
    let dir = Path::new(&data.output_dir);
    let fingerprint = vwap_data::write::fingerprint(dir).with_context(|| {
        format!(
            "no cleaned data in {}; run `vwap data clean config` first",
            dir.display()
        )
    })?;
    let manifest = Path::new(&data.artifact_dir).join("manifest.json");
    let text = std::fs::read_to_string(&manifest).with_context(|| {
        format!(
            "{} is missing; run `vwap data clean config` first",
            manifest.display()
        )
    })?;
    let v: serde_json::Value = serde_json::from_str(&text)?;
    let recorded = v
        .get("clean_fingerprint")
        .and_then(serde_json::Value::as_str)
        .context(
            "the data manifest has no clean_fingerprint; run `vwap data clean config` again",
        )?;
    ensure!(
        recorded == fingerprint,
        "the cleaned data does not match its manifest (recorded {recorded}, found {fingerprint}); run `vwap data clean config` again"
    );
    let bars = vwap_data::write::read_partitioned(dir)?;
    Ok(Clean { bars, fingerprint })
}

/// Writes `<out_dir>/manifest.json` for a stage.
pub fn write_manifest(
    config_dir: &Path,
    out_dir: &str,
    stage: &str,
    config_files: &[&str],
    fingerprint: Option<&str>,
    rows: &[(&str, u64)],
) -> Result<()> {
    let mut m = vwap_core::manifest::Manifest::new(
        stage,
        vwap_core::manifest::config_hashes(config_dir, config_files)?,
        fingerprint.map(str::to_string),
        &head(),
    );
    for (k, v) in rows {
        m.rows.insert((*k).to_string(), *v);
    }
    m.write(Path::new(out_dir))?;
    Ok(())
}

/// The calendar table for the features: the frozen snapshot when `calendar_enabled`, otherwise
/// a disabled table whose features are NaN. A missing or changed snapshot is an error.
pub fn calendar_table(
    config_dir: &Path,
    feat: &vwap_core::FeaturesConfig,
) -> Result<vwap_features::CalendarTable> {
    if !feat.calendar_enabled {
        return Ok(vwap_features::CalendarTable::disabled());
    }
    let cal = vwap_core::load_calendar_config(config_dir)?;
    let (events, _) = vwap_calendar::read_snapshot(Path::new(&cal.output_dir))?;
    let mut rel: [Vec<i64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for e in events {
        if let Some(slot) = vwap_calendar::Kind::ALL
            .iter()
            .position(|k| *k == e.kind)
            .and_then(|i| rel.get_mut(i))
        {
            slot.push(e.utc_us);
        }
    }
    Ok(vwap_features::CalendarTable::new(
        rel,
        feat.calendar_cap_minutes,
    ))
}
