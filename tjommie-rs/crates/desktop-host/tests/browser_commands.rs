//! T3.2: the commands a bot's browser takes, against the real WebView2 runtime: the person's own clicks, remote
//! controllers' fencing and input, dialogs, popups, permissions and the navigation guard. Pages come from loopback
//! fixtures; no real site is visited.

// Test helpers may panic: a failure here is the test failing (the deny lints guard shipped code).
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::time::Duration;

use common::Rig;
use desktop_host::browser::engine::BotEvent;
use protocol::browser::{BrowserControl, BrowserState};
use serde_json::{Value, json};

fn wait_state(
    rig: &mut Rig,
    id: &str,
    what: &str,
    condition: impl Fn(&BrowserState) -> bool + 'static,
) {
    let (engine, id) = (rig.engine.clone(), id.to_string());
    rig.ui.wait_until(what, Duration::from_secs(15), move || {
        engine
            .state(&id, "local")
            .is_ok_and(|state| condition(&state))
    });
}

fn page_command(kind: &str, state: &BrowserState, extra: Value) -> Value {
    let mut command =
        json!({ "kind": kind, "generation": state.generation, "pageId": state.active_page_id });
    if let (Some(command), Some(extra)) = (command.as_object_mut(), extra.as_object()) {
        command.extend(extra.clone());
    }
    command
}

fn take_control(rig: &mut Rig, id: &str) -> BrowserState {
    rig.exec_state(id, json!({ "kind": "control", "control": "human" }))
}

fn remote_take(rig: &mut Rig, controller: &str) -> BrowserState {
    let reply = rig
        .exec(
            "alpha",
            remote(controller, json!({ "kind": "control", "control": "human" })),
        )
        .expect("take control");
    match reply {
        desktop_host::browser::engine::Reply::State(state) => *state,
        other => panic!("{other:?}"),
    }
}

fn remote(controller: &str, action: Value) -> Value {
    json!({ "kind": "remote", "controllerId": controller, "action": action })
}

#[test]
fn a_person_must_take_control_before_a_page_command_and_a_stale_session_is_refused() {
    let mut rig = Rig::new();
    let state = rig.exec_state("alpha", json!({ "kind": "open" }));
    assert_eq!(
        (state.bot_id.as_str(), state.tabs.len(), state.control),
        ("alpha", 1, BrowserControl::Bot)
    );
    assert_eq!(state.can_control, Some(false));
    let url = rig.server.url("controls.html");

    let navigate = page_command("navigate", &state, json!({ "url": url }));
    assert_eq!(
        rig.exec_error("alpha", navigate.clone()),
        "Browser control changed. Take control again and use the current frame."
    );
    let held = take_control(&mut rig, "alpha");
    assert_eq!(
        (held.control, held.can_control),
        (BrowserControl::Human, Some(true))
    );
    assert!(held.control_epoch > state.control_epoch);

    rig.exec_state("alpha", navigate);
    wait_state(&mut rig, "alpha", "the page to arrive", {
        let url = url.clone();
        move |state| state.tabs.first().is_some_and(|tab| tab.url == url)
    });
    // The title follows the address by a moment.
    wait_state(&mut rig, "alpha", "the page's title", |state| {
        state
            .tabs
            .first()
            .is_some_and(|tab| tab.title == "Tjommie fixture")
    });

    let stale = page_command(
        "navigate",
        &BrowserState {
            generation: "an-old-session".into(),
            ..held.clone()
        },
        json!({ "url": url }),
    );
    assert_eq!(
        rig.exec_error("alpha", stale),
        "The browser session changed; refresh its state."
    );
    let refused = page_command("navigate", &held, json!({ "url": "javascript:alert(1)" }));
    assert_eq!(
        rig.exec_error("alpha", refused),
        "Browser navigation only supports HTTP, HTTPS and about:blank."
    );
    let back = rig.exec_state("alpha", json!({ "kind": "control", "control": "bot" }));
    assert_eq!(
        (back.control, back.can_control),
        (BrowserControl::Bot, Some(false))
    );
}

