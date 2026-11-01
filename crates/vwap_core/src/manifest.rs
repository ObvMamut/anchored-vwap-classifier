//! Stage manifests: what a stage read, with which configuration, and what it produced.
//! They make stale inputs detectable and tie every output to a git commit.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::CoreError;

pub fn sha256_bytes(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn sha256_file(path: &Path) -> Result<String, CoreError> {
    let bytes = fs::read(path).map_err(|source| CoreError::Io {
        path: path.display().to_string(),
        source,
    })?;
    Ok(sha256_bytes(&bytes))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub stage: String,
    /// SHA256 of each configuration file the stage used, by file name.
    pub config_sha256: BTreeMap<String, String>,
    /// Fingerprint of the cleaned data the stage read, if it read any.
    pub data_fingerprint: Option<String>,
    pub rows: BTreeMap<String, u64>,
    pub git_commit: String,
    pub time_utc: String,
}

/// Hashes `<dir>/<name>` for each name.
pub fn config_hashes(dir: &Path, names: &[&str]) -> Result<BTreeMap<String, String>, CoreError> {
    names
        .iter()
        .map(|n| Ok(((*n).to_string(), sha256_file(&dir.join(n))?)))
        .collect()
}

impl Manifest {
    pub fn new(
        stage: &str,
        config_sha256: BTreeMap<String, String>,
        data_fingerprint: Option<String>,
        git_commit: &str,
    ) -> Self {
        Self {
            stage: stage.to_string(),
            config_sha256,
            data_fingerprint,
            rows: BTreeMap::new(),
            git_commit: git_commit.to_string(),
            time_utc: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Writes `<dir>/manifest.json`.
    pub fn write(&self, dir: &Path) -> Result<(), CoreError> {
        fs::create_dir_all(dir).map_err(|source| CoreError::Io {
            path: dir.display().to_string(),
            source,
        })?;
        let path = dir.join("manifest.json");
        fs::write(&path, serde_json::to_string_pretty(self)?).map_err(|source| CoreError::Io {
            path: path.display().to_string(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_the_known_empty_and_abc_digests() {
        assert_eq!(
            sha256_bytes(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn a_manifest_round_trips_through_json() {
        let dir = std::env::temp_dir().join(format!("vwap_manifest_{}", std::process::id()));
        let mut m = Manifest::new("events", BTreeMap::new(), Some("fp".into()), "abc123");
        m.rows.insert("events_5m".into(), 10);
        m.write(&dir).unwrap();
        let text = fs::read_to_string(dir.join("manifest.json")).unwrap();
        let back: Manifest = serde_json::from_str(&text).unwrap();
        assert_eq!(back.stage, "events");
        assert_eq!(back.data_fingerprint.as_deref(), Some("fp"));
        assert_eq!(back.rows.get("events_5m"), Some(&10));
        let _ = fs::remove_dir_all(&dir);
    }
}
