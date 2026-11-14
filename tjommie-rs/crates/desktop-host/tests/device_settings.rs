//! Device-local settings and the hand-over from the 0.5.x Electron app (Phase 3 plan T1.5): `<root>/data/desktop-ui.json`,
//! and the one-time import of `<root>/data/desktop-ui-export.json`, which the Electron 0.5.x patch (T1.6) writes.

// Tests may panic. clippy.toml allows that inside #[test] functions, but not in the helpers beside them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::sync::{Mutex, Once};

use desktop_host::device_settings::{
    Density, DeviceSettings, MAX_HOSTS, RemoteHost, TextSize, Theme, export_file, load_or_import,
    save, settings_file,
};
use serde_json::{Value, json};

// ---- a logger that keeps what was logged, so tests can check the import was recorded ----

static LINES: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Capture;

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }
    fn log(&self, record: &log::Record<'_>) {
        LINES
            .lock()
            .unwrap()
            .push(format!("{} {}", record.level(), record.args()));
    }
    fn flush(&self) {}
}

fn logged_for(root: &Path) -> Vec<String> {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        log::set_logger(&Capture).unwrap();
        log::set_max_level(log::LevelFilter::Info);
    });
    let marker = root.to_string_lossy().into_owned();
    LINES
        .lock()
        .unwrap()
        .iter()
        .filter(|line| line.contains(&marker))
        .cloned()
        .collect()
}

/// Makes sure the capturing logger is installed before the code under test logs.
fn root() -> tempfile::TempDir {
    let _ = logged_for(Path::new("-"));
    tempfile::tempdir().unwrap()
}

fn key(byte: u8) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([byte; 32])
}

fn host(n: u8, changes: Value) -> Value {
    let mut host = json!({
        "id": format!("5b0e2c8e-6f1a-4c55-9a43-2f0d7b1e9c{n:02x}"),
        "name": format!("Synthetic PC {n}"),
        "deviceId": null,
        "hostKey": key(n),
        "lan": [{ "host": "192.168.1.20", "port": 47101 }],
        "relay": null,
    });
    if let (Some(host), Some(changes)) = (host.as_object_mut(), changes.as_object()) {
        for (name, value) in changes {
            host.insert(name.clone(), value.clone());
        }
    }
    host
}

