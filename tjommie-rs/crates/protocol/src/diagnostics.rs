//! `diagnostics.ts`

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::model::Provider;
use crate::remote::RelayState;
use crate::{Invalid, Wire, parse};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StorageState {
    Ok,
    Unavailable,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderVersion {
    pub provider: Provider,
    /// `major.minor.patch`, optionally `-alpha`, `-beta` or `-rc` with a number.
    pub version: Option<String>,
}

/// `diagnostics.get`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostDiagnostics {
    pub app_version: Option<String>,
    pub daemon_version: Option<String>,
    pub providers: Vec<ProviderVersion>,
    pub relay: RelayState,
    pub storage: StorageState,
}

// The exported report only round-trips through Rust, so its enum fields stay strings; the schema checks their values.

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportDevice {
    pub platform: String,
    pub app_version: Option<String>,
    pub file_bridge: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportHost {
    pub selection: String,
    pub app_version: Option<String>,
    pub daemon_version: Option<String>,
    pub providers: Vec<ProviderVersion>,
    pub relay: String,
    pub storage: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportConnection {
    pub status: String,
    pub issue: Option<String>,
    pub route: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportSpeech {
    pub engine: String,
    pub availability: String,
    pub permission: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticReport {
    /// Always 1.
    pub report_version: u8,
    pub device: ReportDevice,
    pub host: ReportHost,
    pub connection: ReportConnection,
    pub speech: ReportSpeech,
}

impl Wire for DiagnosticReport {
    const NAME: &'static str = "DiagnosticReport";
}

/// `parseDiagnosticReport`: at most 16384 UTF-8 bytes of report JSON, returned in the schema's key order with two-space
/// indents like `JSON.stringify(report, null, 2)`.
pub fn parse_diagnostic_report(value: &str) -> Result<String, Invalid> {
    let invalid = || Invalid("Invalid diagnostic report.".into());
    if value.len() > 16384 {
        return Err(invalid());
    }
    let json: Value = serde_json::from_str(value).map_err(|_| invalid())?;
    let report: DiagnosticReport = parse(json).map_err(|_| invalid())?;
    Ok(serde_json::to_string_pretty(&report).expect("a report serializes"))
}
