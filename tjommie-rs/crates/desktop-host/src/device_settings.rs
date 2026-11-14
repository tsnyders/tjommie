//! Device-local settings (Phase 3 plan T1.5): `<root>/data/desktop-ui.json` holds the theme, text size, density and zoom,
//! the PCs this device is paired with (and the ones whose pairing hasn't finished), and the "Tjommie is still running"
//! tray hint marker. The first time the file is missing, the settings the 0.5.x Electron app exported
//! (`<root>/data/desktop-ui-export.json`, written by `packages/desktop/src/desktop-ui-export.ts`) are imported once.
//!
//! The device keys the hosts refer to are already in Credential Manager under the root's namespace, so they carry over
//! without an export (`device_keys`). App zoom is not exported (Electron keeps it in Chromium's preferences), so it starts at 100%.

use std::io::Write as _;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use protocol::remote::{LanEndpoint, RelayUrl, RemoteKey, RemoteName};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// At most this many paired PCs, and this many pending ones (`remote-hosts.ts`).
pub const MAX_HOSTS: usize = 16;
pub const MIN_ZOOM_PERCENT: u16 = 25;
pub const MAX_ZOOM_PERCENT: u16 = 500;
const VERSION: u32 = 1;
const EXPORT_FIELDS: [&str; 6] = [
    "version", "theme", "textSize", "density", "hosts", "pending",
];

/// Dark is the default, as in the Electron app: its page was dark until the owner chose otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    System,
    Light,
    #[default]
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextSize {
    #[default]
    System,
    Large,
    Larger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    #[default]
    Comfortable,
    Compact,
}

