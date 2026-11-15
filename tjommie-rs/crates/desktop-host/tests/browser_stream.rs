//! T3.4: a bot's live view. A remote controller asks for frames (`stream`) or one picture (`frame`); the engine runs a
//! screencast on the bot's page while anyone watches, throttles and validates its frames, delivers the newest, and stops
//! when a private interaction begins, the browser closes, or nobody is listening. The cases are those of
//! `native-browser-stream.test.ts`, run against the real WebView2 runtime and a stand-in for the daemon (`Rig::sink`).

// Test helpers may panic: a failure here is the test failing (the deny lints guard shipped code).
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::time::{Duration, Instant};

use base64::Engine as _;
use common::Rig;
use desktop_host::browser::engine::Reply;
use desktop_host::browser::stream::jpeg_size;
use protocol::browser::{BrowserControl, BrowserFrame};
use protocol::native_browser::StreamEnd;
use serde_json::json;

const PHONE: &str = "rpc:phone";

fn watching(rig: &mut Rig) {
    rig.open("alpha");
    rig.load("alpha", "stream.html");
}

fn stream_on(rig: &mut Rig, controller: &str, max_fps: u32) {
    rig.remote(
        "alpha",
        controller,
        json!({ "kind": "stream", "on": true, "maxFps": max_fps }),
    )
    .expect("a stream to start");
}

fn stream_off(rig: &mut Rig, controller: &str) {
    rig.remote(
        "alpha",
        controller,
        json!({ "kind": "stream", "on": false }),
    )
    .expect("a stream to stop");
}

/// The frames received so far, oldest first.
fn frames(rig: &Rig) -> Vec<(Instant, BrowserFrame)> {
    rig.sink.borrow().frames.clone()
}

fn jpeg_of(frame: &BrowserFrame) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(&frame.data)
        .expect("a frame is standard base64")
}

#[test]
fn a_stream_forwards_valid_ordered_frames_that_carry_their_ticket() {
    let mut rig = Rig::new();
    watching(&mut rig);
    let state = rig.state("alpha");
    stream_on(&mut rig, PHONE, 60);
    rig.wait_frames(8, Duration::from_secs(20));
    let seen = frames(&rig);
    let mut last = 0;
    for (_, frame) in &seen {
        let (width, height) = jpeg_size(&jpeg_of(frame)).expect("a JPEG with a size");
        assert_eq!(
            (frame.width, frame.height),
            (width, height),
            "the size is the JPEG's own"
        );
        assert!(width >= 100 && height >= 100 && width <= 4096 && height <= 4096);
        assert!(
            frame.viewport_width.is_some_and(|w| w >= 1)
                && frame.viewport_height.is_some_and(|h| h >= 1)
        );
        assert_eq!(frame.page_id, state.active_page_id);
        assert_eq!(frame.control_epoch, state.control_epoch);
        assert_eq!(frame.private_epoch, state.private_epoch);
        assert_eq!(
            frame.stream_id,
            Some(format!(
                "{}:{}:{}",
                state.generation,
                state.active_page_id.clone().expect("a page"),
                state.private_epoch.expect("an epoch")
            ))
        );
        assert!(
            frame.sequence > last,
            "sequences only rise: {} after {last}",
            frame.sequence
        );
        last = frame.sequence;
    }
    // Chromium sent frames for as long as they were acknowledged: the page kept painting and we kept receiving.
    let before = rig.frame_count();
    rig.wait_frames(before + 8, Duration::from_secs(20));
}

#[test]
fn turning_the_stream_off_stops_the_frames_and_lets_the_page_sleep() {
    let mut rig = Rig::new();
    watching(&mut rig);
    stream_on(&mut rig, PHONE, 60);
    rig.wait_frames(3, Duration::from_secs(20));
    assert!(
        rig.engine.snapshot("alpha").is_some_and(|s| !s.suspended),
        "a watched page is awake"
    );
    stream_off(&mut rig, PHONE);
    rig.ui.settle(Duration::from_millis(500));
    let stopped_at = rig.frame_count();
    rig.ui.settle(Duration::from_millis(700));
    assert_eq!(
        rig.frame_count(),
        stopped_at,
        "no frame after the stream is off"
    );
    assert!(
        rig.sink.borrow().ends.is_empty(),
        "the controller turned it off itself: nobody is told"
    );
    rig.wait_suspended("alpha");
}

