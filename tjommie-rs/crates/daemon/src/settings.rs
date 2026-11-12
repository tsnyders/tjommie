//! `settings.ts`: settings are rows of the `settings` table, one JSON value per key, over defaults.

use protocol::model::{Settings, SettingsPatch};
use protocol::relay::DEFAULT_RELAY_URL;
use protocol::remote::RelayUrl;
use protocol::wire::method;
use protocol::{ErrorCode, Event};
use rusqlite::Connection;
use serde_json::Value;

use crate::context::Context;
use crate::server::{Handlers, RpcError};

fn defaults() -> Settings {
    Settings {
        chain_limit: 5,
        relay_url: DEFAULT_RELAY_URL
            .map(|url| RelayUrl::new(url).expect("the default relay URL is valid")),
    }
}

/// Settings that pass the protocol's checks, or `None`.
fn checked(candidate: Value) -> Option<Settings> {
    protocol::validate("settings.get", &candidate).ok()?;
    serde_json::from_value(candidate).ok()
}

/// Stored values over the defaults. A stored value that is corrupt, or not a setting at all, leaves the current value.
pub fn get_settings(db: &Connection) -> Result<Settings, RpcError> {
    let mut statement = db.prepare("SELECT key, value FROM settings")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut settings = defaults();
    for row in rows {
        let (key, value) = row?;
        let Ok(value) = serde_json::from_str::<Value>(&value) else {
            continue;
        };
        let mut candidate = serde_json::to_value(&settings).expect("settings serialize");
        candidate[key.as_str()] = value;
        if let Some(next) = checked(candidate) {
            settings = next;
        }
    }
    Ok(settings)
}

/// Stores only the patched keys, so a later default (a new relay, say) still reaches settings nobody changed.
pub fn set_settings(db: &Connection, patch: &SettingsPatch) -> Result<Settings, RpcError> {
    let current = get_settings(db)?;
    let merged = Settings {
        chain_limit: patch.chain_limit.unwrap_or(current.chain_limit),
        relay_url: patch.relay_url.clone().unwrap_or(current.relay_url),
    };
    let merged = serde_json::to_value(&merged).expect("settings serialize");
    let settings = checked(merged.clone())
        .ok_or_else(|| RpcError::new(ErrorCode::BadParams, "Invalid settings"))?;
    let transaction = db.unchecked_transaction()?;
    let patched = serde_json::to_value(patch).expect("a settings patch serializes");
    for key in patched.as_object().expect("a patch is an object").keys() {
        transaction.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            (key, merged[key].to_string()),
        )?;
    }
    transaction.commit()?;
    Ok(settings)
}

pub fn register(handlers: Handlers, context: &Context) -> Handlers {
    let get = context.clone();
    let set = context.clone();
    handlers
        .on::<method::SettingsGet, _>(move |_, _| {
            let settings = get.with_db(get_settings);
            async move { settings }
        })
        .on::<method::SettingsSet, _>(move |patch, _| {
            let settings = set.with_db(|db| set_settings(db, &patch));
            if let Ok(settings) = &settings {
                set.events.send(Event::SettingsChanged(settings.clone()));
            }
            // A patch that names relayUrl (even as null) logs in to the relay again, or stops it (`daemon.ts:620-627`).
            let remote = set.late.remote.get().cloned().filter(|_| patch.relay_url.is_some());
            async move {
                if let Some(remote) = remote {
                    remote.relay_url_changed().await;
                }
                settings
            }
        })
}
