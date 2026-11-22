//! Motion checks (Phase 3 plan T7.2; the spike's `motion_scenarios.rs`). In a test-driver build, `--scenario <name>`
//! opens the app on invented data (six agents, two rooms, Forge's chat; no daemon, no network), drives one motion the
//! way the pointer would, `--loops` times (50), and quits. `--markers <csv>` gets the run's QPC span and, beside it
//! (`<stem>.windows.csv`), the span of each loop's motion, so `tools/perf/frame-stats.py` counts only frames that moved;
//! `--trace <csv>` is the motion clock's own frame log; `--out <jsonl>` gets one result line per scenario or check, with
//! `pass`. The exit code is 0 only when the run passed. `--mmcss` and `--above-normal` raise the UI thread for the heavy-
//! load experiment (T7.3). Sample data is invented: no real names, paths or accounts.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use desktop_host::perf::{qpc, qpc_frequency};
use gpui::{
    AnyWindowHandle, App, AsyncApp, Modifiers, PlatformInput, ScrollDelta, ScrollEvent,
    ScrollSource, TouchPhase, Window, point, px,
};
use protocol::model::{
    Autonomy, Bot, BotColor, BotShape, BotState, Message, MessageKind, MessageStatus, Provider,
    Room,
};
use serde_json::{Value, json};

use crate::guard::guarded_async;
use crate::model::state::{AppState, Connection, Conversation, ConversationStatus};
use crate::model::testing::FakeRpc;
use crate::motion::clock::{self, Live};
use crate::notices::{Notice, Notices};
use crate::ui::shell::data::{self, Recent, RecentKind, ShellData};
use crate::ui::shell::route::{self, Navigate as _, Route};

/// The timed scenarios: `motion-<name>`, each `--loops` pointer-triggered loops.
pub const TIMED: [&str; 6] = [
    "motion-recents",
    "motion-flight",
    "motion-toast",
    "motion-panel",
    "motion-numbers",
    "motion-arrival",
];

/// A run failed a check, or couldn't run: the exit code says so.
static FAILED: AtomicBool = AtomicBool::new(false);

pub struct Options {
    pub name: String,
    loops: usize,
    seconds: f32,
    markers: Option<PathBuf>,
    out: Option<PathBuf>,
}

/// The scenario the command line asks for, if any.
pub fn options() -> Option<Options> {
    let args: Vec<String> = std::env::args_os()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    let value = |flag: &str| {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|at| args.get(at + 1))
            .cloned()
    };
    // A --scenario with no name runs nothing and fails, rather than starting the app as usual (M7 review n10).
    let at = args.iter().position(|arg| arg == "--scenario")?;
    let name = args.get(at + 1).cloned().unwrap_or_default();
    Some(Options {
        name,
        loops: value("--loops").and_then(|n| n.parse().ok()).unwrap_or(50),
        seconds: value("--seconds")
            .and_then(|s| s.parse().ok())
            .unwrap_or(20.),
        markers: value("--markers").map(PathBuf::from),
        out: value("--out").map(PathBuf::from),
    })
}

/// Whether the run failed: `main` exits with 1 then.
pub fn failed() -> bool {
    FAILED.load(Ordering::SeqCst)
}

fn bot(id: &str, name: &str, color: BotColor, shape: BotShape, state: BotState) -> Bot {
    Bot {
        id: id.into(),
        name: name.into(),
        color,
        shape,
        instructions: String::new(),
        provider: Provider::Claude,
        model: "sonnet".into(),
        account_id: None,
        effective_account_id: None,
        using_fallback: None,
        autonomy: Autonomy::Ask,
        home_folder: format!("C:/Agents/{name}"),
        project_folder: None,
        browser: false,
        paused: state == BotState::Paused,
        can_message: Vec::new(),
        state,
        setup_reason: None,
        limited_until: None,
        created_at: "2026-11-01T09:00:00.000Z".into(),
    }
}

