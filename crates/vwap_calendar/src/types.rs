use chrono::NaiveDate;

/// The three scheduled releases. The order is the column order of the calendar features.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Cpi,
    Fomc,
    Nfp,
}

impl Kind {
    pub const ALL: [Kind; 3] = [Kind::Cpi, Kind::Fomc, Kind::Nfp];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Cpi => "cpi",
            Kind::Fomc => "fomc",
            Kind::Nfp => "nfp",
        }
    }

    pub fn from_label(s: &str) -> Option<Self> {
        Kind::ALL.into_iter().find(|k| k.label() == s)
    }
}

/// One scheduled release: the New York date, the release instant in UTC microseconds, and the
/// page it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarEvent {
    pub kind: Kind,
    pub date: NaiveDate,
    pub utc_us: i64,
    pub source: String,
}
