//! `vwap calendar fetch` and `vwap calendar import`: the only commands that write the frozen
//! calendar snapshot. `fetch` is the only code in the project that uses the network.

use std::path::Path;

use anyhow::{Context, Result};
use vwap_calendar::{HttpFetcher, fetch_all, import_csv, validate, write_snapshot};

fn summary(lock: &vwap_calendar::Lock, dir: &str) -> String {
    let mut s = format!(
        "wrote {dir}/events.csv and {dir}/lock.json\ncsv sha256 {}\nreleases per year (cpi, fomc, nfp):\n",
        lock.csv_sha256
    );
    for (y, c) in &lock.counts {
        s.push_str(&format!("{y}: {}, {}, {}\n", c[0], c[1], c[2]));
    }
    s.push_str("Commit both files, then set calendar_enabled = true in config/features.toml to use the K1 to K3 features.\n");
    s
}

/// Downloads the schedules, validates them and writes the snapshot. Fails closed.
pub fn fetch(config: &Path) -> Result<String> {
    let cal = vwap_core::load_calendar_config(config)?;
    let fetcher = HttpFetcher::new(cal.timeout_seconds, &cal.user_agent);
    let fetched = fetch_all(&fetcher, &cal)?;
    let now = chrono::Utc::now().to_rfc3339();
    let lock = write_snapshot(Path::new(&cal.output_dir), &fetched, &cal, &now)?;
    Ok(summary(&lock, &cal.output_dir))
}

/// Freezes a hand supplied CSV (`kind,date` rows) with the same validation as a fetch.
pub fn import(csv: &Path, config: &Path) -> Result<String> {
    let cal = vwap_core::load_calendar_config(config)?;
    let text =
        std::fs::read_to_string(csv).with_context(|| format!("reading {}", csv.display()))?;
    let got = import_csv(&text, &cal, &format!("imported from {}", csv.display()))?;
    validate(&got.events, &cal)?;
    let now = chrono::Utc::now().to_rfc3339();
    let lock = write_snapshot(Path::new(&cal.output_dir), &got, &cal, &now)?;
    Ok(summary(&lock, &cal.output_dir))
}