fn message(id: usize, author: &str, body: &str) -> Message {
    Message {
        id: format!("m{id}"),
        conversation_id: "forge".into(),
        author: author.into(),
        kind: MessageKind::Text,
        body: body.into(),
        data: None,
        status: MessageStatus::Done,
        created_at: format!(
            "2026-11-20T{:02}:{:02}:{:02}.000Z",
            8 + id / 3600 % 12,
            id / 60 % 60,
            id % 60
        ),
    }
}

/// Forge's chat: `count` messages, yours and Forge's in turn, Forge's with a code block now and then.
fn chat(count: usize) -> Vec<Message> {
    (0..count)
        .map(|n| {
            if n % 2 == 0 {
                message(n, "you", "Port the next migration to the Rust daemon.")
            } else if n % 6 == 1 {
                message(
                    n,
                    "forge",
                    "Ported it. `open_db` runs each one in a transaction:\n\n```rust\nfor sql in MIGRATIONS {\n    let tx = conn.transaction()?;\n    tx.execute_batch(sql)?;\n    tx.commit()?;\n}\n```",
                )
            } else {
                message(n, "forge", "Done: the tests pass and the schema matches.")
            }
        })
        .collect()
}

/// The invented data: six agents, none working (a working agent's scan never rests, which a loop waits for; the
/// living scenario sets two to work, the idle check one), two rooms, and Forge's chat (10,000 messages for the scroll).
fn fixture(name: &str) -> AppState {
    let (scout, quill) = match name {
        "motion-alive" => (BotState::Working, BotState::Working),
        // One agent at work among quiet ones: what a window not in front must not redraw for (M7 review m8).
        "motion-idle" => (BotState::Working, BotState::Idle),
        _ => (BotState::Idle, BotState::Idle),
    };
    let bots = vec![
        bot(
            "forge",
            "Forge",
            BotColor::Blue,
            BotShape::Hexagon,
            BotState::Waiting,
        ),
        bot("scout", "Scout", BotColor::Teal, BotShape::Blob, scout),
        bot("quill", "Quill", BotColor::Purple, BotShape::Droplet, quill),
        bot(
            "ledger",
            "Ledger",
            BotColor::Amber,
            BotShape::RoundedSquare,
            BotState::Idle,
        ),
        bot(
            "pixel",
            "Pixel",
            BotColor::Pink,
            BotShape::Cloud,
            BotState::Limited,
        ),
        bot(
            "atlas",
            "Atlas",
            BotColor::Green,
            BotShape::Triangle,
            BotState::Paused,
        ),
    ];
    let rooms = vec![
        Room {
            id: "launch".into(),
            name: "Launch crew".into(),
            bot_ids: vec!["forge".into(), "scout".into()],
            everyone: false,
            created_at: Some("2026-11-02T09:00:00.000Z".into()),
        },
        Room {
            id: "everyone".into(),
            name: "Everyone".into(),
            bot_ids: Vec::new(),
            everyone: true,
            created_at: Some("2026-11-02T09:00:00.000Z".into()),
        },
    ];
    let count = if name == "scroll-10k" { 10_000 } else { 12 };
    let mut state = AppState {
        connection: Connection::Ready,
        bots,
        rooms,
        ..AppState::default()
    };
    state.conversations.insert(
        "forge".into(),
        Conversation {
            status: ConversationStatus::Ready,
            messages: chat(count),
            has_more: false,
            pending: Vec::new(),
        },
    );
    state
}

/// The Recents the live adapter would make of `state`: the agents in order, then the rooms.
fn recents(state: &AppState) -> Vec<Recent> {
    let bots = state.bots.iter().map(|bot| Recent {
        id: bot.id.clone().into(),
        name: bot.name.clone().into(),
        kind: RecentKind::Bot {
            color: bot.color,
            shape: bot.shape,
            state: bot.state,
        },
    });
    let rooms = state.rooms.iter().map(|room| Recent {
        id: room.id.clone().into(),
        name: room.name.clone().into(),
        kind: RecentKind::Room { unread: false },
    });
    bots.chain(rooms).collect()
}

