//! Operations commands: the preflight check and the pipeline orchestrator.

use std::path::Path;
use std::process::Command;
use std::time::Instant;

use anyhow::{Result, bail};

use crate::ctx::{git, guard_ledger};

#[derive(PartialEq, Eq)]
enum Status {
    Ok,
    Warn,
    Fail,
}

struct Check {
    status: Status,
    text: String,
}

fn ok(text: impl Into<String>) -> Check {
    Check {
        status: Status::Ok,
        text: text.into(),
    }
}

fn warn(text: impl Into<String>) -> Check {
    Check {
        status: Status::Warn,
        text: text.into(),
    }
}

fn fail(text: impl Into<String>) -> Check {
    Check {
        status: Status::Fail,
        text: text.into(),
    }
}

fn first_line(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .to_string()
    })
}

/// Free space in GB of the filesystem holding the current directory, from `df`.
fn free_gb() -> Option<f64> {
    let out = Command::new("df").args(["-Pk", "."]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let kb: f64 = text
        .lines()
        .nth(1)?
        .split_whitespace()
        .nth(3)?
        .parse()
        .ok()?;
    Some(kb / 1_048_576.0)
}

fn pinned_toolchain() -> Option<String> {
    let text = std::fs::read_to_string("rust-toolchain.toml").ok()?;
    let v: toml::Value = toml::from_str(&text).ok()?;
    v.get("toolchain")?
        .get("channel")?
        .as_str()
        .map(str::to_string)
}

fn checks(config: &Path) -> Vec<Check> {
    let mut c = Vec::new();

    // configuration files
    let loaded = [
        vwap_core::load_data_config(config).map(|_| ()),
        vwap_core::load_events_config(config).map(|_| ()),
        vwap_core::load_labels_config(config).map(|_| ()),
        vwap_core::load_baselines_config(config).map(|_| ()),
        vwap_core::load_research_config(config).map(|_| ()),
        vwap_core::load_features_config(config).map(|_| ()),
        vwap_core::load_model_config(config).map(|_| ()),
        vwap_core::load_validation_config(config).map(|_| ()),
        vwap_core::load_strategy_config(config).map(|_| ()),
        vwap_core::load_backtest_config(config).map(|_| ()),
    ];
    match loaded.iter().find_map(|r| r.as_ref().err()) {
        None => c.push(ok("all 10 configuration files load and validate")),
        Some(e) => c.push(fail(format!("configuration: {e}"))),
    }

    // toolchain and build prerequisites (only needed to build, so warnings)
    let rustc = first_line("rustc", &["--version"]);
    match (rustc, pinned_toolchain()) {
        (Some(v), Some(pin)) if v.contains(&pin) => {
            c.push(ok(format!("{v} matches the pinned toolchain {pin}")))
        }
        (Some(v), Some(pin)) => {
            c.push(warn(format!("{v} differs from the pinned toolchain {pin}")))
        }
        (Some(v), None) => c.push(ok(v)),
        (None, _) => c.push(warn("rustc not found on PATH (only needed to build)")),
    }
    match first_line("cmake", &["--version"]) {
        Some(v) => c.push(ok(v)),
        None => c.push(warn(
            "cmake not found: building LightGBM from source needs it",
        )),
    }
    let cxx = ["c++", "g++", "clang++"]
        .iter()
        .find_map(|n| first_line(n, &["--version"]).map(|v| (n, v)));
    match cxx {
        Some((n, v)) => c.push(ok(format!("{n}: {v}"))),
        None => c.push(warn(
            "no C++ compiler found: building LightGBM from source needs one",
        )),
    }

    // data
    if let Ok(data) = vwap_core::load_data_config(config) {
        let path = Path::new(&data.input_path);
        if !path.exists() {
            c.push(fail(format!("raw data file {} is missing", path.display())));
        } else {
            match (&data.input_sha256, vwap_data::write::sha256_file(path)) {
                (Some(pin), Ok(h)) if *pin == h => {
                    c.push(ok(format!("{} matches its pinned SHA256", path.display())))
                }
                (Some(pin), Ok(h)) => c.push(fail(format!(
                    "{} has SHA256 {h}, config pins {pin}",
                    path.display()
                ))),
                (None, Ok(_)) => c.push(warn("input_sha256 is not pinned in config/data.toml")),
                (_, Err(e)) => c.push(fail(format!("cannot hash {}: {e}", path.display()))),
            }
        }
        match crate::ctx::load_clean(&data) {
            Ok(x) => c.push(ok(format!(
                "cleaned data present and matches its manifest ({} bars)",
                x.bars.len()
            ))),
            Err(_) => c.push(warn(
                "cleaned data not built yet: run `vwap data clean config`",
            )),
        }
    }
    if let Ok(ev) = vwap_core::load_events_config(config) {
        if ev.font_paths.iter().any(|p| std::fs::read(p).is_ok()) {
            c.push(ok("a chart font is available"));
        } else {
            c.push(warn("no font from font_paths in config/events.toml is readable: `vwap spotcheck` will fail"));
        }
    }

    // environment
    match free_gb() {
        Some(g) if g < 5.0 => c.push(warn(format!(
            "only {g:.1} GB free; a debug build needs about 2 GB and a release build about 2 GB"
        ))),
        Some(g) => c.push(ok(format!("{g:.1} GB free"))),
        None => c.push(warn("cannot read free disk space")),
    }
    if git(&["rev-parse", "--git-dir"]).is_ok() {
        match git(&["status", "--porcelain"]) {
            Ok(s) if s.is_empty() => c.push(ok("git working tree is clean")),
            Ok(_) => c.push(warn("git working tree has uncommitted changes")),
            Err(e) => c.push(warn(format!("git status failed: {e}"))),
        }
        if let Ok(res) = vwap_core::load_research_config(config) {
            match guard_ledger(&res) {
                Ok(()) => c.push(ok(format!(
                    "{} and {} are tracked by git",
                    res.ledger_path, res.report_dir
                ))),
                Err(e) => c.push(fail(e.to_string())),
            }
        }
    } else {
        c.push(warn(
            "not a git repository: research cards cannot be registered",
        ));
    }
    c
}

pub fn doctor(config: &Path) -> Result<String> {
    let cs = checks(config);
    let mut out = String::new();
    for c in &cs {
        let tag = match c.status {
            Status::Ok => "[ok]  ",
            Status::Warn => "[warn]",
            Status::Fail => "[FAIL]",
        };
        out.push_str(&format!("{tag} {}\n", c.text));
    }
    let fails = cs.iter().filter(|c| c.status == Status::Fail).count();
    let warns = cs.iter().filter(|c| c.status == Status::Warn).count();
    out.push_str(&format!(
        "{} checks, {fails} failed, {warns} warnings\n",
        cs.len()
    ));
    if fails > 0 {
        bail!("{out}");
    }
    Ok(out)
}

/// A pipeline stage: a name and the function that runs it.
type Stage = (&'static str, fn(&Path) -> Result<String>);

/// Runs the deterministic stages in order and stops at the first error. The omnibus and the card
/// runs are left out on purpose, because they spend alpha.
pub fn pipeline(config: &Path) -> Result<String> {
    let stages: [Stage; 8] = [
        ("doctor", doctor),
        ("data clean", crate::stages::data_clean),
        ("events detect", crate::stages::events_detect),
        ("labels build", crate::stages::labels_build),
        ("features build", crate::stages::features_build),
        ("baselines run", crate::stages::baselines_run),
        ("spotcheck", crate::stages::spotcheck),
        ("research verify", crate::research::verify),
    ];
    let total = Instant::now();
    for (name, f) in stages {
        let t = Instant::now();
        f(config).map_err(|e| anyhow::anyhow!("stage `{name}` failed: {e}"))?;
        let line = format!("{name}: ok in {:.1} s\n", t.elapsed().as_secs_f64());
        eprint!("{line}");
    }
    Ok(format!(
        "pipeline finished in {:.1} s",
        total.elapsed().as_secs_f64()
    ))
}
