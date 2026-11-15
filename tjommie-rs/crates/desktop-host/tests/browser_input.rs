//! T3.5: the shield, focus and shortcuts between the page and the app, and the page's alignment with its panel.
//!
//! What a test here can do without a person: the shield's picture, Shift+Tab through the isolated world (the page gets the
//! key from the DevTools protocol, which is a trusted key event), focus going back to the app, realigning, and the
//! alignment rule at the scale this PC is on. What needs the owner's PC is gated and listed: real keyboard input
//! (`TJOMMIE_OWNER_INPUT=1`; it takes the foreground, so it never runs unasked) and the 100/125/150% sweep (it needs the
//! display scale changed, which no test does; `owner-checks-scale.ps1` of the spike does it and restores the scale).

// Test helpers may panic: a failure here is the test failing (the deny lints guard shipped code).
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::time::{Duration, Instant};

use common::Rig;
use desktop_host::browser::engine::{Notification, png_size};
use desktop_host::browser::keyboard::{Chord, FocusDirection};
use serde_json::json;
use windows::Win32::Foundation::RECT;
use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos,
};

/// A bot's page in the panel, held by the person (so its keyboard is the person's), with `page` loaded.
fn held_in_panel(rig: &mut Rig, page: &str) {
    rig.open("alpha");
    rig.load("alpha", page);
    rig.show("alpha");
    rig.exec_state("alpha", json!({ "kind": "control", "control": "human" }));
    rig.ui.settle(Duration::from_millis(500));
}

/// A key event for the page, through the DevTools protocol.
fn key(rig: &mut Rig, event: &str) {
    let guard = rig.begin("alpha");
    let tab = guard.tab().expect("a tab");
    let params = event.to_string();
    rig.ui.run(async move {
        tab.call("Input.dispatchKeyEvent", &params)
            .await
            .expect("a key event");
    });
    guard.end();
}

fn focus_returns(rig: &Rig) -> Vec<FocusDirection> {
    rig.notifications()
        .into_iter()
        .filter_map(|item| match item {
            Notification::FocusReturn { direction, .. } => Some(direction),
            _ => None,
        })
        .collect()
}

#[test]
fn shield_snapshot_before_hide() {
    let mut rig = Rig::new();
    rig.open("alpha");
    rig.load("alpha", "stream.html");
    rig.show("alpha");
    rig.ui.settle(Duration::from_millis(800));
    assert!(
        rig.engine
            .snapshot("alpha")
            .is_some_and(|s| s.container_shown),
        "the page is on screen"
    );

    let (engine, started) = (rig.engine.clone(), Instant::now());
    let shield = rig
        .ui
        .run(async move { engine.raise_shield("alpha").await });
    eprintln!(
        "shield: {} bytes, {}x{}, {:?}",
        shield.png.len(),
        shield.width,
        shield.height,
        started.elapsed()
    );
    // The picture was taken while the page was visible: a PNG of the page's size, with the page's colours in it.
    assert_eq!(png_size(&shield.png), Some((shield.width, shield.height)));
    let panel = rig.panel().rect;
    assert!(
        shield.width >= (panel.right - panel.left) as u32 - 2,
        "as wide as the panel: {}",
        shield.width
    );
    assert!(
        shield.height >= (panel.bottom - panel.top) as u32 - 2,
        "and as tall: {}",
        shield.height
    );
    assert!(
        shield.png.len() > 3_000,
        "a page full of colour, not a blank one: {} bytes",
        shield.png.len()
    );
    // Then the page's window left, so the app can draw over its area.
    assert!(
        rig.engine
            .snapshot("alpha")
            .is_some_and(|s| !s.container_shown),
        "the page is hidden behind the shield"
    );
    // Hidden, but awake and rendering: lowering the shield shows the live page at once.
    rig.engine.lower_shield("alpha");
    assert!(
        rig.engine
            .snapshot("alpha")
            .is_some_and(|s| s.container_shown && !s.suspended)
    );
}