/// Puts the invented data in place of the daemon, before the window opens.
pub fn install(options: &Options, cx: &mut App) {
    let state = fixture(&options.name);
    cx.set_global(ShellData {
        connection: data::Connection::Ready,
        recents: recents(&state),
        ..ShellData::default()
    });
    // The browser panel reads its agents from what the live connection feeds it.
    crate::ui::browser::feed(&state, cx);
    // Answers the calls the screens make on their own (the newest page of a chat is already in the state).
    crate::ui::store::fake(cx, state, FakeRpc::default());
    let start = match options.name.as_str() {
        "motion-panel" | "motion-arrival" | "scroll-10k" => Route::Chat("forge".into()),
        _ => Route::Agents,
    };
    cx.navigate(start);
    raise_thread(cx);
}

/// T7.3: `--mmcss` registers the UI thread with MMCSS as a game's, `--above-normal` raises its priority.
fn raise_thread(_cx: &mut App) {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if args.iter().any(|arg| arg == "--mmcss") {
        match desktop_host::perf::mmcss_games() {
            Ok(()) => log::info!("The UI thread is an MMCSS Games thread"),
            Err(error) => log::warn!("No MMCSS for the UI thread: {error}"),
        }
    }
    if args.iter().any(|arg| arg == "--above-normal") {
        match desktop_host::perf::above_normal() {
            Ok(()) => log::info!("The UI thread runs above normal priority"),
            Err(error) => log::warn!("The UI thread's priority stays: {error}"),
        }
    }
}

/// Runs the scenario in `window` once the app has settled, then quits.
pub fn start(options: Options, window: AnyWindowHandle, cx: &mut App) {
    cx.spawn(async move |cx| {
        guarded_async("run a motion scenario", async move {
            let passed = match run(&options, window, cx).await {
                Ok(passed) => passed,
                Err(error) => {
                    log::error!("Scenario {} failed: {error}", options.name);
                    record(
                        &options,
                        json!({ "scenario": options.name, "pass": false, "error": error }),
                    );
                    false
                }
            };
            if !passed {
                FAILED.store(true, Ordering::SeqCst);
            }
            cx.update(|cx| cx.quit());
        })
        .await;
    })
    .detach();
}

fn record(options: &Options, line: Value) {
    log::info!("Scenario result: {line}");
    if let Some(out) = &options.out {
        use std::io::Write as _;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(out)
        {
            let _ = writeln!(file, "{line}");
        }
    }
}

async fn sleep(cx: &AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

/// Runs `f` in the window.
fn act<R>(
    window: AnyWindowHandle,
    cx: &mut AsyncApp,
    f: impl FnOnce(&mut Window, &mut App) -> R,
) -> Result<R, String> {
    window
        .update(cx, |_, window, cx| f(window, cx))
        .map_err(|error| format!("the window is gone: {error}"))
}

/// Clicks the element with `id`, as the pointer does.
fn click(window: AnyWindowHandle, id: &str, cx: &mut AsyncApp) -> Result<(), String> {
    act(window, cx, |window, cx| {
        crate::test_driver::click_id(id, window, cx)
    })?
}

/// Waits until a frame that began after `after` (QPC) is drawn with nothing moving.
async fn until_rest(after: i64, cx: &mut AsyncApp, timeout_ms: u64) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        let rest = cx.update(|cx| {
            clock::frames(cx)
                .last()
                .is_some_and(|frame| frame.begin_qpc > after && !frame.moving)
        });
        if rest {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(format!("nothing came to rest within {timeout_ms} ms"));
        }
        sleep(cx, 5).await;
    }
}