#[test]
fn tabs_open_switch_and_close() {
    let mut rig = Rig::new();
    let state = rig.exec_state("alpha", json!({ "kind": "open" }));
    take_control(&mut rig, "alpha");
    let first = state.active_page_id.clone().expect("first tab");
    let opened = rig.exec_state(
        "alpha",
        page_command("tab", &state, json!({ "action": "new" })),
    );
    assert_eq!(opened.tabs.len(), 2);
    let second = opened.active_page_id.clone().expect("second tab");
    assert_ne!(first, second);
    let back = rig.exec_state(
        "alpha",
        page_command(
            "tab",
            &opened,
            json!({ "action": "select", "pageId": first }),
        ),
    );
    assert_eq!(back.active_page_id.as_deref(), Some(first.as_str()));
    let closed = rig.exec_state(
        "alpha",
        page_command("tab", &back, json!({ "action": "close", "pageId": second })),
    );
    assert_eq!(closed.tabs.len(), 1);
    assert_eq!(closed.active_page_id.as_deref(), Some(first.as_str()));
    // Each change moved the revision on, so a watcher can tell.
    assert!(closed.revision > state.revision);
}

#[test]
fn a_javascript_dialog_waits_for_an_answer_and_the_page_carries_on() {
    let mut rig = Rig::new();
    rig.open("alpha");
    rig.load("alpha", "blank.html");
    let awake = rig.begin("alpha");
    let held = take_control(&mut rig, "alpha");
    assert!(held.dialog.is_none());
    rig.eval(
        "alpha",
        "setTimeout(() => { window.answer = confirm('Proceed?'); }, 0); 'scheduled'",
    );
    wait_state(&mut rig, "alpha", "the dialog to open", |state| {
        state.dialog.is_some()
    });
    let dialog = rig.state("alpha").dialog.expect("dialog");
    assert_eq!(
        (dialog.kind.as_str(), dialog.message.as_str()),
        ("confirm", "Proceed?")
    );
    let state = rig.state("alpha");
    rig.exec_state(
        "alpha",
        page_command("dialog", &state, json!({ "accept": true })),
    );
    wait_state(&mut rig, "alpha", "the dialog to close", |state| {
        state.dialog.is_none()
    });
    // The page goes on a moment after its dialog is answered; until then it may refuse a command.
    rig.eval_until(
        "alpha",
        "String(window.answer)",
        "true",
        Duration::from_secs(15),
    );
    // Answering with no dialog open is refused.
    let state = rig.state("alpha");
    assert_eq!(
        rig.exec_error(
            "alpha",
            page_command("dialog", &state, json!({ "accept": true }))
        ),
        "There is no dialog in that tab."
    );
    awake.end();
}

#[test]
fn remote_controllers_are_fenced_by_owner_and_epoch_and_stay_gone_once_disconnected() {
    let mut rig = Rig::new();
    let state = rig.exec_state("alpha", json!({ "kind": "open" }));
    let url = rig.server.url("controls.html");
    let phone = |action: Value| remote("phone-1", action);

    let taken = remote_take(&mut rig, "phone-1");
    assert_eq!(
        (taken.control, taken.can_control),
        (BrowserControl::Human, Some(true))
    );
    // A second device cannot take it, and cannot see itself as the controller.
    assert_eq!(
        rig.exec_error(
            "alpha",
            remote("phone-2", json!({ "kind": "control", "control": "human" }))
        ),
        "Another device is controlling this browser."
    );
    let epoch = taken.control_epoch.expect("epoch");
    let navigate = |epoch: u64, url: &str| {
        phone(
            json!({ "kind": "navigate", "controlEpoch": epoch, "generation": state.generation, "pageId": state.active_page_id, "url": url }),
        )
    };
    assert_eq!(
        rig.exec_error("alpha", navigate(epoch + 7, &url)),
        "Browser control changed. Take control again and use the current frame."
    );
    rig.exec("alpha", navigate(epoch, &url))
        .expect("navigate with the current epoch");
    wait_state(&mut rig, "alpha", "the page to arrive", {
        let url = url.clone();
        move |state| state.tabs.first().is_some_and(|tab| tab.url == url)
    });
    // The page changed under the controller: what it saw is stale.
    assert!(rig.state("alpha").control_epoch > Some(epoch));
    assert_eq!(
        rig.exec_error(
            "alpha",
            phone(json!({ "kind": "control", "control": "bot" }))
        ),
        "Use the current browser control epoch."
    );
    // Disconnecting hands the browser back, and the controller's late commands are refused.
    rig.exec("alpha", phone(json!({ "kind": "disconnect" })))
        .expect("disconnect");
    assert_eq!(rig.state("alpha").control, BrowserControl::Bot);
    assert_eq!(
        rig.exec_error("alpha", phone(json!({ "kind": "state" }))),
        "This browser controller disconnected."
    );
}

