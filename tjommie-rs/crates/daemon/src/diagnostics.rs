//! `diagnostics.ts`: cached versions and one bounded, read-only database check. No account access, no CLI or model run.

use std::fs;
use std::path::Path;
use std::sync::LazyLock;

use protocol::diagnostics::{HostDiagnostics, ProviderVersion, StorageState};
use protocol::model::{Provider, ProviderSetup};
use protocol::remote::RelayState;
use protocol::wire::method;
use regex::Regex;
use rusqlite::Connection;

use crate::context::Context;
use crate::server::Handlers;

static VERSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9]{1,4}\.[0-9]{1,4}\.[0-9]{1,4}(?:-(?:alpha|beta|rc)(?:\.[0-9]{1,4})?)?$")
        .unwrap()
});

/// `DiagnosticVersion`: a plain version number, so no path, URL or secret can ride along.
fn version(value: Option<&str>) -> Option<String> {
    value
        .filter(|value| VERSION.is_match(value))
        .map(str::to_owned)
}

/// `setup` holds the cached CLI versions; the check never runs a CLI.
pub fn host_diagnostics(
    db: Option<&Connection>,
    setup: &[(Provider, Option<String>)],
    relay: RelayState,
    app_version: Option<&str>,
    daemon_version: Option<&str>,
) -> HostDiagnostics {
    let storage = match db
        .map(|db| db.query_row("PRAGMA quick_check(1)", [], |row| row.get::<_, String>(0)))
    {
        None => StorageState::Unavailable,
        Some(Ok(result)) if result == "ok" => StorageState::Ok,
        Some(_) => StorageState::Error,
    };
    let providers = [Provider::Claude, Provider::Codex]
        .map(|provider| ProviderVersion {
            provider,
            version: version(
                setup
                    .iter()
                    .find(|(entry, _)| *entry == provider)
                    .and_then(|(_, version)| version.as_deref()),
            ),
        })
        .to_vec();
    HostDiagnostics {
        app_version: version(app_version),
        daemon_version: version(daemon_version),
        providers,
        relay,
        storage,
    }
}

/// The desktop app's version from a packaged `build-info.json` in `directory`: a small regular file, never a link.
pub fn read_app_version(directory: &Path) -> Option<String> {
    let file = directory.join("build-info.json");
    let metadata = fs::symlink_metadata(&file).ok()?;
    if !metadata.is_file() || metadata.len() > 4096 {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(&fs::read_to_string(&file).ok()?).ok()?;
    version(value.get("appVersion")?.as_str())
}

pub fn register(handlers: Handlers, context: &Context) -> Handlers {
    let context = context.clone();
    handlers.on::<method::DiagnosticsGet, _>(move |_, _| {
        // M7 passes the relay state (daemon.ts:382).
        let setup: Vec<(Provider, Option<String>)> = context
            .accounts
            .provider_setup()
            .into_iter()
            .map(|setup| match setup {
                ProviderSetup::Claude(fields) => (Provider::Claude, fields.version),
                ProviderSetup::Codex(codex) => (Provider::Codex, codex.setup.version),
            })
            .collect();
        let report = context.with_db(|db| {
            Ok(host_diagnostics(
                Some(db),
                &setup,
                RelayState::Off,
                context.app_version.as_deref(),
                Some(env!("CARGO_PKG_VERSION")),
            ))
        });
        async move { report }
    })
}