/// What a scenario does each loop, the way the pointer would.
async fn trigger(
    name: &str,
    i: usize,
    window: AnyWindowHandle,
    cx: &mut AsyncApp,
) -> Result<(), String> {
    match name {
        // Something changes state and Recents reorders: the first row moves to the end, the others slide up.
        "motion-recents" => {
            cx.update(|cx| {
                cx.global_mut::<ShellData>().recents.rotate_left(1);
                cx.refresh_windows();
            });
            Ok(())
        }
        // Forge's and Ledger's rows in turn: each opens its chat, and its avatar flies into the header.
        "motion-flight" => {
            let row = if i.is_multiple_of(2) {
                "sidebar.recent.ledger"
            } else {
                "sidebar.recent.forge"
            };
            click(window, row, cx)
        }
        // The panel button in Forge's chat header: open, then closed.
        "motion-panel" => click(window, "chat.forge.panel", cx),
        // Ledger asks for you and stops: the Needs you count rolls between 1 and 2.
        "motion-numbers" => {
            cx.update(|cx| {
                crate::ui::store::update(cx, |state| {
                    if let Some(ledger) = state.bots.iter_mut().find(|bot| bot.id == "ledger") {
                        ledger.state = if ledger.state == BotState::Waiting {
                            BotState::Idle
                        } else {
                            BotState::Waiting
                        };
                    }
                    true
                });
            });
            Ok(())
        }
        // A new reply arrives at the bottom of Forge's chat.
        "motion-arrival" => {
            cx.update(|cx| {
                crate::ui::store::update(cx, |state| {
                    if let Some(chat) = state.conversations.get_mut("forge") {
                        let n = chat.messages.len() + 1;
                        chat.messages.push(message(
                            n,
                            if n % 2 == 0 { "you" } else { "forge" },
                            "Done: the tests pass and the schema matches.",
                        ));
                    }
                    true
                });
            });
            Ok(())
        }
        other => Err(format!("unknown scenario {other}")),
    }
}

/// p95 runs: the motion `loops` times, each loop's span recorded, then the markers written.
async fn loops(
    options: &Options,
    window: AnyWindowHandle,
    cx: &mut AsyncApp,
) -> Result<bool, String> {
    let mut windows = Vec::with_capacity(options.loops);
    let mut durations = Vec::with_capacity(options.loops);
    let start = qpc();
    let drawn_before = cx.update(|cx| clock::drawn(cx));
    for i in 0..options.loops {
        if options.name == "motion-toast" {
            // In, then out: two spans a loop.
            for step in [false, true] {
                let began = qpc();
                let timer = Instant::now();
                if step {
                    click(window, "toast.close.0", cx)?;
                } else {
                    cx.update(|cx| {
                        cx.set_global(Notices(vec![Notice::SomethingWentWrong]));
                        cx.refresh_windows();
                    });
                }
                until_rest(began, cx, 4000).await?;
                windows.push((began, qpc()));
                // Each span is a motion, in or out: its time counts (M7 review n5).
                durations.push(timer.elapsed().as_secs_f64() * 1000.);
                sleep(cx, 150).await;
            }
            continue;
        }
        let began = qpc();
        let timer = Instant::now();
        trigger(&options.name, i, window, cx).await?;
        until_rest(began, cx, 4000).await?;
        windows.push((began, qpc()));
        durations.push(timer.elapsed().as_secs_f64() * 1000.);
        sleep(cx, 150).await;
    }
    write_markers(options, start, qpc(), &windows, drawn_before, cx);
    durations.sort_by(f64::total_cmp);
    let median = durations.get(durations.len() / 2).copied();
    record(
        options,
        json!({ "scenario": options.name, "pass": true, "loops": options.loops, "motion_ms_p50": median,
                "motion_ms_max": durations.last() }),
    );
    Ok(true)
}

fn write_markers(
    options: &Options,
    start: i64,
    end: i64,
    windows: &[(i64, i64)],
    drawn_before: u64,
    cx: &mut AsyncApp,
) {
    let Some(markers) = &options.markers else {
        return;
    };
    // Every frame drawn in the run: the log is a ring of the last 2048 (M7 review n4).
    let frames = cx
        .update(|cx| clock::drawn(cx))
        .saturating_sub(drawn_before);
    let _ = std::fs::write(
        markers,
        format!(
            "start_qpc,end_qpc,qpc_freq,frames\n{start},{end},{},{frames}\n",
            qpc_frequency()
        ),
    );
    if !windows.is_empty() {
        let rows: String = windows
            .iter()
            .map(|(began, ended)| format!("{began},{ended}\n"))
            .collect();
        let _ = std::fs::write(
            markers.with_extension("windows.csv"),
            format!("start_qpc,end_qpc\n{rows}"),
        );
    }
}

