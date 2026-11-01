//! Parquet export and import of the sample table. Class is an integer: 0 rejection,
//! 1 breakthrough, 2 chop, minus 1 for an ambiguous event. Missing feature values stay NaN.

use std::fs::{self, File};
use std::path::Path;

use polars::prelude::*;
use vwap_core::Class;
use vwap_features::feature_names;

use crate::error::DatasetError;
use crate::sample::Sample;

fn ts(name: &str, v: Vec<i64>) -> Result<Column, DatasetError> {
    Ok(Series::new(name.into(), v)
        .cast(&DataType::Datetime(
            TimeUnit::Microseconds,
            Some(TimeZone::UTC),
        ))?
        .into())
}

/// The frame of the samples of one timeframe, in the order given.
pub fn samples_frame(samples: &[&Sample]) -> Result<DataFrame, DatasetError> {
    let i = |n: &str, f: &dyn Fn(&Sample) -> i64| -> Column {
        Series::new(n.into(), samples.iter().map(|s| f(s)).collect::<Vec<i64>>()).into()
    };
    let f = |n: &str, g: &dyn Fn(&Sample) -> f64| -> Column {
        Series::new(n.into(), samples.iter().map(|s| g(s)).collect::<Vec<f64>>()).into()
    };
    let mut cols = vec![
        i("width", &|s| i64::from(s.width)),
        i("event_id", &|s| i64::from(s.event_id)),
        i("session", &|s| i64::from(s.session)),
        i("year", &|s| i64::from(s.year)),
        ts("close_ts", samples.iter().map(|s| s.t_close_us).collect())?,
        ts("t_end_ts", samples.iter().map(|s| s.t_end_us).collect())?,
        i("side", &|s| i64::from(s.side)),
        i("class", &|s| s.class.map_or(-1, |c| c.index() as i64)),
        i("ambiguous", &|s| i64::from(s.ambiguous)),
        f("outcome_ticks", &|s| s.outcome_ticks),
        f("sigma_over_price", &|s| s.sigma_over_price),
        i("bar", &|s| s.bar as i64),
        f("close", &|s| s.close),
        f("sigma", &|s| s.sigma),
        f("b_rej", &|s| s.b_rej),
        f("b_brk", &|s| s.b_brk),
        f("r_t_ticks", &|s| s.r_t_ticks),
    ];
    for (k, name) in feature_names().iter().enumerate() {
        cols.push(
            Series::new(
                (*name).into(),
                samples
                    .iter()
                    .map(|s| s.features.get(k).copied().unwrap_or(f64::NAN))
                    .collect::<Vec<f64>>(),
            )
            .into(),
        );
    }
    Ok(DataFrame::new_infer_height(cols)?)
}

/// Writes `features_<w>m.parquet` for each timeframe present. Returns the row count per timeframe.
pub fn write_features(
    samples: &[Sample],
    widths: &[u32],
    dir: &Path,
) -> Result<Vec<(u32, usize)>, DatasetError> {
    let io = |p: &Path| {
        let s = p.display().to_string();
        move |source| DatasetError::Io {
            path: s.clone(),
            source,
        }
    };
    fs::create_dir_all(dir).map_err(io(dir))?;
    let mut rows = Vec::new();
    for &w in widths {
        let sub: Vec<&Sample> = samples.iter().filter(|s| s.width == w).collect();
        let mut df = samples_frame(&sub)?;
        let path = dir.join(format!("features_{w}m.parquet"));
        let file = File::create(&path).map_err(io(&path))?;
        ParquetWriter::new(file).finish(&mut df)?;
        rows.push((w, sub.len()));
    }
    Ok(rows)
}

fn ints(df: &DataFrame, name: &str) -> Result<Vec<i64>, DatasetError> {
    let c = df
        .column(name)
        .map_err(|_| DatasetError::Column(name.into()))?;
    let c = if matches!(c.dtype(), DataType::Datetime(_, _)) {
        c.cast(&DataType::Int64)?
    } else {
        c.clone()
    };
    Ok(c.i64()
        .map_err(|_| DatasetError::Column(name.into()))?
        .iter()
        .map(|v| v.unwrap_or_default())
        .collect())
}

fn floats(df: &DataFrame, name: &str) -> Result<Vec<f64>, DatasetError> {
    Ok(df
        .column(name)
        .map_err(|_| DatasetError::Column(name.into()))?
        .f64()
        .map_err(|_| DatasetError::Column(name.into()))?
        .iter()
        .map(|v| v.unwrap_or(f64::NAN))
        .collect())
}

