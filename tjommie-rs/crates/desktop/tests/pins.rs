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