#[test]
fn remote_input_clicks_types_and_stays_inside_the_viewport() {
    let mut rig = Rig::new();
    rig.open("alpha");
    rig.load("alpha", "controls.html");
    let taken = remote_take(&mut rig, "phone-1");
    assert_eq!(taken.can_control, Some(true));
    let input = |state: &BrowserState, input: Value| {
        remote(
            "phone-1",
            json!({
                "kind": "input", "controlEpoch": state.control_epoch, "generation": state.generation,
                "pageId": state.active_page_id, "viewportRevision": state.viewport_revision, "input": input,
            }),
        )
    };
    let center = |rig: &mut Rig, id: &str| -> (f64, f64) {
        let rect = rig.eval("alpha", &format!("(() => {{ const r = document.getElementById('{id}').getBoundingClientRect(); return `${{r.x + r.width / 2}},${{r.y + r.height / 2}}`; }})()"));
        let (x, y) = rect.split_once(',').expect("x,y");
        (x.parse().expect("x"), y.parse().expect("y"))
    };

    // A real click on the Count button.
    let (x, y) = center(&mut rig, "count");
    for action in ["move", "down", "up"] {
        let state = rig.state("alpha");
        rig.exec(
            "alpha",
            input(
                &state,
                json!({ "kind": "mouse", "action": action, "x": x, "y": y, "button": "left" }),
            ),
        )
        .expect("mouse");
    }
    assert_eq!(
        rig.eval("alpha", "document.getElementById('clicks').value"),
        "1"
    );

    // Click into the Name box and type: text, then one key.
    let (x, y) = center(&mut rig, "person");
    for action in ["down", "up"] {
        let state = rig.state("alpha");
        rig.exec(
            "alpha",
            input(
                &state,
                json!({ "kind": "mouse", "action": action, "x": x, "y": y, "button": "left" }),
            ),
        )
        .expect("mouse");
    }
    let state = rig.state("alpha");
    rig.exec(
        "alpha",
        input(&state, json!({ "kind": "text", "text": "hello" })),
    )
    .expect("text");
    for action in ["down", "up"] {
        let state = rig.state("alpha");
        rig.exec(
            "alpha",
            input(
                &state,
                json!({ "kind": "key", "action": action, "key": "x" }),
            ),
        )
        .expect("key");
    }
    assert_eq!(
        rig.eval("alpha", "document.getElementById('person').value"),
        "hellox"
    );

    // Outside the viewport, and against an old frame, input is refused.
    let state = rig.state("alpha");
    assert_eq!(
        rig.exec_error(
            "alpha",
            input(
                &state,
                json!({ "kind": "mouse", "action": "move", "x": 99999, "y": 5 })
            )
        ),
        "Browser input must be inside the current viewport."
    );
    let old = BrowserState {
        viewport_revision: Some(999),
        ..state.clone()
    };
    assert_eq!(
        rig.exec_error(
            "alpha",
            input(
                &old,
                json!({ "kind": "mouse", "action": "move", "x": 5, "y": 5 })
            )
        ),
        "The browser viewport changed; wait for its current frame."
    );
    assert_eq!(
        rig.exec_error(
            "alpha",
            input(
                &state,
                json!({ "kind": "mouse", "action": "down", "x": 5, "y": 5 })
            )
        ),
        "A pointer press or release needs a button."
    );
}