#[test]
fn shift_tab_leaves_page() {
    let mut rig = Rig::new();
    held_in_panel(&mut rig, "tools.html");
    rig.eval(
        "alpha",
        "document.getElementById('name').focus(); document.activeElement.id",
    );
    rig.notifications();
    // Shift+Tab, as the person types it: the page's own world never sees it, and focus goes to the app.
    key(
        &mut rig,
        r#"{"type":"rawKeyDown","key":"Tab","code":"Tab","windowsVirtualKeyCode":9,"modifiers":8}"#,
    );
    key(
        &mut rig,
        r#"{"type":"keyUp","key":"Tab","code":"Tab","windowsVirtualKeyCode":9,"modifiers":8}"#,
    );
    let mut seen = Vec::new();
    for _ in 0..30 {
        rig.ui.settle(Duration::from_millis(100));
        seen.extend(focus_returns(&rig));
        if !seen.is_empty() {
            break;
        }
    }
    assert_eq!(
        seen,
        vec![FocusDirection::Previous],
        "Shift+Tab leaves the page, backwards"
    );
    // SAFETY: a plain Win32 call on the test's own thread.
    assert_eq!(
        unsafe { GetFocus() },
        rig.ui.parent,
        "the app's window has the keyboard now"
    );
    assert_eq!(
        rig.eval("alpha", "document.activeElement.id"),
        "name",
        "and the page did not move its own focus"
    );
}

#[test]
fn a_page_cannot_hand_the_keyboard_off_with_a_synthetic_shift_tab() {
    let mut rig = Rig::new();
    held_in_panel(&mut rig, "tools.html");
    rig.eval("alpha", "document.getElementById('name').focus(); 1");
    rig.notifications();
    // The page's own script makes the key up: a DOM event crosses into the isolated world, but it is not a trusted one.
    rig.eval(
        "alpha",
        "document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Tab', shiftKey: true, bubbles: true, cancelable: true })); 1",
    );
    rig.ui.settle(Duration::from_millis(800));
    assert!(
        focus_returns(&rig).is_empty(),
        "only the person's own Shift+Tab hands the keyboard to the app"
    );
}

#[test]
fn shift_tab_still_leaves_the_page_after_a_private_interaction() {
    let mut rig = Rig::new();
    held_in_panel(&mut rig, "tools.html");
    // A private interaction switches the runtime domain off; ending it must give the page its hand-off back, or a
    // person typing in the next document could not leave it with the keyboard.
    let held = rig.state("alpha");
    let (engine, bot) = (rig.engine.clone(), rig.bot("alpha"));
    let epoch = held.control_epoch.expect("an epoch");
    let private = rig
        .ui
        .run(async move { engine.begin_private(&bot, "local", epoch).await })
        .expect("private mode to begin");
    let (engine, bot) = (rig.engine.clone(), rig.bot("alpha"));
    rig.ui
        .run(async move {
            engine
                .end_private(&bot, "local", private, async { Ok(()) })
                .await
        })
        .expect("private mode to end");
    rig.load("alpha", "tools.html");
    rig.eval("alpha", "document.getElementById('name').focus(); 1");
    rig.notifications();
    key(
        &mut rig,
        r#"{"type":"rawKeyDown","key":"Tab","code":"Tab","windowsVirtualKeyCode":9,"modifiers":8}"#,
    );
    let mut seen = Vec::new();
    for _ in 0..30 {
        rig.ui.settle(Duration::from_millis(100));
        seen.extend(focus_returns(&rig));
        if !seen.is_empty() {
            break;
        }
    }
    assert_eq!(seen, vec![FocusDirection::Previous]);
}

#[test]
fn tab_past_end_moves_focus() {
    let mut rig = Rig::new();
    held_in_panel(&mut rig, "tools.html");
    // The last control of the page has the focus; Tab has nowhere left to go inside it.
    rig.eval(
        "alpha",
        "document.getElementById('far').focus(); document.activeElement.id",
    );
    rig.notifications();
    key(
        &mut rig,
        r#"{"type":"rawKeyDown","key":"Tab","code":"Tab","windowsVirtualKeyCode":9}"#,
    );
    key(
        &mut rig,
        r#"{"type":"keyUp","key":"Tab","code":"Tab","windowsVirtualKeyCode":9}"#,
    );
    let mut seen = Vec::new();
    for _ in 0..30 {
        rig.ui.settle(Duration::from_millis(100));
        seen.extend(focus_returns(&rig));
        if !seen.is_empty() {
            break;
        }
    }
    assert_eq!(
        seen,
        vec![FocusDirection::Next],
        "Tab past the last control goes to the next control of the app"
    );
}

