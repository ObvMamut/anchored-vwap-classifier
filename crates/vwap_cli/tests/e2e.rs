//! End to end test of the real `vwap` binary on a synthetic raw file: the stages run in order,
//! each records a manifest tied to the cleaned data, stale data is refused, and the safety
//! commands refuse when they should.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use chrono::{Datelike, Duration, NaiveDate, Weekday};
use polars::prelude::*;

fn lcg(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    *state >> 33
}

/// Eight weeks of Globex sessions in the broker's server time (New York plus 7 hours, stored as
/// if it were UTC). One anchor day is a holiday and one session has a 10 minute hole.
fn write_raw(path: &Path) -> usize {
    let (mut ts, mut o, mut h, mut l, mut c, mut tv) =
        (vec![], vec![], vec![], vec![], vec![], vec![]);
    let mut rng = 7u64;
    let mut price = 6400.0f64;
    let first = NaiveDate::from_ymd_opt(2018, 1, 7).unwrap(); // a Sunday
    for day in 0..55 {
        let anchor = first + Duration::days(day);
        if !matches!(
            anchor.weekday(),
            Weekday::Sun | Weekday::Mon | Weekday::Tue | Weekday::Wed | Weekday::Thu
        ) || anchor == NaiveDate::from_ymd_opt(2018, 2, 19).unwrap()
        {
            continue;
        }
        let next = anchor + Duration::days(1);
        for k in 0..1380u32 {
            // minute 0 is 18:00 of the anchor day, minute 1379 is 16:59 of the next day
            let (date, minute) = if k < 360 {
                (anchor, 1080 + k)
            } else {
                (next, k - 360)
            };
            if anchor == NaiveDate::from_ymd_opt(2018, 1, 24).unwrap() && (540..550).contains(&k) {
                continue; // an unexpected gap
            }
            let local = date.and_hms_opt(minute / 60, minute % 60, 0).unwrap();
            let label = local + Duration::hours(7);
            ts.push(label.and_utc().timestamp_micros());
            let open = price;
            price = (price + ((lcg(&mut rng) % 31) as f64 - 15.0) * 0.1).max(100.0);
            let close = (price * 10.0).round() / 10.0;
            o.push(open);
            c.push(close);
            h.push(open.max(close) + (lcg(&mut rng) % 4) as f64 * 0.1 + 0.1);
            l.push(open.min(close) - (lcg(&mut rng) % 4) as f64 * 0.1 - 0.1);
            tv.push(20.0 + (lcg(&mut rng) % 180) as f64);
        }
    }
    let n = ts.len();
    // the raw file is sorted newest first, like the real one
    for v in [&mut o, &mut h, &mut l, &mut c, &mut tv] {
        v.reverse();
    }
    ts.reverse();
    let col = |name: &str, v: Vec<f64>| Series::new(name.into(), v).into();
    let dt = Series::new("DateTime".into(), ts)
        .cast(&DataType::Datetime(TimeUnit::Microseconds, None))
        .unwrap();
    let mut df = DataFrame::new_infer_height(vec![
        dt.into(),
        col("Open", o),
        col("High", h),
        col("Low", l),
        col("Close", c),
        col("Volume", vec![0.0; n]),
        col("TickVolume", tv),
    ])
    .unwrap();
    ParquetWriter::new(fs::File::create(path).unwrap())
        .finish(&mut df)
        .unwrap();
    n
}

/// Replaces the line `key = ...` in a TOML text.
fn set(text: &str, key: &str, value: &str) -> String {
    let mut found = false;
    let out: Vec<String> = text
        .lines()
        .map(|l| {
            if l.trim_start().starts_with(&format!("{key} =")) {
                found = true;
                format!("{key} = {value}")
            } else {
                l.to_string()
            }
        })
        .collect();
    assert!(found, "key {key} not found");
    out.join("\n") + "\n"
}

struct Sandbox {
    dir: PathBuf,
    config: PathBuf,
}

