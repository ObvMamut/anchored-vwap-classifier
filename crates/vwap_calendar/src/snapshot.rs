//! Building, checking, writing and reading the frozen snapshot. Fails closed: anything that does
//! not match the configured expectations is an error and nothing is written.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use vwap_core::CalendarConfig;
use vwap_data::time::{ny_clock, utc_of_local};

use crate::error::CalendarError;
use crate::fetch::Fetcher;
use crate::parse::{parse_bls, parse_fomc};
use crate::types::{CalendarEvent, Kind};

/// Releases per year in the order cpi, fomc, nfp.
pub type Counts = BTreeMap<i32, [usize; 3]>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageRecord {
    pub url: String,
    pub sha256: String,
}

#[derive(Debug, Clone)]
pub struct Fetched {
    pub events: Vec<CalendarEvent>,
    pub pages: Vec<PageRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lock {
    pub retrieved_utc: String,
    pub csv_sha256: String,
    pub first_year: i32,
    pub last_year: i32,
    pub counts: Counts,
    pub pages: Vec<PageRecord>,
}

fn sha_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn minute_of(kind: Kind, cfg: &CalendarConfig) -> u32 {
    match kind {
        Kind::Cpi => cfg.cpi_minute,
        Kind::Fomc => cfg.fomc_minute,
        Kind::Nfp => cfg.nfp_minute,
    }
}

fn event(
    kind: Kind,
    date: NaiveDate,
    cfg: &CalendarConfig,
    source: &str,
) -> Result<CalendarEvent, CalendarError> {
    let utc_us = utc_of_local(date, minute_of(kind, cfg)).ok_or_else(|| {
        CalendarError::Validation(format!("{date} has no valid local release time"))
    })?;
    Ok(CalendarEvent {
        kind,
        date,
        utc_us,
        source: source.to_string(),
    })
}

/// Downloads and parses every configured page. The releases outside `first_year..=last_year`
/// are dropped. A BLS row whose time is not the configured one is an error.
pub fn fetch_all(f: &dyn Fetcher, cfg: &CalendarConfig) -> Result<Fetched, CalendarError> {
    let mut events: Vec<CalendarEvent> = Vec::new();
    let mut pages = Vec::new();
    let in_range = |d: NaiveDate| d.year() >= cfg.first_year && d.year() <= cfg.last_year;
    for (kind, urls) in [
        (Kind::Cpi, &cfg.cpi_urls),
        (Kind::Nfp, &cfg.nfp_urls),
        (Kind::Fomc, &cfg.fomc_urls),
    ] {
        for url in urls {
            let html = f.get(url)?;
            pages.push(PageRecord {
                url: url.clone(),
                sha256: sha_hex(html.as_bytes()),
            });
            let dates: Vec<NaiveDate> = if kind == Kind::Fomc {
                parse_fomc(&html, url)?
            } else {
                let want = minute_of(kind, cfg);
                let mut ds = Vec::new();
                for (d, minute) in parse_bls(&html, url)? {
                    if minute != want {
                        return Err(CalendarError::Validation(format!(
                            "{url}: the {} release of {d} is at minute {minute} of the day, expected {want}",
                            kind.label()
                        )));
                    }
                    ds.push(d);
                }
                ds
            };
            for d in dates.into_iter().filter(|d| in_range(*d)) {
                if !events.iter().any(|e| e.kind == kind && e.date == d) {
                    events.push(event(kind, d, cfg, url)?);
                }
            }
        }
    }
    events.sort_by_key(|e| (e.date, e.kind));
    Ok(Fetched { events, pages })
}

/// Checks the events against the configuration and returns the count per year and type.
pub fn validate(events: &[CalendarEvent], cfg: &CalendarConfig) -> Result<Counts, CalendarError> {
    let bad = |m: String| Err(CalendarError::Validation(m));
    let mut counts: Counts = (cfg.first_year..=cfg.last_year)
        .map(|y| (y, [0; 3]))
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    for e in events {
        if !seen.insert((e.kind, e.date)) {
            return bad(format!("{} on {} appears twice", e.kind.label(), e.date));
        }
        let Some(c) = counts.get_mut(&e.date.year()) else {
            return bad(format!(
                "{} on {} is outside {}..={}",
                e.kind.label(),
                e.date,
                cfg.first_year,
                cfg.last_year
            ));
        };
        if let Some(slot) = c.get_mut(Kind::ALL.iter().position(|k| *k == e.kind).unwrap_or(0)) {
            *slot += 1;
        }
        let clock = ny_clock(e.utc_us);
        if clock.date != e.date || clock.minute != minute_of(e.kind, cfg) {
            return bad(format!(
                "{} on {} is stamped {} minute {} New York time, expected minute {}",
                e.kind.label(),
                e.date,
                clock.date,
                clock.minute,
                minute_of(e.kind, cfg)
            ));
        }
    }
    for (year, c) in &counts {
        for (i, kind) in Kind::ALL.iter().enumerate() {
            let (lo, hi) = if *kind == Kind::Fomc {
                (cfg.fomc_min_per_year, cfg.fomc_max_per_year)
            } else {
                (cfg.monthly_min_per_year, cfg.monthly_max_per_year)
            };
            let n = c.get(i).copied().unwrap_or(0);
            if n < lo || n > hi {
                return bad(format!(
                    "{year}: {n} {} releases, expected {lo} to {hi}; the source page may have changed or be missing",
                    kind.label()
                ));
            }
        }
    }
    Ok(counts)
}

fn csv_of(events: &[CalendarEvent]) -> Result<String, CalendarError> {
    let mut sorted: Vec<&CalendarEvent> = events.iter().collect();
    sorted.sort_by_key(|e| (e.date, e.kind));
    let mut s = String::from("kind,date,utc_us,source\n");
    for e in sorted {
        if e.source.contains(',') || e.source.contains('\n') {
            return Err(CalendarError::Validation(format!(
                "the source address {} contains a comma or a line break",
                e.source
            )));
        }
        s.push_str(&format!(
            "{},{},{},{}\n",
            e.kind.label(),
            e.date,
            e.utc_us,
            e.source
        ));
    }
    Ok(s)
}

/// Validates, then writes `events.csv` and `lock.json` into `dir`.
pub fn write_snapshot(
    dir: &Path,
    fetched: &Fetched,
    cfg: &CalendarConfig,
    retrieved_utc: &str,
) -> Result<Lock, CalendarError> {
    let counts = validate(&fetched.events, cfg)?;
    let csv = csv_of(&fetched.events)?;
    let lock = Lock {
        retrieved_utc: retrieved_utc.to_string(),
        csv_sha256: sha_hex(csv.as_bytes()),
        first_year: cfg.first_year,
        last_year: cfg.last_year,
        counts,
        pages: fetched.pages.clone(),
    };
    let io = |p: &Path| {
        let s = p.display().to_string();
        move |source| CalendarError::Io {
            path: s.clone(),
            source,
        }
    };
    fs::create_dir_all(dir).map_err(io(dir))?;
    let (cp, lp) = (dir.join("events.csv"), dir.join("lock.json"));
    fs::write(&cp, csv).map_err(io(&cp))?;
    fs::write(&lp, serde_json::to_string_pretty(&lock)?).map_err(io(&lp))?;
    Ok(lock)
}

fn parse_csv(text: &str) -> Result<Vec<CalendarEvent>, CalendarError> {
    let bad = |m: String| CalendarError::Validation(format!("events.csv: {m}"));
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate().skip(1) {
        if line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.splitn(4, ',').collect();
        let [kind, date, utc, source] = f.as_slice() else {
            return Err(bad(format!("line {} has fewer than 4 fields", n + 1)));
        };
        out.push(CalendarEvent {
            kind: Kind::from_label(kind)
                .ok_or_else(|| bad(format!("line {}: unknown kind {kind}", n + 1)))?,
            date: date
                .parse()
                .map_err(|_| bad(format!("line {}: bad date {date}", n + 1)))?,
            utc_us: utc
                .parse()
                .map_err(|_| bad(format!("line {}: bad time {utc}", n + 1)))?,
            source: (*source).to_string(),
        });
    }
    Ok(out)
}

/// Reads the snapshot in `dir` and checks it against its lock. A changed CSV is an error.
pub fn read_snapshot(dir: &Path) -> Result<(Vec<CalendarEvent>, Lock), CalendarError> {
    let io = |p: &Path| {
        let s = p.display().to_string();
        move |source| CalendarError::Io {
            path: s.clone(),
            source,
        }
    };
    let (cp, lp) = (dir.join("events.csv"), dir.join("lock.json"));
    let csv = fs::read_to_string(&cp).map_err(io(&cp))?;
    let lock: Lock = serde_json::from_str(&fs::read_to_string(&lp).map_err(io(&lp))?)?;
    if sha_hex(csv.as_bytes()) != lock.csv_sha256 {
        return Err(CalendarError::Lock(format!(
            "{} differs from the hash in lock.json; rerun `vwap calendar fetch`",
            cp.display()
        )));
    }
    Ok((parse_csv(&csv)?, lock))
}

/// Events from a hand supplied CSV with the columns `kind,date` (for example
/// `cpi,2025-02-12`). The release time is the configured one. Use it when the pages cannot be
/// fetched; the result goes through the same validation and lock as a fetch.
pub fn import_csv(
    text: &str,
    cfg: &CalendarConfig,
    source: &str,
) -> Result<Fetched, CalendarError> {
    let bad = |m: String| CalendarError::Validation(format!("import: {m}"));
    let mut events = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || (n == 0 && line.starts_with("kind")) {
            continue;
        }
        let Some((k, d)) = line.split_once(',') else {
            return Err(bad(format!("line {} is not kind,date", n + 1)));
        };
        let kind = Kind::from_label(k.trim())
            .ok_or_else(|| bad(format!("line {}: unknown kind {k}", n + 1)))?;
        let date: NaiveDate = d
            .trim()
            .parse()
            .map_err(|_| bad(format!("line {}: bad date {d}", n + 1)))?;
        events.push(event(kind, date, cfg, source)?);
    }
    events.sort_by_key(|e| (e.date, e.kind));
    Ok(Fetched {
        events,
        pages: vec![PageRecord {
            url: source.to_string(),
            sha256: sha_hex(text.as_bytes()),
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::FixtureFetcher;

    fn cfg() -> CalendarConfig {
        vwap_core::load_calendar_config(Path::new("../../config")).unwrap()
    }

    const MONTH: [&str; 12] = [
        "Jan.", "Feb.", "March", "April", "May", "June", "July", "Aug.", "Sept.", "Oct.", "Nov.",
        "Dec.",
    ];

    fn bls_page(first: i32, last: i32, time: &str, skip: Option<(i32, usize)>) -> String {
        let mut s = String::from("<table>");
        for y in first..=last {
            for (m, name) in MONTH.iter().enumerate() {
                if skip == Some((y, m)) {
                    continue;
                }
                s.push_str(&format!(
                    "<tr><td>ref</td><td>{name} {}, {y}</td><td>{time}</td></tr>",
                    10 + m
                ));
            }
        }
        s.push_str("</table>");
        s
    }

    fn fomc_pages(c: &CalendarConfig) -> Vec<(String, String)> {
        let months = [1, 3, 5, 6, 7, 9, 11, 12];
        let names = [
            "January",
            "March",
            "May",
            "June",
            "July",
            "September",
            "November",
            "December",
        ];
        c.fomc_urls
            .iter()
            .enumerate()
            .map(|(i, url)| {
                let years: Vec<i32> = if i == 0 {
                    (2021..=c.last_year).collect()
                } else {
                    vec![2015 + i as i32]
                };
                let mut s = String::new();
                for y in years {
                    s.push_str(&format!("<h4>{y} FOMC Meetings</h4>"));
                    for (k, n) in names.iter().enumerate() {
                        let _ = months[k];
                        s.push_str(&format!("<div>{n} {}-{}</div>", 10 + k, 11 + k));
                    }
                }
                (url.clone(), s)
            })
            .collect()
    }

    fn fixtures(c: &CalendarConfig, skip: Option<(i32, usize)>, nfp_time: &str) -> FixtureFetcher {
        let mut f = FixtureFetcher::default();
        f.pages.insert(
            c.cpi_urls[0].clone(),
            bls_page(c.first_year, c.last_year, "08:30 AM", skip),
        );
        f.pages.insert(
            c.nfp_urls[0].clone(),
            bls_page(c.first_year, c.last_year, nfp_time, None),
        );
        for (u, s) in fomc_pages(c) {
            f.pages.insert(u, s);
        }
        f
    }

    #[test]
    fn complete_pages_give_a_valid_snapshot_with_the_right_instants() {
        let c = cfg();
        let got = fetch_all(&fixtures(&c, None, "08:30 AM"), &c).unwrap();
        let counts = validate(&got.events, &c).unwrap();
        assert_eq!(counts.len(), 10);
        assert!(counts.values().all(|v| *v == [12, 8, 12]));
        // 8:30 New York in January is 13:30 UTC, in July 12:30 UTC; 14:00 FOMC is 19:00 and 18:00
        let cpi_jan = got
            .events
            .iter()
            .find(|e| e.kind == Kind::Cpi && e.date.to_string() == "2020-01-10")
            .unwrap();
        assert_eq!((cpi_jan.utc_us / 3_600_000_000) % 24, 13);
        let fomc_jul = got
            .events
            .iter()
            .find(|e| e.kind == Kind::Fomc && e.date.to_string() == "2019-07-15")
            .unwrap();
        assert_eq!((fomc_jul.utc_us / 3_600_000_000) % 24, 18);
        assert_eq!(got.pages.len(), 2 + c.fomc_urls.len());
    }

    #[test]
    fn a_missing_release_fails_closed_and_names_the_year() {
        let c = cfg();
        let got = fetch_all(&fixtures(&c, Some((2018, 4)), "08:30 AM"), &c).unwrap();
        // 11 per year is accepted for the monthly releases
        assert!(validate(&got.events, &c).is_ok());
        let two = fetch_all(
            &fixtures(&c, Some((2018, 4)), "08:30 AM"),
            &CalendarConfig {
                monthly_min_per_year: 12,
                ..c.clone()
            },
        )
        .unwrap();
        let err = validate(
            &two.events,
            &CalendarConfig {
                monthly_min_per_year: 12,
                ..c
            },
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("2018") && err.contains("cpi"), "{err}");
    }

    #[test]
    fn a_wrong_release_time_is_an_error() {
        let c = cfg();
        let err = fetch_all(&fixtures(&c, None, "10:00 AM"), &c)
            .unwrap_err()
            .to_string();
        assert!(err.contains("expected 510"), "{err}");
    }

    #[test]
    fn an_unreachable_page_is_an_error() {
        let c = cfg();
        let mut f = fixtures(&c, None, "08:30 AM");
        f.pages.remove(&c.cpi_urls[0]);
        assert!(matches!(
            fetch_all(&f, &c),
            Err(CalendarError::Fetch { .. })
        ));
    }

    #[test]
    fn the_snapshot_round_trips_and_a_changed_file_is_refused() {
        let c = cfg();
        let got = fetch_all(&fixtures(&c, None, "08:30 AM"), &c).unwrap();
        let dir = std::env::temp_dir().join(format!("vwap_cal_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let lock = write_snapshot(&dir, &got, &c, "2026-01-01T00:00:00Z").unwrap();
        let (back, lock2) = read_snapshot(&dir).unwrap();
        assert_eq!(lock, lock2);
        assert_eq!(back.len(), got.events.len());
        assert!(
            back.iter()
                .zip(&got.events)
                .all(|(a, b)| a.kind == b.kind && a.date == b.date && a.utc_us == b.utc_us)
        );
        let mut csv = fs::read_to_string(dir.join("events.csv")).unwrap();
        csv.push_str("cpi,2016-02-01,0,x\n");
        fs::write(dir.join("events.csv"), csv).unwrap();
        assert!(matches!(read_snapshot(&dir), Err(CalendarError::Lock(_))));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn nothing_is_written_when_validation_fails() {
        let c = cfg();
        let mut got = fetch_all(&fixtures(&c, None, "08:30 AM"), &c).unwrap();
        got.events.pop();
        got.events.pop();
        got.events.pop();
        got.events.pop();
        let dir = std::env::temp_dir().join(format!("vwap_cal_bad_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert!(write_snapshot(&dir, &got, &c, "t").is_err());
        assert!(!dir.join("events.csv").exists());
    }

    #[test]
    fn a_hand_supplied_csv_goes_through_the_same_checks() {
        let c = CalendarConfig {
            first_year: 2020,
            last_year: 2020,
            ..cfg()
        };
        let mut text = String::from("kind,date\n");
        for m in 1..=12 {
            text.push_str(&format!("cpi,2020-{m:02}-13\nnfp,2020-{m:02}-06\n"));
        }
        for m in [1, 3, 4, 6, 7, 9, 10, 12] {
            text.push_str(&format!("fomc,2020-{m:02}-29\n"));
        }
        let got = import_csv(&text, &c, "owner supplied").unwrap();
        assert_eq!(validate(&got.events, &c).unwrap()[&2020], [12, 8, 12]);
        assert!(import_csv("kind,date\nxyz,2020-01-01\n", &c, "s").is_err());
        assert!(import_csv("kind,date\ncpi,not a date\n", &c, "s").is_err());
        // a duplicate is refused by the validation
        let dup = format!("{text}cpi,2020-01-13\n");
        let got = import_csv(&dup, &c, "s").unwrap();
        assert!(validate(&got.events, &c).is_err());
    }
}