#[test]
fn max_fps_caps_how_many_frames_are_forwarded() {
    let mut rig = Rig::new();
    watching(&mut rig);
    stream_on(&mut rig, PHONE, 10);
    rig.wait_frames(2, Duration::from_secs(20));
    let first = rig.frame_count();
    rig.ui.settle(Duration::from_secs(2));
    let slow = rig.frame_count() - first;
    assert!(
        slow <= 24,
        "10 fps allows about 20 frames in two seconds, got {slow}"
    );
    // Another controller wants more: the stream runs at the highest rate asked.
    stream_on(&mut rig, "rpc:tablet", 60);
    let before = rig.frame_count();
    rig.ui.settle(Duration::from_secs(2));
    let fast = rig.frame_count() - before;
    assert!(
        fast > slow + 5,
        "60 fps is clearly faster than 10 fps: {fast} against {slow}"
    );
}

#[test]
fn a_slow_delivery_gets_only_the_newest_frames_never_a_backlog() {
    let mut rig = Rig::new();
    watching(&mut rig);
    rig.sink.borrow_mut().delay = Duration::from_millis(300);
    let painted_before: u64 = rig.eval("alpha", "window.ticks").parse().expect("a count");
    stream_on(&mut rig, PHONE, 60);
    rig.ui.settle(Duration::from_secs(3));
    let seen = frames(&rig);
    let painted: u64 = rig
        .eval("alpha", "window.ticks")
        .parse::<u64>()
        .expect("a count")
        - painted_before;
    assert!(seen.len() >= 4, "deliveries went on: {}", seen.len());
    assert!(
        seen.len() <= 14,
        "about one delivery per 300 ms, not one per frame: {}",
        seen.len()
    );
    // The page painted far more often than frames were delivered: the ones in between were dropped, never queued.
    assert!(
        painted > seen.len() as u64 * 5,
        "{painted} paints against {} deliveries",
        seen.len()
    );
    // And nothing was queued behind the slow deliveries: with them quick again, no burst of old frames follows.
    rig.sink.borrow_mut().delay = Duration::ZERO;
    rig.ui.settle(Duration::from_millis(400));
    let at_speed = rig.frame_count();
    assert!(
        at_speed - seen.len() <= 40,
        "no backlog flushed: {} more frames in 0.4 s",
        at_speed - seen.len()
    );
    let sequences: Vec<u64> = seen.iter().map(|(_, f)| f.sequence).collect();
    assert!(sequences.windows(2).all(|pair| pair[1] > pair[0]));
}

#[test]
fn a_daemon_with_nobody_watching_stops_the_stream_without_a_goodbye() {
    let mut rig = Rig::new();
    watching(&mut rig);
    rig.sink.borrow_mut().refuse = true;
    stream_on(&mut rig, PHONE, 60);
    rig.wait_frames(1, Duration::from_secs(20));
    rig.ui.settle(Duration::from_millis(800));
    let seen = rig.frame_count();
    assert!(
        seen <= 3,
        "the first refused frame ended the stream: {seen}"
    );
    rig.ui.settle(Duration::from_millis(700));
    assert_eq!(rig.frame_count(), seen, "and nothing more is sent");
    assert!(
        rig.sink.borrow().ends.is_empty(),
        "nobody is listening, so nobody is told"
    );
}

#[test]
fn private_mode_ends_the_stream_and_no_frame_follows() {
    let mut rig = Rig::new();
    watching(&mut rig);
    // The person holds the browser; a phone watches.
    let held = rig.exec_state("alpha", json!({ "kind": "control", "control": "human" }));
    assert_eq!(held.control, BrowserControl::Human);
    stream_on(&mut rig, PHONE, 60);
    rig.wait_frames(4, Duration::from_secs(20));

    let (engine, bot) = (rig.engine.clone(), rig.bot("alpha"));
    let epoch = held.control_epoch.expect("an epoch");
    let private = rig
        .ui
        .run(async move { engine.begin_private(&bot, "local", epoch).await })
        .expect("private mode to begin");
    assert!(private > held.private_epoch.expect("an epoch"));
    rig.ui.settle(Duration::from_millis(600));
    assert_eq!(
        rig.sink.borrow().ends,
        vec![StreamEnd::Private],
        "the watcher is told once, and why"
    );
    let at_end = rig.frame_count();
    rig.ui.settle(Duration::from_millis(800));
    assert_eq!(rig.frame_count(), at_end, "private_forwards_zero_frames");

    // While it lasts there is nothing to show, to ask for or to watch.
    match rig
        .remote("alpha", PHONE, json!({ "kind": "frame" }))
        .expect("a frame answer")
    {
        Reply::Frame(result) => {
            assert!(
                result.frame.is_none(),
                "no picture during a private interaction"
            );
            assert!(result.state.private_active == Some(true));
        }
        other => panic!("expected a frame answer, got {other:?}"),
    }
    rig.remote(
        "alpha",
        PHONE,
        json!({ "kind": "stream", "on": true, "maxFps": 60 }),
    )
    .expect("the request is accepted");
    rig.ui.settle(Duration::from_millis(600));
    assert_eq!(
        rig.frame_count(),
        at_end,
        "no controller is registered while private"
    );

    // After it, the watcher must ask again: the stream does not resume on its own.
    let (engine, bot) = (rig.engine.clone(), rig.bot("alpha"));
    rig.ui
        .run(async move {
            engine
                .end_private(&bot, "local", private, async { Ok(()) })
                .await
        })
        .expect("private mode to end");
    rig.ui.settle(Duration::from_millis(800));
    assert_eq!(
        rig.frame_count(),
        at_end,
        "still nothing until it is asked for"
    );
    stream_on(&mut rig, PHONE, 60);
    rig.wait_frames(at_end + 3, Duration::from_secs(20));
    // The frames that follow carry the epoch the browser is in now, so a watcher can tell them from the earlier stream's.
    let now = rig.state("alpha").private_epoch;
    assert!(now > held.private_epoch);
    let after = frames(&rig);
    assert!(
        after
            .iter()
            .skip(at_end)
            .all(|(_, f)| f.private_epoch == now)
    );
}