#[test]
fn a_popup_becomes_a_tab_of_the_same_bot_and_a_refused_address_opens_nothing() {
    let mut rig = Rig::new();
    rig.open("alpha");
    rig.load("alpha", "blank.html");
    // The bot is at work in its browser, so the page is awake while popups come and go.
    let awake = rig.begin("alpha");
    let tab = awake.tab().expect("active tab");
    let page = rig.server.url("controls.html");
    // A script popup counts as a gesture here, as a click would.
    let open = |tab: desktop_host::browser::engine::TabHandle, address: String, rig: &mut Rig| {
        let params = json!({ "expression": format!("window.open('{address}'); 'opened'"), "returnByValue": true, "userGesture": true }).to_string();
        rig.ui.run(async move {
            tab.call("Runtime.evaluate", &params)
                .await
                .expect("evaluate")
        });
    };
    open(tab.clone(), page.clone(), &mut rig);
    wait_state(&mut rig, "alpha", "the popup's tab", |state| {
        state.tabs.len() == 2
    });
    let state = rig.state("alpha");
    assert_eq!(
        state.active_page_id.as_deref(),
        state.tabs.last().map(|tab| tab.id.as_str()),
        "the popup is the active tab"
    );
    wait_state(&mut rig, "alpha", "the popup to load", {
        let page = page.clone();
        move |state| state.tabs.last().is_some_and(|tab| tab.url == page)
    });
    open(tab, "file:///C:/Windows/win.ini".to_string(), &mut rig);
    rig.ui.settle(Duration::from_secs(2));
    assert_eq!(
        rig.state("alpha").tabs.len(),
        2,
        "a refused address opens no window"
    );
    awake.end();
}

#[test]
fn every_permission_request_is_denied_and_the_person_is_told() {
    let mut rig = Rig::new();
    rig.open("alpha");
    rig.load("alpha", "blank.html");
    let answer = rig.eval(
        "alpha",
        "new Promise((done) => navigator.geolocation.getCurrentPosition(() => done('granted'), (error) => done('code ' + error.code)))",
    );
    assert_eq!(answer, "code 1", "geolocation must be denied");
    let notice = rig.wait_notice("geolocation");
    assert_eq!(
        notice,
        "Browser permission 'geolocation' is unavailable in Tjommie."
    );
}

#[test]
fn a_page_that_navigates_to_a_privileged_address_is_stopped_and_the_person_is_told() {
    let mut rig = Rig::new();
    rig.open("alpha");
    rig.load("alpha", "blank.html");
    let url = rig.server.url("blank.html");
    let awake = rig.begin("alpha");
    rig.eval(
        "alpha",
        "setTimeout(() => { location.href = 'ftp://127.0.0.1/privileged'; }, 0); 'scheduled'",
    );
    rig.wait_notice("unsupported browser address");
    rig.ui.settle(Duration::from_secs(1));
    assert_eq!(
        rig.eval("alpha", "location.href"),
        url,
        "the page stays where it was"
    );
    awake.end();
}

#[test]
fn a_deleted_or_switched_off_bot_closes_and_cannot_open_until_switched_on() {
    let mut rig = Rig::new();
    rig.open("alpha");
    let engine = rig.engine.clone();
    rig.ui.run(async move {
        engine
            .on_bot_event(BotEvent::Updated {
                id: "alpha".into(),
                browser: false,
            })
            .await
    });
    assert!(
        rig.engine.snapshot("alpha").is_none(),
        "switching the browser off closes it"
    );
    assert_eq!(
        rig.exec_error("alpha", json!({ "kind": "open" })),
        "This agent's browser is off or the agent no longer exists."
    );
    rig.engine.allow_bot("alpha");
    rig.exec_state("alpha", json!({ "kind": "open" }));
    let engine = rig.engine.clone();
    rig.ui.run(async move {
        engine
            .on_bot_event(BotEvent::Deleted { id: "alpha".into() })
            .await
    });
    assert!(
        rig.engine.snapshot("alpha").is_none(),
        "deleting the bot closes its browser"
    );
    assert!(
        rig.exec_error("alpha", json!({ "kind": "open" }))
            .contains("no longer exists")
    );
}

