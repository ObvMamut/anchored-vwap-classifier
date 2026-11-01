//! Getting the pages. `HttpFetcher` is the only code that touches the network; tests use
//! `FixtureFetcher`.

use std::collections::BTreeMap;
use std::time::Duration;

use crate::error::CalendarError;

pub trait Fetcher {
    fn get(&self, url: &str) -> Result<String, CalendarError>;
}

pub struct HttpFetcher {
    agent: ureq::Agent,
    user_agent: String,
}

impl HttpFetcher {
    pub fn new(timeout_seconds: u64, user_agent: &str) -> Self {
        Self {
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(timeout_seconds))
                .build(),
            user_agent: user_agent.to_string(),
        }
    }
}

impl Fetcher for HttpFetcher {
    fn get(&self, url: &str) -> Result<String, CalendarError> {
        let fail = |reason: String| CalendarError::Fetch {
            url: url.to_string(),
            reason,
        };
        self.agent
            .get(url)
            .set("User-Agent", &self.user_agent)
            .call()
            .map_err(|e| fail(e.to_string()))?
            .into_string()
            .map_err(|e| fail(e.to_string()))
    }
}

/// Saved pages by address, for the tests.
#[derive(Debug, Default, Clone)]
pub struct FixtureFetcher {
    pub pages: BTreeMap<String, String>,
}

impl Fetcher for FixtureFetcher {
    fn get(&self, url: &str) -> Result<String, CalendarError> {
        self.pages
            .get(url)
            .cloned()
            .ok_or_else(|| CalendarError::Fetch {
                url: url.to_string(),
                reason: "not in the fixtures".into(),
            })
    }
}
