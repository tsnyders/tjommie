//! Holds the engine in one memory-budget state so `tools/perf/measure-memory.ps1` can measure the process tree (Phase 3
//! plan, "Memory budget"). Never part of a normal run: start it by hand, give the script its PID, kill it afterwards.
//!
//!   $env:TJOMMIE_HOLD_PHASE = "A"   # A: engine only, no bot ever opened (S0 without GPUI)
//!                                   # B: one bot's page shown in the panel (S1)
//!                                   # C: B, then the panel closed with nothing streaming: suspended (S3)
//!   desktop-host-measure_hold.exe --ignored --exact hold_one_state
//!
//! It writes `%TEMP%\tjommie-hold-<phase>.pid` once the state is reached, then idles for TJOMMIE_HOLD_SECONDS (default 240).
//! Everything lives in a temp root with fixtures on loopback; no production data is touched.

// A manual tool: it may panic.
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::time::Duration;

use common::Rig;

#[test]
#[ignore = "a manual measurement hold, not a test"]
fn hold_one_state() {
    let phase = std::env::var("TJOMMIE_HOLD_PHASE").unwrap_or_else(|_| "B".into());
    let seconds: u64 = std::env::var("TJOMMIE_HOLD_SECONDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(240);
    let mut rig = Rig::new();
    if phase != "A" {
        rig.open("alpha");
        rig.load("alpha", "controls.html");
        rig.show("alpha");
        if phase == "C" {
            rig.hide("alpha");
            rig.wait_suspended("alpha");
        }
    }
    let marker = std::env::temp_dir().join(format!("tjommie-hold-{phase}.pid"));
    std::fs::write(&marker, std::process::id().to_string()).expect("pid marker");
    rig.ui.settle(Duration::from_secs(seconds));
    let _ = std::fs::remove_file(marker);
}
