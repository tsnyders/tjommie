//! L7: the engine's default way to wait (`EngineConfig::sleep`) costs no thread per wait. The settle loops of the tools
//! wait many times over, so a thread each would be hundreds of threads at the busiest moment.

// Test helpers may panic: a failure here is the test failing (the deny lints guard shipped code).
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::time::{Duration, Instant};

use desktop_host::browser::engine::EngineConfig;
use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows::Win32::System::Threading::GetCurrentProcessId;

/// How many threads this process has right now.
fn threads() -> usize {
    // SAFETY: plain ToolHelp calls; the snapshot handle is closed before returning.
    unsafe {
        let pid = GetCurrentProcessId();
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0).expect("a thread snapshot");
        let mut entry = THREADENTRY32 {
            dwSize: std::mem::size_of::<THREADENTRY32>() as u32,
            ..Default::default()
        };
        let mut count = 0;
        let mut more = Thread32First(snapshot, &mut entry).is_ok();
        while more {
            if entry.th32OwnerProcessID == pid {
                count += 1;
            }
            more = Thread32Next(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
        count
    }
}

#[test]
fn the_default_wait_costs_no_thread_per_wait_and_still_waits() {
    let root = tempfile::tempdir().expect("a temp root");
    let config = EngineConfig::new(
        HWND::default(),
        root.path().join("webview2"),
        desktop_host::identity::E2E,
    );
    // The first wait may start the timer; after it, many more must not add threads.
    futures::executor::block_on((config.sleep)(Duration::from_millis(1)));
    let before = threads();
    let started = Instant::now();
    let waits: Vec<_> = (0..300)
        .map(|_| (config.sleep)(Duration::from_millis(800)))
        .collect();
    let during = threads();
    assert!(
        during < before + 30,
        "300 waits took the process from {before} threads to {during}"
    );
    futures::executor::block_on(futures::future::join_all(waits));
    assert!(
        started.elapsed() >= Duration::from_millis(790),
        "the waits ended early, after {:?}",
        started.elapsed()
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the waits ended late, after {:?}",
        started.elapsed()
    );
    // A shorter wait that starts later still ends first, and a zero wait ends at once.
    let (long, short) = (
        (config.sleep)(Duration::from_millis(600)),
        (config.sleep)(Duration::from_millis(50)),
    );
    let order = futures::executor::block_on(futures::future::select(long, short));
    assert!(
        matches!(order, futures::future::Either::Right(_)),
        "the short wait ends first"
    );
    futures::executor::block_on((config.sleep)(Duration::ZERO));
}
