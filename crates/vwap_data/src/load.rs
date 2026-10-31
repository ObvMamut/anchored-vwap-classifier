//! Reads the raw parquet file into plain columns.

use std::fs::File;
use std::path::Path;

use polars::prelude::*;

use crate::error::DataError;

/// Raw 1 minute bars as plain columns. Timestamps are UTC microseconds.
#[derive(Debug, Clone, Default)]
pub struct RawBars {
    pub ts_us: Vec<i64>,
    pub open: Vec<f64>,
    pub high: Vec<f64>,
    pub low: Vec<f64>,
    pub close: Vec<f64>,
    pub volume: Vec<f64>,
    pub tick_volume: Vec<f64>,
    /// Dtype of the `DateTime` column in the file, recorded in the report.
    pub source_dtype: String,
}

impl RawBars {
    pub fn len(&self) -> usize {
        self.ts_us.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ts_us.is_empty()
    }
}

fn f64_column(df: &DataFrame, name: &str) -> Result<Vec<f64>, DataError> {
    let col = df
        .column(name)
        .map_err(|_| DataError::Schema(format!("missing column {name}")))?;
    let cast = col.cast(&DataType::Float64)?;
    let ca = cast.f64()?;
    ca.iter()
        .map(|v| v.ok_or_else(|| DataError::Schema(format!("null value in column {name}"))))
        .collect()
}

/// Reads `path`. A naive datetime column is interpreted as UTC. Time units are
/// normalised to microseconds.
pub fn read_raw(path: &Path) -> Result<RawBars, DataError> {
    let file = File::open(path).map_err(|source| DataError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let df = ParquetReader::new(file).finish()?;
    let ts_col = df
        .column("DateTime")
        .map_err(|_| DataError::Schema("missing column DateTime".into()))?;
    let source_dtype = format!("{:?}", ts_col.dtype());
    let unit = match ts_col.dtype() {
        DataType::Datetime(unit, _) => *unit,
        other => {
            return Err(DataError::Schema(format!(
                "DateTime must be a datetime column, found {other:?}"
            )));
        }
    };
    let physical = ts_col.cast(&DataType::Int64)?;
    let raw = physical.i64()?;
    let mut ts_us = Vec::with_capacity(raw.len());
    for v in raw.iter() {
        let v = v.ok_or_else(|| DataError::Schema("null timestamp".into()))?;
        let us = match unit {
            TimeUnit::Nanoseconds => v.div_euclid(1_000),
            TimeUnit::Microseconds => v,
            TimeUnit::Milliseconds => v.saturating_mul(1_000),
        };
        ts_us.push(us);
    }
    Ok(RawBars {
        ts_us,
        open: f64_column(&df, "Open")?,
        high: f64_column(&df, "High")?,
        low: f64_column(&df, "Low")?,
        close: f64_column(&df, "Close")?,
        volume: f64_column(&df, "Volume")?,
        tick_volume: f64_column(&df, "TickVolume")?,
        source_dtype,
    })
}
