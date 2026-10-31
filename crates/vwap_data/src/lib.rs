//! Loading, cleaning and quality checks for the raw 1 minute bars.
#![cfg_attr(
    test,
    allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::expect_used)
)]

pub mod calendar;
pub mod clean;
pub mod diag;
pub mod error;
pub mod load;
pub mod quality;
pub mod roll;
pub mod time;
pub mod write;

use std::fs;
use std::path::Path;

use serde::Serialize;
use vwap_core::DataConfig;

pub use clean::{CleanBars, flag};
pub use error::DataError;
pub use load::{RawBars, read_raw};

#[derive(Debug, Serialize)]
pub struct CleanSummary {
    pub input_sha256: String,
    pub source_dtype: String,
    pub counts: clean::CleanCounts,
    pub first_utc: String,
    pub last_utc: String,
    pub rows_per_year: Vec<(i32, usize, String)>,
}

/// The whole Phase 1 stage: load, clean, check, flag rolls, write.
pub fn run_clean(cfg: &DataConfig) -> Result<CleanSummary, DataError> {
    let input = Path::new(&cfg.input_path);
    let raw = read_raw(input)?;
    let (mut bars, counts) = clean::clean(&raw, cfg)?;
    let mut quality = quality::analyze(&mut bars, cfg)?;
    let (rolls, roll_idx) = roll::roll_report(&bars, cfg);
    quality::flag_roll_buffers(&mut bars, &roll_idx, cfg.roll_buffer_sessions);
    quality.flag_counts = quality::count_flags(&bars);

    let out_dir = Path::new(&cfg.output_dir);
    let art_dir = Path::new(&cfg.artifact_dir);
    for d in [out_dir, art_dir] {
        fs::create_dir_all(d).map_err(|source| DataError::Io {
            path: d.display().to_string(),
            source,
        })?;
    }
    let rows_per_year = write::write_partitioned(&bars, out_dir)?;
    let summary = CleanSummary {
        input_sha256: write::sha256_file(input)?,
        source_dtype: raw.source_dtype.clone(),
        counts,
        first_utc: diag::utc(bars.ts_us.first().copied().unwrap_or(0)).to_rfc3339(),
        last_utc: diag::utc(bars.ts_us.last().copied().unwrap_or(0)).to_rfc3339(),
        rows_per_year,
    };
    let put = |name: &str, v: &dyn erased::Json| -> Result<(), DataError> {
        let p = art_dir.join(name);
        fs::write(&p, v.to_json()?).map_err(|source| DataError::Io {
            path: p.display().to_string(),
            source,
        })
    };
    put("quality_report.json", &quality)?;
    put("roll_table.json", &rolls)?;
    put("manifest.json", &summary)?;
    Ok(summary)
}

mod erased {
    use serde::Serialize;

    use crate::error::DataError;

    pub trait Json {
        fn to_json(&self) -> Result<String, DataError>;
    }

    impl<T: Serialize> Json for T {
        fn to_json(&self) -> Result<String, DataError> {
            Ok(serde_json::to_string_pretty(self)?)
        }
    }
}