/// The living avatars for `--seconds`: every agent blinks on its own clock, two scan as they work.
async fn alive(options: &Options, cx: &mut AsyncApp) -> Result<bool, String> {
    let start = qpc();
    let drawn_before = cx.update(|cx| clock::drawn(cx));
    sleep(cx, (options.seconds * 1000.) as u64).await;
    let end = qpc();
    write_markers(options, start, end, &[(start, end)], drawn_before, cx);
    record(
        options,
        json!({ "scenario": options.name, "pass": true, "seconds": options.seconds }),
    );
    Ok(true)
}

/// The app at rest (M7 review m8, the owner's "stop when not in front"): with one agent at work among quiet ones, the
/// frames drawn over `--seconds` in front, then with the window not in front (the taskbar takes the foreground; the
/// window stays visible), which must stay at most 1 a second. The in-front rate is reported beside it. A window that
/// opened behind (launched by a process not in front, which Windows lets take no foreground) is measured behind only.
async fn idle(
    options: &Options,
    window: AnyWindowHandle,
    cx: &mut AsyncApp,
) -> Result<bool, String> {
    let seconds = f64::from(options.seconds).max(1.);
    let ms = (seconds * 1000.) as u64;
    let rate = |before: u64, cx: &mut AsyncApp| {
        cx.update(|cx| clock::drawn(cx)).saturating_sub(before) as f64 / seconds
    };
    let was_in_front = act(window, cx, |window, _| window.is_window_active())?;
    let mut in_front = None;
    if was_in_front {
        let before = cx.update(|cx| clock::drawn(cx));
        sleep(cx, ms).await;
        in_front = Some(rate(before, cx));
        // Outside any update: Windows activates the taskbar, and the window's deactivation comes as a message.
        desktop_host::perf::put_taskbar_in_front()?;
        // The redraw that comes with the deactivation.
        sleep(cx, 1000).await;
    }
    let behind_now = act(window, cx, |window, _| !window.is_window_active())?;
    let before = cx.update(|cx| clock::drawn(cx));
    sleep(cx, ms).await;
    let behind = rate(before, cx);
    let pass = behind_now && behind <= 1.;
    record(
        options,
        json!({ "scenario": options.name, "pass": pass, "seconds": seconds, "bar_frames_per_second": 1.0,
                "frames_per_second_behind": behind, "was_behind": behind_now,
                "frames_per_second_in_front": in_front, "was_in_front": was_in_front }),
    );
    Ok(pass)
}

/// The 10,000-message thread, scrolled up and back by the wheel, `loops` times.
async fn scroll(
    options: &Options,
    window: AnyWindowHandle,
    cx: &mut AsyncApp,
) -> Result<bool, String> {
    let start = qpc();
    let drawn_before = cx.update(|cx| clock::drawn(cx));
    let mut windows = Vec::new();
    for i in 0..options.loops {
        let began = qpc();
        let up = i % 2 == 0;
        for _ in 0..20 {
            act(window, cx, |window, cx| {
                let size = window.viewport_size();
                let position = point(size.width * 0.6, size.height * 0.5);
                window.dispatch_event(
                    PlatformInput::Scroll(ScrollEvent {
                        position,
                        delta: ScrollDelta::Pixels(point(
                            px(0.),
                            px(if up { 120. } else { -120. }),
                        )),
                        modifiers: Modifiers::none(),
                        touch_phase: TouchPhase::Moved,
                        source: ScrollSource::Pointer,
                    }),
                    cx,
                );
            })?;
            sleep(cx, 16).await;
        }
        windows.push((began, qpc()));
    }
    write_markers(options, start, qpc(), &windows, drawn_before, cx);
    record(
        options,
        json!({ "scenario": options.name, "pass": true, "loops": options.loops }),
    );
    Ok(true)
}