#[test]
fn closing_the_browser_ends_the_stream_as_closed() {
    let mut rig = Rig::new();
    watching(&mut rig);
    stream_on(&mut rig, PHONE, 60);
    rig.wait_frames(3, Duration::from_secs(20));
    rig.exec("alpha", json!({ "kind": "close" }))
        .expect("close");
    rig.ui.settle(Duration::from_millis(500));
    assert_eq!(rig.sink.borrow().ends, vec![StreamEnd::Closed]);
}

#[test]
fn a_controller_that_disconnects_takes_its_stream_with_it() {
    let mut rig = Rig::new();
    watching(&mut rig);
    stream_on(&mut rig, PHONE, 60);
    rig.wait_frames(3, Duration::from_secs(20));
    rig.remote("alpha", PHONE, json!({ "kind": "disconnect" }))
        .expect("disconnect");
    rig.ui.settle(Duration::from_millis(500));
    let stopped_at = rig.frame_count();
    rig.ui.settle(Duration::from_millis(700));
    assert_eq!(rig.frame_count(), stopped_at, "nobody is watching");
}

#[test]
fn a_daemon_restart_ends_the_remote_streams_but_not_the_page() {
    let mut rig = Rig::new();
    watching(&mut rig);
    stream_on(&mut rig, PHONE, 60);
    rig.wait_frames(3, Duration::from_secs(20));
    // The old daemon's connections are gone with it, and so are the streams that ran on them.
    let engine = rig.engine.clone();
    rig.ui.run(async move { engine.daemon_restarted().await });
    rig.ui.settle(Duration::from_millis(600));
    let stopped_at = rig.frame_count();
    rig.ui.settle(Duration::from_millis(700));
    assert_eq!(
        rig.frame_count(),
        stopped_at,
        "no frame goes to a daemon that is not there"
    );
    assert_eq!(
        rig.eval("alpha", "document.title"),
        "Stream fixture",
        "the page itself lives on"
    );
    // A phone of the new daemon asks again, and gets frames.
    stream_on(&mut rig, "rpc:new-phone", 60);
    rig.wait_frames(stopped_at + 3, Duration::from_secs(20));
}

#[test]
fn the_stream_follows_the_active_tab() {
    let mut rig = Rig::new();
    watching(&mut rig);
    stream_on(&mut rig, PHONE, 60);
    rig.wait_frames(3, Duration::from_secs(20));
    let first_page = frames(&rig)
        .last()
        .and_then(|(_, f)| f.page_id.clone())
        .expect("a page");
    // The person (who holds the browser) opens another tab; the stream, which belongs to the phone, follows it.
    rig.exec_state("alpha", json!({ "kind": "control", "control": "human" }));
    let state = rig.state("alpha");
    let moved = rig.exec_state(
        "alpha",
        json!({ "kind": "tab", "generation": state.generation, "action": "new" }),
    );
    assert_ne!(moved.active_page_id.as_deref(), Some(first_page.as_str()));
    // A screencast sends a frame when the page changes: the new tab animates, as the first did.
    rig.load("alpha", "stream.html");
    rig.ui.settle(Duration::from_millis(800));
    let before = rig.frame_count();
    rig.wait_frames(before + 3, Duration::from_secs(20));
    let newest = frames(&rig).last().and_then(|(_, f)| f.page_id.clone());
    assert_eq!(
        newest, moved.active_page_id,
        "frames now come from the new tab"
    );
}

