//! Spec §3.10: a dev or e2e binary has no way to look for an update, and the production feed's address isn't even in it
//! (CI runs the same scan on the finished `tjommie-desktop.exe`; this one covers what `desktop-host` itself compiles).
//! This test's own executable links the library, so it holds whatever the library compiled in. It joins the address
//! it looks for from its parts at run time, so the test doesn't put the address into its own executable. The needle is
//! the feed's path, not the project's address: About links to the project in every build.

use std::hint::black_box;

/// The feed's path, as one string only while the test runs.
fn address() -> String {
    black_box(["tsnyders", "tjommie", "releases", "latest", "download"]).join("/")
}

/// The project's address, also joined at run time.
fn project() -> String {
    black_box(["github.com", "tsnyders", "tjommie"]).join("/")
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[test]
fn only_a_production_build_contains_the_release_feed_address() {
    // Keeps the library's identity and updater code linked into this executable.
    let identity = black_box(desktop_host::identity::CURRENT);
    let _ = black_box(desktop_host::updater::resolve_feed(
        &identity,
        &desktop_host::updater::UpdateEnv::default(),
    ));

    let needle = address();
    let binary = std::fs::read(std::env::current_exe().unwrap()).unwrap();
    let present = contains(&binary, needle.as_bytes());
    if cfg!(tjommie_identity = "production") {
        assert!(present, "a production build must carry its feed address");
    } else {
        assert!(
            !present,
            "a non-production build must not contain the release feed address"
        );
    }
    // And the identity agrees with what is compiled in.
    assert_eq!(
        identity.update_feed.is_some(),
        cfg!(tjommie_identity = "production")
    );
}

#[test]
fn the_scan_finds_the_feed_and_not_the_project_links() {
    // A mutation check for the scan itself: the feed's address must be found, and About's links (the releases page,
    // the project, its license), which every build carries, must not.
    let needle = address();
    let project = project();
    let feed = format!("....https://{project}/releases/latest/download/latest.yml....");
    assert!(contains(feed.as_bytes(), needle.as_bytes()));
    for link in [
        format!("https://{project}/releases/latest"),
        format!("https://{project}"),
        format!("https://{project}/blob/main/LICENSE"),
    ] {
        assert!(
            !contains(link.as_bytes(), needle.as_bytes()),
            "{link} isn't the feed"
        );
    }
    assert!(!contains(b"github.com/someone/else", needle.as_bytes()));
}