#[test]
fn a_phones_keystrokes_never_take_the_keyboard_from_the_app() {
    let mut rig = Rig::new();
    rig.open("alpha");
    rig.load("alpha", "tools.html");
    rig.show("alpha");
    rig.ui.settle(Duration::from_millis(500));
    // Nobody holds the browser here (the bot does): the page is not interactive, so its Shift+Tab is only the page's.
    rig.eval("alpha", "document.getElementById('name').focus(); 1");
    rig.notifications();
    key(
        &mut rig,
        r#"{"type":"rawKeyDown","key":"Tab","code":"Tab","windowsVirtualKeyCode":9,"modifiers":8}"#,
    );
    rig.ui.settle(Duration::from_millis(800));
    assert!(
        focus_returns(&rig).is_empty(),
        "no hand-off for input that is not the person's"
    );
}

#[test]
fn click_outside_page_takes_focus() {
    let mut rig = Rig::new();
    held_in_panel(&mut rig, "tools.html");
    let parent = rig.ui.parent;
    // Taking control gave the page the keyboard.
    // SAFETY: plain Win32 calls on the test's own thread.
    let focus = unsafe { GetFocus() };
    assert!(
        !focus.is_invalid() && focus != parent,
        "the page holds the keyboard before the click"
    );
    // A click that lands outside the page: the app root takes the keyboard back.
    rig.engine.focus_app();
    // SAFETY: as above.
    assert_eq!(unsafe { GetFocus() }, parent);
}

#[test]
fn dpi_change_realigns_page() {
    let mut rig = Rig::new();
    held_in_panel(&mut rig, "tools.html");
    let before = rig
        .engine
        .snapshot("alpha")
        .and_then(|s| s.container)
        .expect("a container");
    // The page's window is somewhere it should not be (a display change moved things): placing again puts it back.
    let container = find_container(&rig);
    // SAFETY: a plain Win32 call on a window of this test.
    unsafe {
        let _ = SetWindowPos(
            container,
            None,
            before.left + 37,
            before.top + 11,
            300,
            200,
            SWP_NOACTIVATE | SWP_NOZORDER,
        );
    }
    let moved = rect_of(container);
    assert_ne!((moved.left, moved.top), (before.left, before.top));
    rig.engine.dpi_changed();
    let after = rect_of(container);
    assert_eq!(
        (
            after.left,
            after.top,
            after.right - after.left,
            after.bottom - after.top
        ),
        (
            before.left,
            before.top,
            before.right - before.left,
            before.bottom - before.top
        ),
        "back at the panel's rectangle"
    );
}

/// The page's container window: the child of the test's parent that is not the parent itself.
fn find_container(rig: &Rig) -> windows::Win32::Foundation::HWND {
    use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
    // SAFETY: plain Win32 calls on the test's own windows.
    let found = unsafe {
        FindWindowExW(
            Some(rig.ui.parent),
            None,
            windows::core::w!("TjommieBrowserHost"),
            None,
        )
    };
    found.expect("the bot's container window")
}

/// A window's rectangle in the parent's client coordinates.
fn rect_of(window: windows::Win32::Foundation::HWND) -> RECT {
    use windows::Win32::Graphics::Gdi::MapWindowPoints;
    use windows::Win32::UI::WindowsAndMessaging::GetParent;
    let mut rect = RECT::default();
    // SAFETY: plain Win32 calls on windows of this test.
    unsafe {
        let _ = GetWindowRect(window, &mut rect);
        let parent = GetParent(window).ok();
        let mut points = [
            windows::Win32::Foundation::POINT {
                x: rect.left,
                y: rect.top,
            },
            windows::Win32::Foundation::POINT {
                x: rect.right,
                y: rect.bottom,
            },
        ];
        MapWindowPoints(None, parent, &mut points);
        RECT {
            left: points[0].x,
            top: points[0].y,
            right: points[1].x,
            bottom: points[1].y,
        }
    }
}

#[test]
fn alignment_rule_at_this_scale() {
    let mut rig = Rig::new();
    held_in_panel(&mut rig, "tools.html");
    let panel = rig.panel().rect;
    let frame = rig
        .engine
        .snapshot("alpha")
        .and_then(|s| s.container)
        .expect("a container");
    // Every edge of the page's frame is within 1 px of the panel's.
    for (name, a, b) in [
        ("left", frame.left, panel.left),
        ("top", frame.top, panel.top),
        ("right", frame.right, panel.right),
        ("bottom", frame.bottom, panel.bottom),
    ] {
        assert!(
            (a - b).abs() <= 1,
            "{name}: the frame is at {a}, the panel at {b}"
        );
    }
    // And the page's viewport, in device pixels, is within 2 px of the frame (whole CSS pixels at a fractional scale).
    let viewport = rig.eval(
        "alpha",
        "JSON.stringify([innerWidth * devicePixelRatio, innerHeight * devicePixelRatio, devicePixelRatio])",
    );
    let numbers: Vec<f64> = serde_json::from_str(&viewport).expect("three numbers");
    let (width, height) = (
        (frame.right - frame.left) as f64,
        (frame.bottom - frame.top) as f64,
    );
    eprintln!(
        "alignment at scale {}: frame {width}x{height}, viewport {}x{}",
        numbers[2], numbers[0], numbers[1]
    );
    assert!(
        (numbers[0] - width).abs() <= 2.0,
        "viewport width {} against the frame's {width}",
        numbers[0]
    );
    assert!(
        (numbers[1] - height).abs() <= 2.0,
        "viewport height {} against the frame's {height}",
        numbers[1]
    );
}

