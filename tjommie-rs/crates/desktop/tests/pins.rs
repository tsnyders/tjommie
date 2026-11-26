//! The GPUI pins (plan §"Pins and upgrades"): `gpui-pre` and `gpui-kit` move only together, in one reviewed PR.

const LOCK: &str = include_str!("../../../Cargo.lock");

/// The trimmed lines of every `[[package]]` block in Cargo.lock named `name`.
fn packages(name: &str) -> Vec<Vec<&'static str>> {
    let name_line = format!("name = \"{name}\"");
    LOCK.split("[[package]]")
        .map(|block| block.lines().map(str::trim).collect::<Vec<_>>())
        .filter(|lines| lines.contains(&name_line.as_str()))
        .collect()
}

#[test]
fn gpui_pre_is_pinned() {
    let found = packages("gpui-pre");
    assert_eq!(
        found.len(),
        1,
        "exactly one gpui-pre in Cargo.lock: {found:?}"
    );
    assert!(found[0].contains(&"version = \"0.3.12\""), "{:?}", found[0]);
}

#[test]
fn gpui_kit_is_pinned() {
    let found = packages("gpui-kit");
    assert_eq!(
        found.len(),
        1,
        "exactly one gpui-kit in Cargo.lock: {found:?}"
    );
    let source = found[0].iter().find(|line| line.starts_with("source = "));
    assert!(
        source.is_some_and(|line| line.contains("rev=39ab849a")),
        "{:?}",
        found[0]
    );
}

/// The test driver's pipe is in the app only when built with `test-driver` (Phase 3 plan T0.5): the binary this test
/// run built has the pipe name exactly when the feature is on. CI also checks the release build (rust-dev.yml).
#[test]
fn test_driver_only_with_its_feature() {
    let exe = std::fs::read(env!("CARGO_BIN_EXE_tjommie-desktop")).unwrap();
    let needle = b"tjommie-ui-";
    let has_pipe = exe.windows(needle.len()).any(|bytes| bytes == needle);
    assert_eq!(has_pipe, cfg!(feature = "test-driver"));
}

/// The test driver's answer to a path prompt (`AnswerPathPrompt`, M5 review M8) stands in for the system's picker only
/// in a test-driver build: its tag is in the binary exactly when the feature is on. CI checks the release build too.
#[test]
fn path_prompt_answers_only_with_the_test_driver() {
    let exe = std::fs::read(env!("CARGO_BIN_EXE_tjommie-desktop")).unwrap();
    let needle = b"tjommie-test-path-answer";
    let has_hook = exe.windows(needle.len()).any(|bytes| bytes == needle);
    assert_eq!(has_hook, cfg!(feature = "test-driver"));
}

/// The motion checks' switches are in the app only with the test driver (M7 review m6): a production binary has no
/// `--trace` to write a file wherever it is told, nor `--force-reduced` to override Windows' motion setting.
#[test]
fn motion_switches_only_with_the_test_driver() {
    let exe = std::fs::read(env!("CARGO_BIN_EXE_tjommie-desktop")).unwrap();
    for needle in [&b"--trace"[..], &b"--force-reduced"[..]] {
        let has = exe.windows(needle.len()).any(|bytes| bytes == needle);
        assert_eq!(
            has,
            cfg!(feature = "test-driver"),
            "{}",
            String::from_utf8_lossy(needle)
        );
    }
}
