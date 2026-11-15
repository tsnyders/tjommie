//! The `provider-fixtures` feature must never reach a shipped daemon, so lib.rs refuses to compile it for the production
//! identity (`TJOMMIE_IDENTITY=production`, which build.rs turns into `cfg(tjommie_identity = "production")`) and for
//! any build without debug assertions (what `--release` is). These tests run the real compiler on the real crate, with
//! the feature on, in the same target directory as this test, so only the daemon crate is rebuilt, and assert that it
//! stops with the guard's own message.
//!
//! The first test sets the real environment variable, so it makes cargo run build.rs again: the next build of the
//! daemon with this feature recompiles it once (about half a minute), and nothing else is affected. The second uses
//! `cargo rustc` with extra compiler flags, which cargo keeps apart from the normal build. A real `--release` build
//! would compile every dependency again for minutes, so it stands in as `-C debug-assertions=off`, which is exactly the
//! cfg lib.rs tests.
#![cfg(feature = "provider-fixtures")]

use std::process::Output;
use std::time::Instant;

/// `cargo <args>` in this crate, with `TJOMMIE_IDENTITY` as given and no other build identity leaking in.
fn cargo(args: &[&str], identity: Option<&str>) -> (Output, String) {
    let program = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = daemon::spawn::command(program);
    command
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(args)
        .env_remove("TJOMMIE_IDENTITY")
        // Plain output: no colour codes inside the message being matched.
        .env("CARGO_TERM_COLOR", "never");
    if let Some(identity) = identity {
        command.env("TJOMMIE_IDENTITY", identity);
    }
    let started = Instant::now();
    let output = command.output().expect("cargo runs");
    eprintln!(
        "cargo {}: {:.1}s",
        args.join(" "),
        started.elapsed().as_secs_f32()
    );
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    (output, stderr)
}

#[test]
fn the_production_identity_cannot_be_built_with_the_feature() {
    let (output, stderr) = cargo(
        &[
            "build",
            "-p",
            "daemon",
            "--lib",
            "--features",
            "provider-fixtures",
            "--locked",
        ],
        Some("production"),
    );
    assert!(!output.status.success(), "{stderr}");
    assert!(
        stderr.contains(
            "must never be built into the production daemon (TJOMMIE_IDENTITY=production)"
        ),
        "{stderr}"
    );
    assert!(
        stderr.contains("could not compile `daemon` (lib)"),
        "{stderr}"
    );
}

#[test]
fn a_build_without_debug_assertions_cannot_have_the_feature() {
    let (output, stderr) = cargo(
        &[
            "rustc",
            "-p",
            "daemon",
            "--lib",
            "--features",
            "provider-fixtures",
            "--locked",
            "--",
            "-C",
            "debug-assertions=off",
        ],
        None,
    );
    assert!(!output.status.success(), "{stderr}");
    assert!(
        stderr.contains("must not be built without debug assertions (a --release build)"),
        "{stderr}"
    );
    assert!(
        stderr.contains("could not compile `daemon` (lib)"),
        "{stderr}"
    );
}
