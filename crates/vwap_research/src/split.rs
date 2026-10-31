//! The three data segments. Each segment is a distinct type, and only the discovery
//! view can be built without a permit.

use vwap_core::{Class, ResearchConfig};

use crate::error::ResearchError;

/// One event with its label, features and the fields the statistics need.
#[derive(Debug, Clone)]
pub struct Sample {
    pub width: u32,
    pub event_id: u32,
    pub session: i32,
    pub year: i32,
    pub t_close_us: i64,
    pub t_end_us: i64,
    pub side: i8,
    pub class: Option<Class>,
    pub ambiguous: bool,
    pub outcome_ticks: f64,
    pub sigma_over_price: f64,
    /// Aligned with `vwap_features::feature_names()`.
    pub features: Vec<f64>,
}

/// Segment boundaries in UTC microseconds.
#[derive(Debug, Clone, Copy)]
pub struct Bounds {
    pub discovery_start: i64,
    pub confirm_start: i64,
    pub holdout_start: i64,
}

impl Bounds {
    pub fn from_config(c: &ResearchConfig) -> Self {
        Self {
            discovery_start: ResearchConfig::date_us(&c.discovery_start),
            confirm_start: ResearchConfig::date_us(&c.confirm_start),
            holdout_start: ResearchConfig::date_us(&c.holdout_start),
        }
    }
}

/// Events of the discovery segment: they close and their label ends before confirmation starts.
#[derive(Debug)]
pub struct DiscoveryView(Vec<Sample>);

impl DiscoveryView {
    pub fn select(all: &[Sample], b: &Bounds) -> Self {
        Self(
            all.iter()
                .filter(|s| s.t_close_us >= b.discovery_start && s.t_end_us < b.confirm_start)
                .cloned()
                .collect(),
        )
    }

    pub fn samples(&self) -> &[Sample] {
        &self.0
    }
}

/// Proof that the ledger allows a one shot confirmation of this card. Only the ledger can make one.
#[derive(Debug)]
pub struct ConfirmPermit {
    pub(crate) card_id: String,
}

impl ConfirmPermit {
    pub fn card_id(&self) -> &str {
        &self.card_id
    }
}

#[derive(Debug)]
pub struct ConfirmView(Vec<Sample>);

impl ConfirmView {
    pub fn select(all: &[Sample], b: &Bounds, _permit: &ConfirmPermit) -> Self {
        Self(
            all.iter()
                .filter(|s| s.t_close_us >= b.confirm_start && s.t_end_us < b.holdout_start)
                .cloned()
                .collect(),
        )
    }

    pub fn samples(&self) -> &[Sample] {
        &self.0
    }
}

/// Proof that the holdout lock was created for this run. Only `HoldoutPermit::acquire` makes one.
#[derive(Debug)]
pub struct HoldoutPermit(());

impl HoldoutPermit {
    /// Creates the lock file. Fails if it already exists, so the holdout runs once.
    pub fn acquire(lock_path: &std::path::Path, note: &str) -> Result<Self, ResearchError> {
        use std::io::Write;
        if let Some(dir) = lock_path.parent() {
            std::fs::create_dir_all(dir).map_err(|source| ResearchError::Io {
                path: dir.display().to_string(),
                source,
            })?;
        }
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(lock_path)
            .map_err(|e| ResearchError::HoldoutLocked(format!("{}: {e}", lock_path.display())))?;
        f.write_all(note.as_bytes())
            .map_err(|source| ResearchError::Io {
                path: lock_path.display().to_string(),
                source,
            })?;
        Ok(Self(()))
    }
}

#[derive(Debug)]
pub struct HoldoutView(Vec<Sample>);

impl HoldoutView {
    pub fn select(all: &[Sample], b: &Bounds, _permit: &HoldoutPermit) -> Self {
        Self(
            all.iter()
                .filter(|s| s.t_close_us >= b.holdout_start)
                .cloned()
                .collect(),
        )
    }

    pub fn samples(&self) -> &[Sample] {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(close: i64, end: i64) -> Sample {
        Sample {
            width: 15,
            event_id: 0,
            session: 0,
            year: 2020,
            t_close_us: close,
            t_end_us: end,
            side: 1,
            class: None,
            ambiguous: false,
            outcome_ticks: 0.0,
            sigma_over_price: 0.0,
            features: vec![],
        }
    }

    const B: Bounds = Bounds {
        discovery_start: 0,
        confirm_start: 100,
        holdout_start: 200,
    };

    #[test]
    fn discovery_view_excludes_events_that_touch_later_segments() {
        let all = [s(10, 50), s(90, 110), s(150, 160), s(250, 260)];
        let d = DiscoveryView::select(&all, &B);
        // the event whose label ends after the boundary is dropped from both sides
        assert_eq!(d.samples().len(), 1);
        assert!(d.samples().iter().all(|x| x.t_end_us < B.confirm_start));
        let p = ConfirmPermit {
            card_id: "T".into(),
        };
        let c = ConfirmView::select(&all, &B, &p);
        assert_eq!(c.samples().len(), 1);
        assert!(
            c.samples()
                .iter()
                .all(|x| x.t_close_us >= B.confirm_start && x.t_end_us < B.holdout_start)
        );
    }

    #[test]
    fn holdout_lock_works_once() {
        let dir = std::env::temp_dir().join(format!("vwap_lock_test_{}", std::process::id()));
        let lock = dir.join("holdout.lock");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(HoldoutPermit::acquire(&lock, "first").is_ok());
        assert!(matches!(
            HoldoutPermit::acquire(&lock, "second"),
            Err(ResearchError::HoldoutLocked(_))
        ));
        let all = [s(250, 260), s(10, 20)];
        let p = HoldoutPermit(());
        assert_eq!(HoldoutView::select(&all, &B, &p).samples().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