/// Reads a file written by `write_features`.
pub fn read_samples(path: &Path) -> Result<Vec<Sample>, DatasetError> {
    let file = File::open(path).map_err(|source| DatasetError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let df = ParquetReader::new(file).finish()?;
    let width = ints(&df, "width")?;
    let event_id = ints(&df, "event_id")?;
    let session = ints(&df, "session")?;
    let year = ints(&df, "year")?;
    let close_ts = ints(&df, "close_ts")?;
    let end_ts = ints(&df, "t_end_ts")?;
    let side = ints(&df, "side")?;
    let class = ints(&df, "class")?;
    let ambiguous = ints(&df, "ambiguous")?;
    let outcome = floats(&df, "outcome_ticks")?;
    let sop = floats(&df, "sigma_over_price")?;
    let bar = ints(&df, "bar")?;
    let close = floats(&df, "close")?;
    let sigma = floats(&df, "sigma")?;
    let b_rej = floats(&df, "b_rej")?;
    let b_brk = floats(&df, "b_brk")?;
    let r_t = floats(&df, "r_t_ticks")?;
    let feats: Vec<Vec<f64>> = feature_names()
        .iter()
        .map(|n| floats(&df, n))
        .collect::<Result<_, _>>()?;
    let at = |v: &[i64], k: usize| v.get(k).copied().unwrap_or_default();
    let fat = |v: &[f64], k: usize| v.get(k).copied().unwrap_or(f64::NAN);
    Ok((0..df.height())
        .map(|k| Sample {
            width: at(&width, k) as u32,
            event_id: at(&event_id, k) as u32,
            session: at(&session, k) as i32,
            year: at(&year, k) as i32,
            t_close_us: at(&close_ts, k),
            t_end_us: at(&end_ts, k),
            side: at(&side, k) as i8,
            class: match at(&class, k) {
                0 => Some(Class::Rejection),
                1 => Some(Class::Breakthrough),
                2 => Some(Class::Chop),
                _ => None,
            },
            ambiguous: at(&ambiguous, k) != 0,
            outcome_ticks: fat(&outcome, k),
            sigma_over_price: fat(&sop, k),
            bar: at(&bar, k) as usize,
            close: fat(&close, k),
            sigma: fat(&sigma, k),
            b_rej: fat(&b_rej, k),
            b_brk: fat(&b_brk, k),
            r_t_ticks: fat(&r_t, k),
            features: feats.iter().map(|c| fat(c, k)).collect(),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(k: u32) -> Sample {
        let n = feature_names().len();
        Sample {
            width: 15,
            event_id: k,
            session: 7,
            year: 2019,
            t_close_us: 1_500_000_000_000_000 + i64::from(k) * 900_000_000,
            t_end_us: 1_500_000_000_000_000 + i64::from(k) * 900_000_000 + 7_200_000_000,
            side: if k.is_multiple_of(2) { 1 } else { -1 },
            class: match k % 4 {
                0 => Some(Class::Rejection),
                1 => Some(Class::Breakthrough),
                2 => Some(Class::Chop),
                _ => None,
            },
            ambiguous: k % 4 == 3,
            outcome_ticks: f64::from(k) * 0.25 - 3.0,
            sigma_over_price: 0.004,
            bar: 100 + k as usize,
            close: 15_000.25,
            sigma: 12.5,
            b_rej: 15_012.75,
            b_brk: 14_987.75,
            r_t_ticks: -1.5,
            features: (0..n)
                .map(|j| {
                    if j == 3 {
                        f64::NAN
                    } else {
                        j as f64 * 0.1 + f64::from(k)
                    }
                })
                .collect(),
        }
    }

    #[test]
    fn parquet_round_trip_reproduces_the_table_bit_for_bit() {
        let all: Vec<Sample> = (0..9).map(sample).collect();
        let dir = std::env::temp_dir().join(format!("vwap_dataset_rt_{}", std::process::id()));
        let rows = write_features(&all, &[15, 5], &dir).unwrap();
        assert_eq!(rows, vec![(15, 9), (5, 0)]);
        let back = read_samples(&dir.join("features_15m.parquet")).unwrap();
        assert_eq!(back.len(), all.len());
        for (a, b) in all.iter().zip(&back) {
            assert_eq!(a.width, b.width);
            assert_eq!(a.event_id, b.event_id);
            assert_eq!(a.session, b.session);
            assert_eq!(a.year, b.year);
            assert_eq!(a.t_close_us, b.t_close_us);
            assert_eq!(a.t_end_us, b.t_end_us);
            assert_eq!(a.side, b.side);
            assert_eq!(a.class, b.class);
            assert_eq!(a.ambiguous, b.ambiguous);
            assert_eq!(a.bar, b.bar);
            for (x, y) in [
                (a.outcome_ticks, b.outcome_ticks),
                (a.sigma_over_price, b.sigma_over_price),
                (a.close, b.close),
                (a.sigma, b.sigma),
                (a.b_rej, b.b_rej),
                (a.b_brk, b.b_brk),
                (a.r_t_ticks, b.r_t_ticks),
            ] {
                assert_eq!(x.to_bits(), y.to_bits());
            }
            assert_eq!(a.features.len(), b.features.len());
            for (x, y) in a.features.iter().zip(&b.features) {
                assert_eq!(x.is_nan(), y.is_nan());
                if !x.is_nan() {
                    assert_eq!(x.to_bits(), y.to_bits());
                }
            }
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
