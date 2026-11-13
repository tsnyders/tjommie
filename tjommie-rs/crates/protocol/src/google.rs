//! `google.ts`. Client configuration is write-only. Authorization URLs are for the trusted desktop opener; the UI gets the
//! attempt id and `google.changed` status, never tokens or client credentials.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::Invalid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoogleService {
    Gmail,
    Drive,
    Calendar,
}

/// One to three services, each once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct GoogleServices(Vec<GoogleService>);

impl GoogleServices {
    pub fn new(services: Vec<GoogleService>) -> Result<Self, Invalid> {
        let mut seen = HashSet::new();
        if services.iter().all(|service| seen.insert(*service)) {
            Ok(Self(services))
        } else {
            Err(Invalid("Choose each service once.".into()))
        }
    }

    pub fn as_slice(&self) -> &[GoogleService] {
        &self.0
    }
}

impl<'de> Deserialize<'de> for GoogleServices {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(Vec::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleClientConfig {
    pub client_id: String,
    pub client_secret: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoogleAttemptState {
    Waiting,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoogleAttempt {
    pub id: String,
    pub state: GoogleAttemptState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleStatus {
    pub configured: bool,
    pub connected: bool,
    pub email: Option<String>,
    pub services: Vec<GoogleService>,
    pub granted_bot_ids: Vec<String>,
    pub attempt: Option<GoogleAttempt>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoogleAuthorization {
    pub attempt: String,
    pub url: WebUrl,
}

/// `z.string().url()`: any URL a browser can parse.
fn web_url(value: &str) -> Option<String> {
    crate::remote::zod_url(value).map(|(_, text)| text)
}

refined_string!(
    /// A URL, stored trimmed and without tabs or newlines.
    WebUrl, web_url, "Invalid URL"
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoogleBegin {
    pub services: GoogleServices,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoogleCancel {
    pub attempt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GoogleGrant {
    pub bot_id: String,
    pub enabled: bool,
}