#[test]
fn forwarded_chords_are_reported_and_the_rest_are_the_pages() {
    // The matching is pure and tested in `keyboard.rs`; what reaches the engine is its list. A real key press into the page
    // is the owner's step (`TJOMMIE_OWNER_INPUT=1`, below).
    let mut rig = Rig::new();
    rig.open("alpha");
    let chords: Vec<Chord> = [
        "Ctrl+T",
        "Ctrl+W",
        "Ctrl+Tab",
        "Ctrl+Shift+Tab",
        "Alt+Left",
        "F11",
        "Ctrl+=",
        "Ctrl+-",
        "Ctrl+0",
    ]
    .iter()
    .map(|text| Chord::parse(text).unwrap_or_else(|| panic!("{text} is a chord")))
    .collect();
    rig.engine.set_forwarded_chords(chords.clone());
    for chord in &chords {
        assert!(chord.matches(chord.key, chord.ctrl, chord.shift, chord.alt));
    }
    assert!(
        !chords
            .iter()
            .any(|chord| chord.matches(0x41, false, false, false)),
        "a plain letter is the page's"
    );
}

/// Real keyboard input into the page: Shift+Tab, Tab past the last control, and each app chord, with `SendInput`. It takes
/// the foreground window and types, so it runs only on the owner's PC and only when asked (`TJOMMIE_OWNER_INPUT=1`).
#[test]
#[ignore = "types on the real keyboard: set TJOMMIE_OWNER_INPUT=1 and run with --ignored on the owner's PC"]
fn real_keyboard_input_reaches_the_host() {
    if std::env::var_os("TJOMMIE_OWNER_INPUT").is_none() {
        eprintln!(
            "real_keyboard_input_reaches_the_host: TJOMMIE_OWNER_INPUT is not set, so no key was typed"
        );
        return;
    }
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
        VIRTUAL_KEY, VK_CONTROL, VK_SHIFT, VK_TAB,
    };
    use windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
    let send = |keys: &[(VIRTUAL_KEY, bool)]| {
        let inputs: Vec<INPUT> = keys
            .iter()
            .map(|(vk, up)| INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: *vk,
                        wScan: 0,
                        dwFlags: if *up {
                            KEYEVENTF_KEYUP
                        } else {
                            KEYBD_EVENT_FLAGS(0)
                        },
                        time: 0,
                        dwExtraInfo: 0,
                    },
                },
            })
            .collect();
        // SAFETY: plain input injection of the keys named above.
        unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
    };
    let mut rig = Rig::new();
    held_in_panel(&mut rig, "tools.html");
    rig.engine
        .set_forwarded_chords(vec![Chord::parse("Ctrl+T").expect("a chord")]);
    // SAFETY: a plain Win32 call on the test's own window.
    unsafe {
        let _ = SetForegroundWindow(rig.ui.parent);
    }
    rig.eval("alpha", "document.getElementById('name').focus(); 1");
    rig.notifications();
    send(&[
        (VK_SHIFT, false),
        (VK_TAB, false),
        (VK_TAB, true),
        (VK_SHIFT, true),
    ]);
    rig.ui.settle(Duration::from_millis(800));
    assert!(
        focus_returns(&rig).contains(&FocusDirection::Previous),
        "Shift+Tab with the real keyboard leaves the page"
    );
    rig.notifications();
    send(&[
        (VK_CONTROL, false),
        (VIRTUAL_KEY(0x54), false),
        (VIRTUAL_KEY(0x54), true),
        (VK_CONTROL, true),
    ]);
    rig.ui.settle(Duration::from_millis(800));
    let shortcuts = rig
        .notifications()
        .into_iter()
        .filter(|item| matches!(item, Notification::Shortcut { .. }))
        .count();
    assert_eq!(shortcuts, 1, "Ctrl+T is the app's, not the page's");
}
