//! Reading release dates out of the official pages. The parsers look at the visible text of a
//! page, not at its markup, so a change of tags does not break them; a change of wording does,
//! and then the snapshot validation fails closed.

use chrono::NaiveDate;
use regex::Regex;

use crate::error::CalendarError;

const MONTHS: [&str; 12] = [
    "january",
    "february",
    "march",
    "april",
    "may",
    "june",
    "july",
    "august",
    "september",
    "october",
    "november",
    "december",
];

fn month_of(word: &str) -> Option<u32> {
    let prefix: String = word.to_lowercase().chars().take(3).collect();
    if prefix.len() < 3 {
        return None;
    }
    MONTHS
        .iter()
        .position(|m| m.starts_with(&prefix))
        .map(|i| i as u32 + 1)
}

fn re(url: &str, pattern: &str) -> Result<Regex, CalendarError> {
    Regex::new(pattern).map_err(|e| CalendarError::Parse {
        url: url.to_string(),
        reason: e.to_string(),
    })
}

/// The visible text of an HTML page: scripts, styles and tags removed, the common entities
/// decoded, white space collapsed.
pub fn visible_text(html: &str) -> String {
    let mut s = html.to_string();
    for tag in ["script", "style"] {
        loop {
            let lower = s.to_lowercase();
            let Some(a) = lower.find(&format!("<{tag}")) else {
                break;
            };
            let Some(b) = lower[a..].find(&format!("</{tag}>")) else {
                s.truncate(a);
                break;
            };
            s.replace_range(a..a + b + tag.len() + 3, " ");
        }
    }
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' if in_tag => in_tag = false,
            _ if in_tag => {}
            _ => out.push(c),
        }
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&#8211;", "-")
        .replace("&ndash;", "-")
        .replace("&#8212;", "-")
        .replace("&mdash;", "-")
        .replace(['\u{2013}', '\u{2014}'], "-")
        .replace('\u{a0}', " ");
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A BLS release schedule: every "Month D, YYYY" followed by a time like "08:30 AM". Returns
/// the date and the New York minute of day.
pub fn parse_bls(html: &str, url: &str) -> Result<Vec<(NaiveDate, u32)>, CalendarError> {
    let text = visible_text(html);
    let r = re(
        url,
        r"([A-Za-z]{3,9})\.?\s+(\d{1,2}),\s+(\d{4})\s+(\d{1,2}):(\d{2})\s*([AaPp])\.?[Mm]",
    )?;
    let mut out = Vec::new();
    for c in r.captures_iter(&text) {
        let (Some(m), Some(d), Some(y), Some(h), Some(mi), Some(ap)) = (
            c.get(1).and_then(|x| month_of(x.as_str())),
            c.get(2).and_then(|x| x.as_str().parse::<u32>().ok()),
            c.get(3).and_then(|x| x.as_str().parse::<i32>().ok()),
            c.get(4).and_then(|x| x.as_str().parse::<u32>().ok()),
            c.get(5).and_then(|x| x.as_str().parse::<u32>().ok()),
            c.get(6).map(|x| x.as_str().to_lowercase()),
        ) else {
            continue;
        };
        let Some(date) = NaiveDate::from_ymd_opt(y, m, d) else {
            continue;
        };
        let hour = match (h, ap.as_str()) {
            (12, "a") => 0,
            (12, "p") => 12,
            (x, "p") => x + 12,
            (x, _) => x,
        };
        out.push((date, hour * 60 + mi));
    }
    Ok(out)
}