fn write(path: PathBuf, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn write_export(root: &Path, export: &Value) {
    write(export_file(root), &export.to_string());
}

fn full_export() -> Value {
    json!({
        "version": 1, "theme": "light", "textSize": "large", "density": "compact",
        "hosts": [host(1, json!({ "deviceId": "8d1f4c1e-2b7a-4e0b-b1d3-0a6c5e9f7a22", "relay": "https://relay.example.workers.dev" })), host(2, json!({}))],
        "pending": [host(3, json!({}))],
    })
}

fn names(hosts: &[RemoteHost]) -> Vec<&str> {
    hosts.iter().map(|host| host.name.as_str()).collect()
}

#[test]
fn imports_export_once() {
    let root = root();
    write_export(root.path(), &full_export());
    let first = load_or_import(root.path());
    assert_eq!(first.theme, Theme::Light);
    assert_eq!(first.text_size, TextSize::Large);
    assert_eq!(first.density, Density::Compact);
    assert_eq!(names(&first.hosts), ["Synthetic PC 1", "Synthetic PC 2"]);
    assert_eq!(names(&first.pending), ["Synthetic PC 3"]);
    assert_eq!(
        first.zoom_percent, 100,
        "zoom is not exported: it starts at 100%"
    );
    assert!(!first.tray_hint_shown);
    assert!(
        settings_file(root.path()).exists(),
        "the import is written, so it happens once"
    );
    let lines = logged_for(root.path());
    assert!(
        lines
            .iter()
            .any(|line| line.starts_with("INFO") && line.contains("imported")),
        "{lines:?}"
    );

    // The export changes afterwards (the 0.5.x app is still running, say): the new app has its own settings now.
    write_export(
        root.path(),
        &json!({ "version": 1, "theme": "dark", "textSize": "system", "density": "comfortable", "hosts": [], "pending": [] }),
    );
    assert_eq!(load_or_import(root.path()), first);
}

#[test]
fn rejects_bad_export_field() {
    let root = root();
    write_export(
        root.path(),
        &json!({
            "version": 1, "theme": "neon", "textSize": "larger", "density": "crowded",
            "hosts": [host(1, json!({})), host(2, json!({ "hostKey": "not-a-key" })), host(4, json!({ "lastSeen": "yesterday" })), host(5, json!({ "lan": [{ "host": "not an address", "port": 47101 }] })), host(6, json!({ "lan": [{ "host": "10.0.0.1", "port": 0 }] })), host(7, json!({ "id": "nope" })), "nonsense", host(8, json!({}))],
            "pending": { "not": "a list" },
        }),
    );
    let settings = load_or_import(root.path());
    // A bad field gives that field's default, and the rest of the export still comes across.
    assert_eq!(
        (settings.theme, settings.density),
        (Theme::Dark, Density::Comfortable)
    );
    assert_eq!(settings.text_size, TextSize::Larger);
    assert_eq!(names(&settings.hosts), ["Synthetic PC 1", "Synthetic PC 8"]);
    assert!(settings.pending.is_empty());
    let lines = logged_for(root.path()).join("\n");
    for field in ["theme", "density", "pending", "hosts"] {
        assert!(lines.contains(field), "the log names {field}: {lines}");
    }
    // The log says which field and entry, never what was in it.
    assert!(
        !lines.contains("not-a-key")
            && !lines.contains("neon")
            && !lines.contains("not an address"),
        "{lines}"
    );
}

#[test]
fn an_export_that_cannot_be_trusted_at_all_is_not_imported() {
    for (name, export) in [
        ("another version", json!({ "version": 2, "theme": "light" })),
        (
            "an unknown field",
            json!({ "version": 1, "theme": "light", "extra": 1 }),
        ),
        ("not an object", json!(["theme"])),
    ] {
        let root = root();
        write_export(root.path(), &export);
        assert_eq!(
            load_or_import(root.path()),
            DeviceSettings::default(),
            "{name}"
        );
        assert!(
            logged_for(root.path())
                .iter()
                .any(|line| line.contains("not imported")),
            "{name}"
        );
    }
    let root = root();
    write(export_file(root.path()), "{ not json");
    assert_eq!(load_or_import(root.path()), DeviceSettings::default());
    assert!(
        logged_for(root.path())
            .iter()
            .any(|line| line.contains("not imported"))
    );
}

#[test]
fn a_host_name_or_relay_outside_the_limits_is_dropped() {
    // zod's RemoteName is trimmed, 1 to 80 characters; the relay is at most 512.
    let root = root();
    let long_relay = format!(
        "https://example.com/{}",
        "a".repeat(512 - "https://example.com/".len() + 1)
    );
    let longest_relay = format!(
        "https://example.com/{}",
        "a".repeat(512 - "https://example.com/".len())
    );
    write_export(
        root.path(),
        &json!({
            "version": 1,
            "hosts": [
                host(1, json!({ "name": "" })),
                host(2, json!({ "name": "   " })),
                host(3, json!({ "name": "n".repeat(81) })),
                host(4, json!({ "name": "n".repeat(80) })),
                host(5, json!({ "name": "  Padded  " })),
                host(6, json!({ "relay": long_relay })),
                host(7, json!({ "relay": longest_relay })),
            ],
        }),
    );
    let settings = load_or_import(root.path());
    let kept: Vec<&str> = settings
        .hosts
        .iter()
        .map(|host| host.name.as_str())
        .collect();
    assert_eq!(kept, ["n".repeat(80).as_str(), "Padded", "Synthetic PC 7"]);
}

#[test]
fn keeps_at_most_16_exported_hosts() {
    let root = root();
    let many: Vec<Value> = (1..=17).map(|n| host(n, json!({}))).collect();
    write_export(root.path(), &json!({ "version": 1, "hosts": many }));
    let settings = load_or_import(root.path());
    assert_eq!(settings.hosts.len(), MAX_HOSTS);
    assert_eq!(names(&settings.hosts).first(), Some(&"Synthetic PC 1"));
}

#[test]
fn atomic_write_survives_partial() {
    let root = root();
    let mut settings = DeviceSettings {
        theme: Theme::Light,
        ..DeviceSettings::default()
    };
    save(root.path(), &settings).unwrap();
    let before = std::fs::read_to_string(settings_file(root.path())).unwrap();

    // A crash in the middle of an earlier write left a partial temporary file beside the real one.
    let stale = settings_file(root.path()).with_extension("json.99999.0.tmp");
    write(stale.clone(), "{ \"version\": 1, \"theme\": \"da");
    assert_eq!(
        load_or_import(root.path()).theme,
        Theme::Light,
        "the partial file is ignored"
    );
    assert_eq!(
        std::fs::read_to_string(settings_file(root.path())).unwrap(),
        before
    );

    // A write that fails at the last step (a folder where the file belongs) leaves no temporary file and reports it.
    let blocked = root.path().join("blocked");
    std::fs::create_dir_all(settings_file(&blocked)).unwrap();
    assert!(save(&blocked, &settings).is_err());
    let leftovers: Vec<_> = std::fs::read_dir(blocked.join("data"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        leftovers,
        ["desktop-ui.json"],
        "only the blocking folder remains"
    );

    // And a later good save replaces the file completely.
    settings.density = Density::Compact;
    save(root.path(), &settings).unwrap();
    assert_eq!(load_or_import(root.path()), settings);
}

#[test]
fn reads_electron_fixture() {
    // The bytes the Electron 0.5.x exporter (packages/desktop/src/desktop-ui-export.ts, T1.6) writes for a filled-in page.
    // Regenerate with `npx vitest run packages/desktop/src/desktop-ui-export.test.ts` when the exporter changes.
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("desktop-ui-export.json");
    let root = root();
    write_export(
        root.path(),
        &serde_json::from_str(&std::fs::read_to_string(fixture).unwrap()).unwrap(),
    );
    let settings = load_or_import(root.path());
    assert_eq!(
        (settings.theme, settings.text_size, settings.density),
        (Theme::Light, TextSize::Larger, Density::Compact)
    );
    assert_eq!(names(&settings.hosts), ["Desk PC", "Laptop"]);
    assert_eq!(names(&settings.pending), ["New PC"]);
    let desk = settings.hosts.first().unwrap();
    assert_eq!(
        desk.device_id.as_deref(),
        Some("8d1f4c1e-2b7a-4e0b-b1d3-0a6c5e9f7a22")
    );
    assert_eq!(
        desk.relay.as_ref().map(|relay| relay.as_str()),
        Some("https://relay.example.workers.dev")
    );
    assert_eq!(desk.lan.len(), 2, "an IPv4 and an IPv6 address");
    // Nothing was dropped: the log has no complaint.
    assert!(
        logged_for(root.path())
            .iter()
            .all(|line| !line.starts_with("WARN")),
        "{:?}",
        logged_for(root.path())
    );
}

#[test]
fn starts_from_the_defaults_when_there_is_nothing_to_import() {
    let root = root();
    let settings = load_or_import(root.path());
    assert_eq!(settings, DeviceSettings::default());
    assert_eq!(
        (
            settings.theme,
            settings.text_size,
            settings.density,
            settings.zoom_percent
        ),
        (Theme::Dark, TextSize::System, Density::Comfortable, 100)
    );
    assert!(settings_file(root.path()).exists());
    assert!(
        logged_for(root.path())
            .iter()
            .any(|line| line.contains("no export"))
    );
}

#[test]
fn saved_settings_round_trip() {
    let root = root();
    let mut settings = DeviceSettings {
        theme: Theme::System,
        text_size: TextSize::Large,
        zoom_percent: 125,
        tray_hint_shown: true,
        ..DeviceSettings::default()
    };
    write_export(root.path(), &full_export());
    let imported = load_or_import(root.path());
    settings.hosts = imported.hosts;
    settings.pending = imported.pending;
    save(root.path(), &settings).unwrap();
    assert_eq!(load_or_import(root.path()), settings);
}

/// The files in the data folder whose names start with `desktop-ui.json.bad-`: copies of unreadable settings.
fn copies(root: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(root.join("data"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("desktop-ui.json.bad-"))
        })
        .collect();
    found.sort();
    found
}

#[test]
fn an_unreadable_settings_file_is_left_as_it_is_and_the_defaults_are_used_for_this_run() {
    for (name, text) in [
        ("not json", "{ nope".to_owned()),
        ("another version", json!({ "version": 9 }).to_string()),
        ("a zoom out of range", json!({ "version": 1, "zoomPercent": 5000 }).to_string()),
        ("a bad host", json!({ "version": 1, "hosts": [host(1, json!({ "hostKey": "x" }))] }).to_string()),
        ("too many hosts", json!({ "version": 1, "pending": (1..=17).map(|n| host(n, json!({}))).collect::<Vec<_>>() }).to_string()),
    ] {
        let root = root();
        write(settings_file(root.path()), &text);
        assert_eq!(load_or_import(root.path()), DeviceSettings::default(), "{name}");
        // The paired PCs in a file this build can't read must survive a downgrade, a partial write or a newer build.
        assert_eq!(std::fs::read_to_string(settings_file(root.path())).unwrap(), text, "{name}: the file is untouched");
        let kept = copies(root.path());
        assert_eq!(kept.len(), 1, "{name}: one copy is kept for recovery");
        assert_eq!(std::fs::read_to_string(&kept[0]).unwrap(), text, "{name}");
        assert!(logged_for(root.path()).iter().any(|line| line.starts_with("WARN") && line.contains("as it is")), "{name}");
    }
}

#[test]
fn a_settings_file_with_more_fields_than_this_build_knows_still_reads() {
    // A newer build wrote a field this one has never heard of; going back to this build must not lose the paired PCs.
    let root = root();
    write(
        settings_file(root.path()),
        &json!({ "version": 1, "theme": "light", "colour": "teal", "hosts": [host(1, json!({}))] })
            .to_string(),
    );
    let settings = load_or_import(root.path());
    assert_eq!(settings.theme, Theme::Light);
    assert_eq!(names(&settings.hosts), ["Synthetic PC 1"]);
    assert!(copies(root.path()).is_empty(), "nothing was unreadable");
}

#[test]
fn starting_again_does_not_pile_up_copies_of_the_same_unreadable_file() {
    let root = root();
    write(settings_file(root.path()), "{ nope");
    load_or_import(root.path());
    // A copy is named by the second it was made in, so wait past it: without a check for the same content, this would add one.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    load_or_import(root.path());
    assert_eq!(copies(root.path()).len(), 1);
    // Different damage is a different file and is kept too.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    write(settings_file(root.path()), "{ still nope");
    load_or_import(root.path());
    assert_eq!(copies(root.path()).len(), 2);
}