#[test]
fn a_frame_request_answers_with_a_picture_and_a_thumbnail_is_small() {
    let mut rig = Rig::new();
    watching(&mut rig);
    let full = match rig
        .remote("alpha", PHONE, json!({ "kind": "frame" }))
        .expect("a frame")
    {
        Reply::Frame(result) => *result,
        other => panic!("expected a frame answer, got {other:?}"),
    };
    let picture = full.frame.expect("a picture");
    let (width, height) = jpeg_size(&jpeg_of(&picture)).expect("a JPEG");
    assert_eq!((picture.width, picture.height), (width, height));
    assert!(
        picture.viewport_width.is_some_and(|w| w >= 1)
            && picture.viewport_height.is_some_and(|h| h >= 1)
    );
    assert_eq!(picture.page_id, full.state.active_page_id);
    assert!(picture.sequence >= 1);

    let small = match rig
        .remote(
            "alpha",
            PHONE,
            json!({ "kind": "frame", "thumbnail": true }),
        )
        .expect("a thumbnail")
    {
        Reply::Frame(result) => result.frame.expect("a picture"),
        other => panic!("expected a frame answer, got {other:?}"),
    };
    let bytes = jpeg_of(&small);
    let (w, h) = jpeg_size(&bytes).expect("a JPEG");
    assert!(w <= 320 && h <= 240, "a thumbnail fits 320 by 240: {w}x{h}");
    assert!(bytes.len() <= 64 * 1024, "and 64 KiB: {}", bytes.len());
    assert!(
        small.sequence > picture.sequence,
        "each picture has its own sequence number"
    );
    // The page has no panel and no tool: it was woken for the picture and goes back to sleep.
    rig.wait_suspended("alpha");
}

#[test]
fn a_hidden_window_keeps_streaming() {
    use windows::Win32::UI::WindowsAndMessaging::{SW_MINIMIZE, SW_RESTORE, ShowWindow};
    let mut rig = Rig::new();
    watching(&mut rig);
    rig.show("alpha");
    stream_on(&mut rig, PHONE, 60);
    rig.wait_frames(4, Duration::from_secs(20));
    // The panel closes and the window is minimized (review focus 3): nothing is visible, yet someone is still watching.
    rig.hide("alpha");
    rig.engine.set_window_visible(false);
    // SAFETY: a plain call on the test's own parent window, from its own thread.
    let _ = unsafe { ShowWindow(rig.ui.parent, SW_MINIMIZE) };
    rig.ui.settle(Duration::from_millis(500));
    let before = rig.frame_count();
    rig.wait_frames(before + 10, Duration::from_secs(20));
    // SAFETY: as above.
    let _ = unsafe { ShowWindow(rig.ui.parent, SW_RESTORE) };
    rig.engine.set_window_visible(true);
}

/// The LAN target of the plan (T3.4): 1280 by 800 for a minute, a median of at least 55 frames a second and 95% of the gaps
/// at most 34 ms, on loopback. It measures this PC, so it runs only when asked: `TJOMMIE_STREAM_BENCH=1` (the length in
/// seconds may be set with `TJOMMIE_STREAM_SECONDS`, 60 by default), on a machine with nothing else busy.
#[test]
#[ignore = "measures this PC: set TJOMMIE_STREAM_BENCH=1 and run with --ignored on a quiet machine"]
fn meets_lan_target() {
    if std::env::var_os("TJOMMIE_STREAM_BENCH").is_none() {
        eprintln!("meets_lan_target: TJOMMIE_STREAM_BENCH is not set, so nothing was measured");
        return;
    }
    let seconds: u64 = std::env::var("TJOMMIE_STREAM_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(60);
    let mut rig = Rig::new();
    watching(&mut rig);
    rig.engine.set_window_visible(true);
    stream_on(&mut rig, PHONE, 60);
    rig.wait_frames(30, Duration::from_secs(20));
    let started = Instant::now();
    let first = rig.frame_count();
    rig.ui.settle(Duration::from_secs(seconds));
    let seen: Vec<Instant> = frames(&rig)
        .iter()
        .skip(first)
        .map(|(at, _)| *at)
        .filter(|at| *at >= started)
        .collect();
    let mut per_second = vec![0u32; usize::try_from(seconds).unwrap_or(60)];
    for at in &seen {
        let bucket = usize::try_from(at.duration_since(started).as_secs()).unwrap_or(usize::MAX);
        if let Some(count) = per_second.get_mut(bucket) {
            *count += 1;
        }
    }
    per_second.sort_unstable();
    let median = per_second.get(per_second.len() / 2).copied().unwrap_or(0);
    let mut gaps: Vec<f64> = seen
        .windows(2)
        .map(|pair| pair[1].duration_since(pair[0]).as_secs_f64() * 1000.0)
        .collect();
    gaps.sort_by(f64::total_cmp);
    let p95 = gaps.get(gaps.len() * 95 / 100).copied().unwrap_or(f64::MAX);
    eprintln!(
        "stream: {} frames in {seconds} s, median {median} fps, p95 gap {p95:.1} ms",
        seen.len()
    );
    assert!(median >= 55, "the median second has {median} frames");
    assert!(p95 <= 34.0, "the p95 gap is {p95:.1} ms");
}
