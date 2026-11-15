//! The provider-fixtures marker (`daemon::provider_fixtures::BUILD_MARKER`) is in a binary only when the feature is built
//! in, so the release workflow can search the shipped daemon for it. This runs both ways: with the feature, the test
//! binary and the daemon exe must carry it; without, neither may. The test spells the marker from parts so that it is
//! not in a feature-off test binary because of the test itself.

/// `TJOMMIE-PROVIDER-FIXTURES-BUILD-7f3a9c41`, assembled at run time.
fn marker() -> Vec<u8> {
    let parts = std::hint::black_box(["TJOMMIE-PROVIDER", "-FIXTURES-BUILD", "-7f3a9c41"]);
    parts.concat().into_bytes()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[test]
fn only_a_build_with_the_feature_carries_the_marker() {
    let wanted = cfg!(feature = "provider-fixtures");
    #[cfg(feature = "provider-fixtures")]
    {
        // The test binary links the feature's code (it can turn fixtures on) and so carries the marker, and the
        // spelled marker is the constant: the release check searches for the right text.
        assert_eq!(
            daemon::provider_fixtures::BUILD_MARKER.as_bytes(),
            marker().as_slice()
        );
        assert!(
            daemon::provider_fixtures::Fixtures::open(std::path::Path::new("relative")).is_err()
        );
    }

    let test_binary = std::fs::read(std::env::current_exe().unwrap()).unwrap();
    assert_eq!(
        contains(&test_binary, &marker()),
        wanted,
        "the test binary (feature on: {wanted})"
    );
    let daemon_exe = std::fs::read(env!("CARGO_BIN_EXE_tjommie-daemon")).unwrap();
    assert_eq!(
        contains(&daemon_exe, &marker()),
        wanted,
        "the daemon exe (feature on: {wanted})"
    );
}

#[test]
fn the_marker_search_can_tell_a_hit_from_a_miss() {
    // Guards the check above against a search that passes whatever it is given.
    let haystack = [b"abc".as_slice(), &marker(), b"xyz"].concat();
    assert!(contains(&haystack, &marker()));
    assert!(!contains(b"abcxyz", &marker()));
}
