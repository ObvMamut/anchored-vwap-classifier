//! Writes cleaned bars to Parquet partitioned by UTC year, plus the session table.
#![allow(clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::path::Path;

use chrono::{DateTime, Datelike};
use polars::prelude::*;

use crate::clean::CleanBars;
use crate::error::DataError;

fn io_err(path: &Path) -> impl Fn(std::io::Error) -> DataError + '_ {
    move |source| DataError::Io {
        path: path.display().to_string(),
        source,
    }
}

pub use vwap_core::manifest::sha256_file;

/// A fingerprint of the cleaned data: the hash of every yearly partition file, in year order.
/// Downstream stages compare it with the value recorded by `data clean`.
pub fn fingerprint(dir: &Path) -> Result<String, DataError> {
    let mut years: Vec<std::path::PathBuf> = fs::read_dir(dir)
        .map_err(io_err(dir))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("part0.parquet").exists())
        .collect();
    years.sort();
    let mut listing = String::new();
    for y in years {
        let name = y
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        listing.push_str(&format!(
            "{name}:{}\n",
            sha256_file(&y.join("part0.parquet"))?
        ));
    }
    Ok(vwap_core::manifest::sha256_bytes(listing.as_bytes()))
}

fn frame(b: &CleanBars, range: std::ops::Range<usize>) -> Result<DataFrame, DataError> {
    let s = |name: &str, v: &[f64]| Series::new(name.into(), v[range.clone()].to_vec());
    let si = |name: &str, v: Vec<i64>| Series::new(name.into(), v);
    let ts = si("ts", b.ts_us[range.clone()].to_vec()).cast(&DataType::Datetime(
        TimeUnit::Microseconds,
        Some(TimeZone::UTC),
    ))?;
    let df = DataFrame::new_infer_height(vec![
        ts.into(),
        s("open", &b.open).into(),
        s("high", &b.high).into(),
        s("low", &b.low).into(),
        s("close", &b.close).into(),
        s("volume", &b.volume).into(),
        s("tick_volume", &b.tick_volume).into(),
        s("weight", &b.weight).into(),
        si(
            "ny_date",
            b.ny_date[range.clone()]
                .iter()
                .map(|&x| i64::from(x))
                .collect(),
        )
        .into(),
        si(
            "ny_minute",
            b.ny_minute[range.clone()]
                .iter()
                .map(|&x| i64::from(x))
                .collect(),
        )
        .into(),
        si(
            "globex_session",
            b.globex_session[range.clone()]
                .iter()
                .map(|&x| i64::from(x))
                .collect(),
        )
        .into(),
        si(
            "flags",
            b.flags[range.clone()]
                .iter()
                .map(|&x| i64::from(x))
                .collect(),
        )
        .into(),
    ])?;
    Ok(df)
}

/// Writes `year=YYYY/part0.parquet` files and returns `(year, rows, sha256)`.
pub fn write_partitioned(
    b: &CleanBars,
    dir: &Path,
) -> Result<Vec<(i32, usize, String)>, DataError> {
    let mut ranges: BTreeMap<i32, (usize, usize)> = BTreeMap::new();
    for (k, &t) in b.ts_us.iter().enumerate() {
        let y = DateTime::from_timestamp_micros(t)
            .unwrap_or_default()
            .year();
        let e = ranges.entry(y).or_insert((k, k));
        e.1 = k;
    }
    let mut out = Vec::new();
    for (year, (first, last)) in ranges {
        let part = dir.join(format!("year={year}"));
        fs::create_dir_all(&part).map_err(io_err(&part))?;
        let path = part.join("part0.parquet");
        let mut df = frame(b, first..last + 1)?;
        let file = File::create(&path).map_err(io_err(&path))?;
        ParquetWriter::new(file).finish(&mut df)?;
        out.push((year, last + 1 - first, sha256_file(&path)?));
    }
    Ok(out)
}

/// Reads all yearly partitions back into one cleaned column set (ascending).
pub fn read_partitioned(dir: &Path) -> Result<CleanBars, DataError> {
    let mut years: Vec<std::path::PathBuf> = fs::read_dir(dir)
        .map_err(io_err(dir))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("part0.parquet").exists())
        .collect();
    years.sort();
    let mut out = CleanBars::default();
    for y in years {
        let path = y.join("part0.parquet");
        let file = File::open(&path).map_err(io_err(&path))?;
        let df = ParquetReader::new(file).finish()?;
        let f = |name: &str| -> Result<Vec<f64>, DataError> {
            Ok(df
                .column(name)?
                .cast(&DataType::Float64)?
                .f64()?
                .iter()
                .map(|v| v.unwrap_or(f64::NAN))
                .collect())
        };
        let i = |name: &str| -> Result<Vec<i64>, DataError> {
            Ok(df
                .column(name)?
                .cast(&DataType::Int64)?
                .i64()?
                .iter()
                .map(|v| v.unwrap_or(0))
                .collect())
        };
        out.ts_us.extend(i("ts")?);
        out.open.extend(f("open")?);
        out.high.extend(f("high")?);
        out.low.extend(f("low")?);
        out.close.extend(f("close")?);
        out.volume.extend(f("volume")?);
        out.tick_volume.extend(f("tick_volume")?);
        out.weight.extend(f("weight")?);
        out.ny_date
            .extend(i("ny_date")?.into_iter().map(|x| x as i32));
        out.ny_minute
            .extend(i("ny_minute")?.into_iter().map(|x| x as u16));
        out.globex_session
            .extend(i("globex_session")?.into_iter().map(|x| x as i32));
        out.flags.extend(i("flags")?.into_iter().map(|x| x as u32));
    }
    Ok(out)
}