#[test]
fn the_bot_can_fix_a_hidden_browsers_size_and_a_panel_gives_it_back() {
    let mut rig = Rig::new();
    rig.open("alpha");
    rig.load("alpha", "blank.html");
    let (engine, bot) = (rig.engine.clone(), rig.bot("alpha"));
    let before = rig.state("alpha").viewport_revision;
    let resized = rig
        .ui
        .run(async move { engine.resize(&bot, 800, 600, None).await.expect("resize") });
    assert!(resized.viewport_revision > before);
    assert_eq!(
        rig.eval("alpha", "innerWidth + 'x' + innerHeight"),
        "800x600"
    );
    // Out-of-range sizes are refused with the TypeScript message.
    let (engine, bot) = (rig.engine.clone(), rig.bot("alpha"));
    let refused = rig
        .ui
        .run(async move { engine.resize(&bot, 100, 600, None).await });
    assert_eq!(
        refused.expect_err("too small").to_string(),
        "Browser dimensions must be 240\u{2013}2560 by 180\u{2013}1600."
    );
    // A panel shows the page at its own size, and the fixed size is released.
    rig.show("alpha");
    let size = loop_until_not(
        &mut rig,
        "alpha",
        "innerWidth + 'x' + innerHeight",
        "800x600",
    );
    assert!(size.contains('x'), "the page reports a size again: {size}");
}

fn loop_until_not(rig: &mut Rig, id: &str, expression: &str, unwanted: &str) -> String {
    for _ in 0..60 {
        let value = rig.eval(id, expression);
        if value != unwanted {
            return value;
        }
        rig.ui.settle(Duration::from_millis(250));
    }
    panic!("{expression} still {unwanted}");
}

#[test]
fn closing_the_panel_hands_the_browser_back_and_opening_another_moves_it() {
    let mut rig = Rig::new();
    let (engine, alpha, beta) = (rig.engine.clone(), rig.bot("alpha"), rig.bot("beta"));
    let panel = rig.panel();
    let state = rig.ui.run({
        let (engine, alpha) = (engine.clone(), alpha.clone());
        async move { engine.mount(&alpha, Some(panel)).await.expect("mount") }
    });
    assert_eq!(state.control, BrowserControl::Bot);
    take_control(&mut rig, "alpha");
    assert_eq!(rig.state("alpha").control, BrowserControl::Human);
    // Another bot's panel opens: the first is handed back to its bot.
    let panel = rig.panel();
    rig.ui.run({
        let (engine, beta) = (engine.clone(), beta.clone());
        async move { engine.mount(&beta, Some(panel)).await.expect("mount") }
    });
    assert_eq!(
        rig.state("alpha").control,
        BrowserControl::Bot,
        "the first bot got its browser back"
    );
    take_control(&mut rig, "beta");
    // The panel closes: the person's control ends with it.
    rig.ui.run({
        let (engine, beta) = (engine.clone(), beta.clone());
        async move { engine.mount(&beta, None).await.expect("unmount") }
    });
    assert_eq!(rig.state("beta").control, BrowserControl::Bot);
}

#[test]
fn a_download_is_saved_in_the_bots_own_browser_files_folder_and_the_person_is_told() {
    let mut rig = Rig::new();
    rig.open("alpha");
    rig.load("alpha", "blank.html");
    let awake = rig.begin("alpha");
    rig.eval(
        "alpha",
        "setTimeout(() => { location.href = '/download.txt'; }, 0); 'scheduled'",
    );
    let notice = rig.wait_notice("Downloaded report.txt");
    assert_eq!(
        notice,
        "Downloaded report.txt to the agent's browser-files folder."
    );
    let folder = rig
        .root
        .path()
        .join("bots")
        .join("alpha")
        .join("browser-files");
    let files: Vec<_> = std::fs::read_dir(&folder)
        .expect("browser-files")
        .flatten()
        .collect();
    assert_eq!(files.len(), 1, "one download, in the bot's own folder");
    let name = files[0].file_name().to_string_lossy().into_owned();
    assert!(
        name.ends_with("-report.txt"),
        "a unique name ending in the page's: {name}"
    );
    assert_eq!(
        std::fs::read_to_string(files[0].path()).expect("content"),
        "synthetic download"
    );
    awake.end();
}