/// Keyboard actions never animate: opened by a key, the chat shows with no flight; closed by a key, a toast is gone at
/// once. Under `--force-reduced`, the same by the pointer, and Recents reorder in place. Each counts the frames the motion
/// drew after the action, by the frame log's marks (M7 review m3): a motion that ran one frame fails it.
async fn snaps(
    options: &Options,
    window: AnyWindowHandle,
    cx: &mut AsyncApp,
) -> Result<bool, String> {
    let by_key = options.name == "motion-keyboard";
    let mut passed = true;
    // The flight.
    let began = qpc();
    if by_key {
        act(window, cx, |window, cx| {
            if !crate::ui::primitives::focus_by_id("sidebar.recent.ledger", window, cx) {
                return Err("Ledger's Recents row took no focus".to_owned());
            }
            crate::test_driver::press_key("enter", window, cx)
        })??;
    } else {
        click(window, "sidebar.recent.ledger", cx)?;
    }
    // Long enough for a flight to have drawn frames, were there one.
    sleep(cx, 300).await;
    let flying = cx.update(|cx| marked(began, clock::FLIGHT, cx));
    let opened = cx.update(|cx| route::current(cx)) == Route::Chat("ledger".into());
    passed &= opened && flying == 0;
    record(
        options,
        json!({ "scenario": options.name, "motion": "flight", "pass": opened && flying == 0, "state_changed": opened,
                "moving_frames": flying }),
    );
    // The toast: raised, drawn and at rest before it is closed.
    let raised = qpc();
    cx.update(|cx| {
        cx.set_global(Notices(vec![Notice::SomethingWentWrong]));
        cx.refresh_windows();
    });
    until_rest(raised, cx, 4000).await?;
    let began = qpc();
    if by_key {
        act(window, cx, |window, cx| {
            if !crate::ui::primitives::focus_by_id("toast.close.0", window, cx) {
                return Err("the toast's Close took no focus".to_owned());
            }
            crate::test_driver::press_key("enter", window, cx)
        })??;
    } else {
        click(window, "toast.close.0", cx)?;
    }
    sleep(cx, 300).await;
    let leaving = cx.update(|cx| marked(began, clock::TOAST, cx));
    let closed = cx.update(|cx| {
        cx.try_global::<Notices>()
            .is_some_and(|notices| notices.0.is_empty())
    });
    passed &= closed && leaving == 0;
    record(
        options,
        json!({ "scenario": options.name, "motion": "toast", "pass": closed && leaving == 0, "state_changed": closed,
                "moving_frames": leaving }),
    );
    // Recents, under reduced motion: nothing moves at all, so the frame log says it.
    if !by_key {
        let began = qpc();
        cx.update(|cx| {
            cx.global_mut::<ShellData>().recents.rotate_left(1);
            cx.refresh_windows();
        });
        sleep(cx, 200).await;
        let moving = cx.update(|cx| {
            clock::frames(cx)
                .iter()
                .filter(|frame| frame.begin_qpc > began && frame.moving)
                .count()
        });
        passed &= moving == 0;
        record(
            options,
            json!({ "scenario": options.name, "motion": "recents", "pass": moving == 0, "moving_frames": moving }),
        );
    }
    Ok(passed)
}

/// The frames since `began` that the motion `mark` drew in (`clock::FLIGHT`, `clock::TOAST`).
fn marked(began: i64, mark: u8, cx: &App) -> usize {
    clock::frames(cx)
        .iter()
        .filter(|frame| frame.begin_qpc > began && frame.marks & mark != 0)
        .count()
}

async fn run(
    options: &Options,
    window: AnyWindowHandle,
    cx: &mut AsyncApp,
) -> Result<bool, String> {
    // A motion check keeps the display's rate even when the window isn't in front (gate condition 5); the idle check
    // measures the app as it is, so it holds nothing.
    let _live = (options.name != "motion-idle").then(|| cx.update(Live::hold));
    // Fonts, icons and the first frames.
    sleep(cx, 1500).await;
    match options.name.as_str() {
        "motion-alive" => alive(options, cx).await,
        "motion-idle" => idle(options, window, cx).await,
        "scroll-10k" => scroll(options, window, cx).await,
        "motion-keyboard" | "motion-reduced" => snaps(options, window, cx).await,
        name if TIMED.contains(&name) => loops(options, window, cx).await,
        other => Err(format!("unknown scenario {other}")),
    }
}