fn sandbox(name: &str) -> Sandbox {
    let dir = std::env::temp_dir().join(format!("vwap_e2e_{name}_{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("config")).unwrap();
    write_raw(&dir.join("raw.parquet"));
    let repo_cfg = Path::new("../../config");
    let q = |p: &str| format!("\"{}\"", dir.join(p).display());
    let edit = |file: &str, edits: &[(&str, String)]| {
        let mut t = fs::read_to_string(repo_cfg.join(file)).unwrap();
        for (k, v) in edits {
            t = set(&t, k, v);
        }
        fs::write(dir.join("config").join(file), t).unwrap();
    };
    let remove_line = |file: &str, key: &str| {
        let p = dir.join("config").join(file);
        let t: String = fs::read_to_string(&p)
            .unwrap()
            .lines()
            .filter(|l| !l.trim_start().starts_with(key))
            .map(|l| format!("{l}\n"))
            .collect();
        fs::write(p, t).unwrap();
    };
    edit(
        "data.toml",
        &[
            ("input_path", q("raw.parquet")),
            ("output_dir", q("clean")),
            ("artifact_dir", q("art_data")),
        ],
    );
    remove_line("data.toml", "input_sha256");
    edit(
        "events.toml",
        &[
            ("output_dir", q("art_events")),
            ("spotcheck_dir", q("art_spot")),
        ],
    );
    edit("labels.toml", &[("output_dir", q("art_labels"))]);
    edit(
        "baselines.toml",
        &[
            ("output_dir", q("art_base")),
            ("report_dir", q("reports")),
            ("bootstrap_draws", "200".into()),
        ],
    );
    edit(
        "research.toml",
        &[
            ("discovery_start", "\"2017-12-01\"".into()),
            ("discovery_end", "\"2018-03-31\"".into()),
            ("confirm_start", "\"2018-04-01\"".into()),
            ("confirm_end", "\"2018-06-30\"".into()),
            ("holdout_start", "\"2018-07-01\"".into()),
            ("cards_dir", q("cards")),
            ("ledger_path", q("research/ledger.jsonl")),
            ("report_dir", q("research/reports")),
        ],
    );
    for f in [
        "features.toml",
        "model.toml",
        "validation.toml",
        "backtest.toml",
        "report.toml",
        "strategy.toml",
    ] {
        fs::copy(repo_cfg.join(f), dir.join("config").join(f)).unwrap();
    }
    Sandbox {
        config: dir.join("config"),
        dir,
    }
}

struct Out {
    ok: bool,
    stdout: String,
    stderr: String,
}

fn vwap(args: &[&str], config: &Path) -> Out {
    let out = Command::new(env!("CARGO_BIN_EXE_vwap"))
        .args(args)
        .arg(config)
        .output()
        .unwrap();
    Out {
        ok: out.status.success(),
        stdout: String::from_utf8_lossy(&out.stdout).to_string(),
        stderr: String::from_utf8_lossy(&out.stderr).to_string(),
    }
}

fn json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn the_stages_run_in_order_and_chain_their_manifests() {
    let sb = sandbox("chain");
    let r = vwap(&["data", "clean"], &sb.config);
    assert!(r.ok, "data clean failed: {}", r.stderr);
    let data_manifest = json(&sb.dir.join("art_data/manifest.json"));
    let fingerprint = data_manifest["clean_fingerprint"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(data_manifest["counts"]["rows_out"].as_u64().unwrap() > 50_000);
    let quality = json(&sb.dir.join("art_data/quality_report.json"));
    assert_eq!(
        quality["gaps"]["unexpected_long"].as_u64(),
        Some(1),
        "the planted 10 minute hole"
    );
    assert_eq!(quality["label_check"]["detected"].as_str(), Some("open"));
    assert!(sb.dir.join("clean/year=2018/part0.parquet").exists());

    let r = vwap(&["events", "detect"], &sb.config);
    assert!(r.ok, "events detect failed: {}", r.stderr);
    let m = json(&sb.dir.join("art_events/manifest.json"));
    assert_eq!(
        m["data_fingerprint"].as_str(),
        Some(fingerprint.as_str()),
        "manifests must chain"
    );
    assert!(m["rows"]["events_5m"].as_u64().unwrap() > 100);

    let r = vwap(&["labels", "build"], &sb.config);
    assert!(r.ok, "labels build failed: {}", r.stderr);
    assert!(
        sb.dir
            .join("art_labels/labeled_events_15m.parquet")
            .exists()
    );
    assert_eq!(
        json(&sb.dir.join("art_labels/manifest.json"))["data_fingerprint"].as_str(),
        Some(fingerprint.as_str())
    );

    let r = vwap(&["baselines", "run"], &sb.config);
    assert!(r.ok, "baselines run failed: {}", r.stderr);
    assert!(r.stdout.contains("Gate 3a"));
    assert!(sb.dir.join("art_base/baselines_report.json").exists());

    let r = vwap(&["research", "status"], &sb.config);
    assert!(r.ok, "{}", r.stderr);
    assert!(r.stdout.contains("cards run 0 of 12"));
    let r = vwap(&["research", "verify"], &sb.config);
    assert!(r.ok, "{}", r.stderr);
    let r = vwap(&["doctor"], &sb.config);
    assert!(r.ok, "doctor failed: {}", r.stderr);
    assert!(r.stdout.contains("checks,"));
}

#[test]
fn changed_cleaned_data_is_refused_until_data_clean_is_rerun() {
    let sb = sandbox("stale");
    assert!(vwap(&["data", "clean"], &sb.config).ok);
    assert!(vwap(&["events", "detect"], &sb.config).ok);
    let part = sb.dir.join("clean/year=2018/part0.parquet");
    let mut bytes = fs::read(&part).unwrap();
    bytes.push(0);
    fs::write(&part, bytes).unwrap();
    let r = vwap(&["events", "detect"], &sb.config);
    assert!(!r.ok);
    assert!(
        r.stderr.contains("does not match its manifest"),
        "{}",
        r.stderr
    );
    let r = vwap(&["baselines", "run"], &sb.config);
    assert!(!r.ok, "every stage must refuse stale data");
    // rerunning the cleaning restores a consistent state
    assert!(vwap(&["data", "clean"], &sb.config).ok);
    assert!(vwap(&["events", "detect"], &sb.config).ok);
}

#[test]
fn a_different_raw_file_is_refused_when_its_checksum_is_pinned() {
    let sb = sandbox("pin");
    let p = sb.config.join("data.toml");
    let t = fs::read_to_string(&p).unwrap();
    fs::write(&p, format!("input_sha256 = \"{}\"\n{t}", "0".repeat(64))).unwrap();
    let r = vwap(&["data", "clean"], &sb.config);
    assert!(!r.ok);
    assert!(r.stderr.contains("pins"), "{}", r.stderr);
}

#[test]
fn the_alternative_options_run_end_to_end() {
    let sb = sandbox("options");
    assert!(vwap(&["data", "clean"], &sb.config).ok);
    let base = vwap(&["events", "detect"], &sb.config);
    assert!(base.ok, "{}", base.stderr);
    let default_events = json(&sb.dir.join("art_events/manifest.json"))["rows"]["events_5m"]
        .as_u64()
        .unwrap();

    // the stricter VWAP reference, a sensitivity option
    let p = sb.config.join("events.toml");
    let t = fs::read_to_string(&p).unwrap();
    fs::write(&p, set(&t, "vwap_reference", "\"prior_bar_close\"")).unwrap();
    assert!(vwap(&["events", "detect"], &sb.config).ok);
    let strict = json(&sb.dir.join("art_events/manifest.json"))["rows"]["events_5m"]
        .as_u64()
        .unwrap();
    assert!(strict > 0);
    assert_ne!(
        strict, default_events,
        "the reference changes which bars touch"
    );
    fs::write(&p, t).unwrap();

    // the regular trading hours anchor
    let p = sb.config.join("data.toml");
    let t = fs::read_to_string(&p).unwrap();
    fs::write(&p, set(&t, "session_anchor", "\"rth\"")).unwrap();
    assert!(vwap(&["data", "clean"], &sb.config).ok);
    let r = vwap(&["events", "detect"], &sb.config);
    assert!(r.ok, "{}", r.stderr);
    assert!(vwap(&["labels", "build"], &sb.config).ok);
}

#[test]
fn confirmation_and_holdout_refuse_without_a_promoted_card() {
    let sb = sandbox("refuse");
    assert!(vwap(&["data", "clean"], &sb.config).ok);
    let r = vwap(&["research", "confirm", "H001"], &sb.config);
    assert!(!r.ok);
    let r = vwap(&["holdout", "run"], &sb.config);
    assert!(!r.ok);
    assert!(
        !sb.dir.join("research/holdout.lock").exists(),
        "a refused run must not create the lock"
    );
}
