//! `setup.ts`: each provider account's CLI check, sign-in, sandbox setup and quota reads.

use protocol::model::Provider;
use serde_json::{Map, Value};

use crate::account_quota::LimitSnapshot;
use crate::providers::{LimitState, LimitWindow, js};

/// One provider account as the CLI sees it: its folder is the CLI's config home, so bots never load the person's own
/// CLI setup (hooks, plugins, skills, memories, MCP servers).
#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub id: String,
    pub provider: Provider,
    pub label: String,
    pub folder: String,
}

fn limit_state(used: f64) -> LimitState {
    if used >= 100.0 {
        LimitState::Reached
    } else if used >= 90.0 {
        LimitState::Near
    } else {
        LimitState::Ok
    }
}

/// A present value: neither missing nor `null` (JavaScript's `!= null`).
fn present<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a Value> {
    map.get(key).filter(|value| !value.is_null())
}

/// Codex's `account/rateLimits/read` as limit snapshots. A malformed or incomplete read is `None`: it must never clear
/// a known exhausted window.
pub fn parse_codex_account_limits(value: &Value) -> Option<Vec<LimitSnapshot>> {
    let value = value.as_object()?;
    let entries: Vec<(Option<&String>, &Value)> = match present(value, "rateLimitsByLimitId") {
        Some(multi) => multi
            .as_object()?
            .iter()
            .map(|(key, bucket)| (Some(key), bucket))
            .collect(),
        None => vec![(None, value.get("rateLimits").unwrap_or(&Value::Null))],
    };
    if entries.is_empty() || entries.len() > 32 {
        return None;
    }
    let mut snapshots = Vec::new();
    for (key, bucket) in entries {
        let bucket = bucket.as_object()?;
        let named = present(bucket, "limitId");
        let limit_id = match key {
            Some(key) => key.as_str(),
            None => named.map_or(Some("codex"), Value::as_str)?,
        };
        if limit_id.trim().is_empty()
            || js::utf16_len(limit_id) > 100
            || key.is_some_and(|key| named.is_some_and(|named| named.as_str() != Some(key)))
        {
            return None;
        }
        let mut windows = Vec::new();
        for window_key in ["primary", "secondary"] {
            let Some(window) = present(bucket, window_key) else {
                continue;
            };
            let window = window.as_object()?;
            let used = window
                .get("usedPercent")
                .and_then(Value::as_f64)
                .filter(|used| (0.0..=100.0).contains(used))?;
            let duration = match present(window, "windowDurationMins") {
                None => None,
                Some(duration) => Some(
                    duration
                        .as_f64()
                        .filter(|minutes| minutes.is_finite() && *minutes > 0.0)?,
                ),
            };
            let resets_at = match present(window, "resetsAt") {
                None => None,
                Some(reset) => Some(js::iso_from_millis(reset.as_f64()? * 1000.0)?),
            };
            windows.push(LimitWindow {
                window_key: window_key.into(),
                duration_minutes: duration,
                used_percent: Some(used),
                resets_at,
                state: limit_state(used),
            });
        }
        if let Some(reached) = present(bucket, "rateLimitReachedType") {
            if reached.as_str()?.trim().is_empty() {
                return None;
            }
            if !windows
                .iter()
                .any(|window| window.state == LimitState::Reached)
            {
                windows.push(LimitWindow {
                    window_key: "provider".into(),
                    duration_minutes: None,
                    used_percent: None,
                    resets_at: None,
                    state: LimitState::Reached,
                });
            }
        }
        if windows.is_empty() {
            return None;
        }
        snapshots.push(LimitSnapshot {
            limit_id: limit_id.to_string(),
            windows,
        });
    }
    Some(snapshots)
}