/// A PC this device is paired with (`RemoteHostSchema` in `remote-hosts.ts`). Validated when read: an `id` or `deviceId`
/// that isn't a UUID, no LAN address or more than 16, an address that isn't an IP address or has port 0, an unknown field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "RawHost", rename_all = "camelCase")]
pub struct RemoteHost {
    pub id: String,
    pub name: RemoteName,
    pub device_id: Option<String>,
    pub host_key: RemoteKey,
    pub lan: Vec<LanEndpoint>,
    pub relay: Option<RelayUrl>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RawHost {
    id: String,
    name: RemoteName,
    device_id: Option<String>,
    host_key: RemoteKey,
    lan: Vec<RawLan>,
    #[serde(default)]
    relay: Option<RelayUrl>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLan {
    host: String,
    port: u16,
}

/// zod's `z.uuid()`: the canonical spelling with a version 1 to 8 and an RFC variant, or the nil or max UUID.
fn is_uuid(text: &str) -> bool {
    if text == "00000000-0000-0000-0000-000000000000"
        || text.eq_ignore_ascii_case("ffffffff-ffff-ffff-ffff-ffffffffffff")
    {
        return true;
    }
    let groups: Vec<&str> = text.split('-').collect();
    let [a, b, c, d, e] = groups.as_slice() else {
        return false;
    };
    let hex = |part: &str, length: usize| {
        part.len() == length && part.bytes().all(|byte| byte.is_ascii_hexdigit())
    };
    hex(a, 8)
        && hex(b, 4)
        && hex(c, 4)
        && hex(d, 4)
        && hex(e, 12)
        && c.bytes()
            .next()
            .is_some_and(|byte| (b'1'..=b'8').contains(&byte))
        && d.bytes()
            .next()
            .is_some_and(|byte| matches!(byte, b'8' | b'9' | b'a' | b'b' | b'A' | b'B'))
}

impl TryFrom<RawHost> for RemoteHost {
    type Error = String;

    fn try_from(raw: RawHost) -> Result<Self, String> {
        if !is_uuid(&raw.id) {
            return Err("id is not a UUID".to_owned());
        }
        if raw.device_id.as_deref().is_some_and(|id| !is_uuid(id)) {
            return Err("deviceId is not a UUID".to_owned());
        }
        if raw.lan.is_empty() || raw.lan.len() > MAX_HOSTS {
            return Err(format!("lan needs 1 to {MAX_HOSTS} addresses"));
        }
        let mut lan = Vec::with_capacity(raw.lan.len());
        for (at, address) in raw.lan.into_iter().enumerate() {
            if address.host.parse::<IpAddr>().is_err() {
                return Err(format!("lan[{at}].host is not an IP address"));
            }
            if address.port == 0 {
                return Err(format!("lan[{at}].port is not a port"));
            }
            lan.push(LanEndpoint {
                host: address.host,
                port: address.port,
            });
        }
        Ok(RemoteHost {
            id: raw.id,
            name: raw.name,
            device_id: raw.device_id,
            host_key: raw.host_key,
            lan,
            relay: raw.relay,
        })
    }
}

/// Everything in `desktop-ui.json`. A field missing from the file takes its default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase", default)]
pub struct DeviceSettings {
    pub version: u32,
    pub theme: Theme,
    pub text_size: TextSize,
    pub density: Density,
    /// App zoom as a percentage, 25 to 500.
    pub zoom_percent: u16,
    pub hosts: Vec<RemoteHost>,
    /// Pairings that started and haven't finished (`tjommie.mobile.pending.v1`).
    pub pending: Vec<RemoteHost>,
    /// The "Tjommie is still running in the tray" notice has been shown once.
    pub tray_hint_shown: bool,
}

impl Default for DeviceSettings {
    fn default() -> Self {
        Self {
            version: VERSION,
            theme: Theme::default(),
            text_size: TextSize::default(),
            density: Density::default(),
            zoom_percent: 100,
            hosts: Vec::new(),
            pending: Vec::new(),
            tray_hint_shown: false,
        }
    }
}

pub fn settings_file(root: &Path) -> PathBuf {
    root.join("data").join("desktop-ui.json")
}

/// What the Electron 0.5.x app left for this app to import.
pub fn export_file(root: &Path) -> PathBuf {
    root.join("data").join("desktop-ui-export.json")
}

fn check(settings: &DeviceSettings) -> Result<(), String> {
    if settings.version != VERSION {
        return Err(format!("it is version {}, not {VERSION}", settings.version));
    }
    if settings.hosts.len() > MAX_HOSTS || settings.pending.len() > MAX_HOSTS {
        return Err(format!(
            "it has more than {MAX_HOSTS} paired or pending PCs"
        ));
    }
    if !(MIN_ZOOM_PERCENT..=MAX_ZOOM_PERCENT).contains(&settings.zoom_percent) {
        return Err("its zoom is out of range".to_owned());
    }
    Ok(())
}

fn parse_settings(text: &str) -> Result<DeviceSettings, String> {
    // serde's own message can quote the value it choked on, so only where is kept.
    let settings: DeviceSettings = serde_json::from_str(text).map_err(|error| {
        format!(
            "it doesn't match the settings format (line {}, column {})",
            error.line(),
            error.column()
        )
    })?;
    check(&settings)?;
    Ok(settings)
}

/// Reads `<root>/data/desktop-ui.json`. When that file is missing, imports the 0.5.x export once (or starts from the
/// defaults when there is none), writes the result, and logs which. A file that can't be understood is kept beside it as
/// `desktop-ui.json.bad` and the defaults are used, so a damaged file never stops the app from starting.
pub fn load_or_import(root: &Path) -> DeviceSettings {
    let file = settings_file(root);
    match std::fs::read_to_string(&file) {
        Ok(text) => match parse_settings(&text) {
            Ok(settings) => settings,
            Err(why) => {
                let aside = file.with_extension("json.bad");
                log::warn!(
                    "device settings: {} can't be read ({why}); keeping a copy as {} and starting from the defaults",
                    file.display(),
                    aside.display()
                );
                let _ = std::fs::rename(&file, &aside);
                let settings = DeviceSettings::default();
                persist(root, &settings);
                settings
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let settings = import(root);
            persist(root, &settings);
            settings
        }
        Err(error) => {
            // Locked or unreadable, not missing: don't overwrite what may be good.
            log::warn!(
                "device settings: {} can't be read ({error}); using the defaults for this run",
                file.display()
            );
            DeviceSettings::default()
        }
    }
}

fn persist(root: &Path, settings: &DeviceSettings) {
    if let Err(error) = save(root, settings) {
        log::warn!(
            "device settings: couldn't write {} ({error})",
            settings_file(root).display()
        );
    }
}

/// Writes `<root>/data/desktop-ui.json` atomically: a temporary file in the same folder, flushed to disk, then renamed over
/// the real one, so a crash leaves the old file or the new one and never half of either.
pub fn save(root: &Path, settings: &DeviceSettings) -> std::io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    check(settings).map_err(|why| std::io::Error::new(std::io::ErrorKind::InvalidInput, why))?;
    let file = settings_file(root);
    if let Some(folder) = file.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let text = serde_json::to_string_pretty(settings).map_err(std::io::Error::other)?;
    let temporary = file.with_extension(format!(
        "json.{}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let written = (|| {
        let mut out = std::fs::File::create(&temporary)?;
        out.write_all(text.as_bytes())?;
        out.write_all(b"\n")?;
        out.sync_all()?;
        drop(out);
        std::fs::rename(&temporary, &file)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written
}

// ---- the import from the Electron app ----

/// The settings in the export, or the defaults when there is none or it can't be trusted at all. Either way, logged.
fn import(root: &Path) -> DeviceSettings {
    let file = export_file(root);
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            log::info!(
                "device settings: no export from the Electron app at {}; starting from the defaults",
                file.display()
            );
            return DeviceSettings::default();
        }
        Err(error) => {
            log::warn!(
                "device settings: the export at {} can't be read ({error}), so it was not imported",
                file.display()
            );
            return DeviceSettings::default();
        }
    };
    match parse_export(&text, &file) {
        Ok(settings) => {
            log::info!(
                "device settings: imported the Electron app's settings from {} ({} paired PCs, {} pending)",
                file.display(),
                settings.hosts.len(),
                settings.pending.len()
            );
            settings
        }
        Err(why) => {
            log::warn!(
                "device settings: the export at {} was not imported: {why}",
                file.display()
            );
            DeviceSettings::default()
        }
    }
}

/// Every field is checked. A file that isn't a version 1 export (or has a field it doesn't know) is not imported at all.
/// Otherwise a bad value gives its field's default, a bad host is dropped, and the log says which (never what it held).
fn parse_export(text: &str, source: &Path) -> Result<DeviceSettings, String> {
    let Value::Object(fields) =
        serde_json::from_str::<Value>(text).map_err(|_| "it is not valid JSON".to_owned())?
    else {
        return Err("it is not an object".to_owned());
    };
    if let Some(unknown) = fields
        .keys()
        .find(|key| !EXPORT_FIELDS.contains(&key.as_str()))
    {
        return Err(format!(
            "it has a field this app doesn't know (`{unknown}`)"
        ));
    }
    if fields.get("version").and_then(Value::as_u64) != Some(u64::from(VERSION)) {
        return Err(format!("it is not a version {VERSION} export"));
    }
    let defaults = DeviceSettings::default();
    Ok(DeviceSettings {
        theme: choice(&fields, "theme", defaults.theme, source),
        text_size: choice(&fields, "textSize", defaults.text_size, source),
        density: choice(&fields, "density", defaults.density, source),
        hosts: hosts(&fields, "hosts", source),
        pending: hosts(&fields, "pending", source),
        ..defaults
    })
}

fn choice<T: serde::de::DeserializeOwned>(
    fields: &Map<String, Value>,
    key: &str,
    default: T,
    source: &Path,
) -> T {
    let Some(value) = fields.get(key) else {
        return default;
    };
    serde_json::from_value(value.clone()).unwrap_or_else(|_| {
        log::warn!(
            "device settings: {}: dropped {key}, which is not one of its choices",
            source.display()
        );
        default
    })
}

fn hosts(fields: &Map<String, Value>, key: &str, source: &Path) -> Vec<RemoteHost> {
    let Some(value) = fields.get(key) else {
        return Vec::new();
    };
    let Value::Array(entries) = value else {
        log::warn!(
            "device settings: {}: dropped {key}, which is not a list",
            source.display()
        );
        return Vec::new();
    };
    let mut kept = Vec::new();
    for (at, entry) in entries.iter().enumerate() {
        match serde_json::from_value::<RemoteHost>(entry.clone()) {
            Ok(host) => kept.push(host),
            Err(error) => log::warn!(
                "device settings: {}: dropped {key}[{at}]: {}",
                source.display(),
                // serde names the field that failed (and a type's own refusal), not the value.
                error
                    .to_string()
                    .split(" at line")
                    .next()
                    .unwrap_or("it is not a valid PC")
            ),
        }
    }
    if kept.len() > MAX_HOSTS {
        log::warn!(
            "device settings: {}: kept the first {MAX_HOSTS} of {} entries in {key}",
            source.display(),
            kept.len()
        );
        kept.truncate(MAX_HOSTS);
    }
    kept
}