/// A Federal Reserve meeting calendar: the statement day of every meeting, which is the last
/// day of a "January 28-29" style range. Dates written with a year ("March 19, 2025", as in the
/// links to minutes and statements) are not meetings and are skipped. A page without "YYYY FOMC
/// Meetings" headers takes its year from the address.
pub fn parse_fomc(html: &str, url: &str) -> Result<Vec<NaiveDate>, CalendarError> {
    let text = visible_text(html);
    let head = re(url, r"(\d{4})\s+FOMC\s+Meetings")?;
    let tok = re(
        url,
        r"(January|February|March|April|May|June|July|August|September|October|November|December)\s+(\d{1,2})(?:\s*-\s*(\d{1,2}))?(\*?)(,\s*\d{4})?",
    )?;
    let url_year = re(url, r"(20\d{2})")?
        .captures(url)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse::<i32>().ok());
    let heads: Vec<(usize, i32)> = head
        .captures_iter(&text)
        .filter_map(|c| Some((c.get(0)?.start(), c.get(1)?.as_str().parse::<i32>().ok()?)))
        .collect();
    let mut out = Vec::new();
    for c in tok.captures_iter(&text) {
        if c.get(5).is_some() {
            continue;
        }
        let Some(pos) = c.get(0).map(|m| m.start()) else {
            continue;
        };
        let year = heads
            .iter()
            .rev()
            .find(|(p, _)| *p < pos)
            .map(|(_, y)| *y)
            .or(if heads.is_empty() { url_year } else { None });
        let (Some(year), Some(m), Some(last)) = (
            year,
            c.get(1).and_then(|x| month_of(x.as_str())),
            c.get(3)
                .or_else(|| c.get(2))
                .and_then(|x| x.as_str().parse::<u32>().ok()),
        ) else {
            continue;
        };
        if let Some(d) = NaiveDate::from_ymd_opt(year, m, last) {
            out.push(d);
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn visible_text_drops_markup_scripts_and_entities() {
        let html = "<html><head><style>.a{color:red}</style><script>var x = '<b>';</script></head><body><p>A&nbsp;&amp;<b> B</b></p></body></html>";
        assert_eq!(visible_text(html), "A & B");
    }

    #[test]
    fn bls_rows_give_the_date_and_the_time() {
        let html = r#"<table class="release-list"><tr><th>Reference Month</th><th>Release Date</th><th>Release Time</th></tr>
            <tr><td>December 2024</td><td>Jan. 15, 2025</td><td>08:30 AM</td></tr>
            <tr><td>January 2025</td><td>Feb. 12, 2025</td><td>08:30 AM</td></tr>
            <tr><td>August 2025</td><td>Sept. 11, 2025</td><td>08:30 AM</td></tr>
            <tr><td>Other</td><td>Oct. 1, 2025</td><td>2:00 PM</td></tr></table>"#;
        let rows = parse_bls(html, "u").unwrap();
        assert_eq!(
            rows,
            vec![
                (d(2025, 1, 15), 510),
                (d(2025, 2, 12), 510),
                (d(2025, 9, 11), 510),
                (d(2025, 10, 1), 840)
            ]
        );
        assert!(parse_bls("<p>nothing here</p>", "u").unwrap().is_empty());
    }

    #[test]
    fn midnight_and_noon_are_handled() {
        let rows = parse_bls("Jan. 2, 2020 12:00 AM and Jan. 3, 2020 12:05 PM", "u").unwrap();
        assert_eq!(rows, vec![(d(2020, 1, 2), 0), (d(2020, 1, 3), 12 * 60 + 5)]);
    }

    #[test]
    fn fomc_takes_the_last_day_of_each_meeting_and_skips_dated_links() {
        let html = r#"
            <h4><a id="a1">2025 FOMC Meetings</a></h4>
            <div class="fomc-meeting"><strong>January</strong> 28-29</div>
            <div>Statement: PDF | HTML  Minutes: Released February 19, 2025</div>
            <div><strong>March</strong> 18-19*</div>
            <div>Implementation Note issued March 19, 2025</div>
            <h4><a id="a2">2024 FOMC Meetings</a></h4>
            <div><strong>December</strong> 17-18</div>
            <div><strong>September</strong> 17</div>
        "#;
        let v = parse_fomc(html, "https://x/fomccalendars.htm").unwrap();
        assert_eq!(
            v,
            vec![
                d(2024, 9, 17),
                d(2024, 12, 18),
                d(2025, 1, 29),
                d(2025, 3, 19)
            ]
        );
    }

    #[test]
    fn a_historical_page_takes_its_year_from_the_address() {
        let html = "<div>January 26-27 Meeting</div><div>March 15-16</div><div>Statement March 16, 2016</div>";
        let v = parse_fomc(html, "https://x/fomchistorical2016.htm").unwrap();
        assert_eq!(v, vec![d(2016, 1, 27), d(2016, 3, 16)]);
        // no header and no year in the address: nothing can be dated
        assert!(parse_fomc(html, "https://x/fomc.htm").unwrap().is_empty());
    }
}
