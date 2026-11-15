//! T3.3: the 26 browser tools over CDP, against the real WebView2 runtime, on loopback fixtures and a temp root. A bot's
//! tool call goes in as the daemon sends it (`{name, args}`) and comes out as the model reads it (text, images, an error
//! flag). The cases are those of `native-browser-automation.test.ts` and of `native-browser.real.test.ts` that are about
//! the tools.

// Test helpers may panic: a failure here is the test failing (the deny lints guard shipped code).
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use common::Rig;
use desktop_host::browser::tools::{Automation, ToolContent, ToolReply};
use protocol::native_browser::NativeBrowserToolCommand;
use serde_json::{Value, json};

struct Tools {
    rig: Rig,
    automation: Rc<Automation>,
}

impl Tools {
    fn new() -> Tools {
        let rig = Rig::new();
        let automation = Automation::new(rig.engine.clone());
        Tools { rig, automation }
    }

    /// The page, loaded and awake only while a tool runs on it, as in production.
    fn on_tools_page() -> Tools {
        let mut tools = Tools::new();
        tools.rig.open("alpha");
        tools.rig.load("alpha", "tools.html");
        tools
    }

    fn call(&mut self, name: &str, args: Value) -> ToolReply {
        let command: NativeBrowserToolCommand =
            serde_json::from_value(json!({ "name": name, "args": args }))
                .expect("a tool command the protocol accepts");
        let (automation, bot) = (self.automation.clone(), self.rig.bot("alpha"));
        self.rig
            .ui
            .run(async move { automation.run(&bot, &command).await })
    }

    /// A tool that must succeed; its words.
    fn ok(&mut self, name: &str, args: Value) -> String {
        let reply = self.call(name, args.clone());
        assert!(!reply.is_error, "{name} {args} failed: {}", reply.words());
        reply.words()
    }

    /// A tool that must fail; its words.
    fn refused(&mut self, name: &str, args: Value) -> String {
        let reply = self.call(name, args);
        assert!(
            reply.is_error,
            "{name} should have failed, but said: {}",
            reply.words()
        );
        reply.words()
    }

    /// The page's own words about itself.
    fn page(&mut self, expression: &str) -> String {
        self.rig.eval("alpha", expression)
    }

    /// The ref of the first snapshot line naming `label`.
    fn ref_of(&mut self, label: &str) -> String {
        let snapshot = self.ok("browser_snapshot", json!({}));
        let line = snapshot
            .lines()
            .find(|line| line.contains(&format!("\"{label}\"")))
            .unwrap_or_else(|| panic!("no {label:?} in:\n{snapshot}"));
        let after = line.split("[ref=").nth(1).expect("a ref");
        after.split(']').next().expect("a ref end").to_string()
    }

    /// Starts a tool and leaves it running: its answer comes through [`Tools::finish`].
    fn start(&mut self, name: &str, args: Value) -> Rc<RefCell<Option<ToolReply>>> {
        let command: NativeBrowserToolCommand =
            serde_json::from_value(json!({ "name": name, "args": args }))
                .expect("a tool command the protocol accepts");
        let answer = Rc::new(RefCell::new(None));
        let (automation, bot, slot) = (
            self.automation.clone(),
            self.rig.bot("alpha"),
            answer.clone(),
        );
        self.rig.ui.spawn(async move {
            let reply = automation.run(&bot, &command).await;
            *slot.borrow_mut() = Some(reply);
        });
        answer
    }

    /// The daemon's `cancel_tools` for the bot.
    fn cancel(&mut self) {
        let automation = self.automation.clone();
        self.rig.ui.run(async move { automation.cancel("alpha") });
    }

    /// The answer of a tool started with [`Tools::start`], once it has come.
    fn finish(&mut self, answer: &Rc<RefCell<Option<ToolReply>>>) -> ToolReply {
        let watching = answer.clone();
        self.rig
            .ui
            .wait_until("the tool to answer", Duration::from_secs(30), move || {
                watching.borrow().is_some()
            });
        answer.borrow_mut().take().expect("an answer")
    }

    fn wait_page(&mut self, what: &str, expression: &str, expected: &str) {
        let (mut waited, deadline) = (Duration::ZERO, Duration::from_secs(15));
        while self.page(expression) != expected {
            assert!(
                waited < deadline,
                "timed out waiting for {what}: the page says {:?}",
                self.page(expression)
            );
            self.rig.ui.settle(Duration::from_millis(200));
            waited += Duration::from_millis(200);
        }
    }
}

#[test]
fn a_snapshot_lists_the_page_with_stable_refs_and_never_a_protected_value() {
    let mut tools = Tools::on_tools_page();
    let first = tools.ok("browser_snapshot", json!({}));
    assert!(
        first.contains("URL: ") && first.contains("Title: \"Tools fixture\""),
        "{first}"
    );
    assert!(first.contains("button \"Save\""), "{first}");
    assert!(
        !first.contains("NEVER-EXPOSE"),
        "a password's value must never be shown"
    );
    let save = first
        .lines()
        .find(|l| l.contains("\"Save\""))
        .expect("save line");
    assert!(save.contains("[ref=n"), "{save}");
    // The same element keeps its ref across snapshots of the same document.
    let again = tools.ok("browser_snapshot", json!({}));
    let reference = save
        .split("[ref=")
        .nth(1)
        .and_then(|r| r.split(']').next())
        .expect("ref")
        .to_string();
    assert!(
        again.contains(&reference),
        "{reference} should be stable in:\n{again}"
    );
    // Narrowed to one element, and with its box.
    let narrow = tools.ok(
        "browser_snapshot",
        json!({ "ref": reference, "boxes": true }),
    );
    assert!(
        narrow.contains("\"Save\"") && narrow.contains("[box="),
        "{narrow}"
    );
    assert!(
        !narrow.contains("\"Name\""),
        "a target narrows the snapshot: {narrow}"
    );
}

#[test]
fn a_click_by_ref_or_css_target_acts_and_a_ref_goes_stale_after_navigation() {
    let mut tools = Tools::on_tools_page();
    let save = tools.ref_of("Save");
    let answer = tools.ok("browser_click", json!({ "ref": save }));
    assert!(
        answer.contains("button \"Save\""),
        "a click answers with a fresh snapshot: {answer}"
    );
    assert_eq!(tools.page("document.getElementById('clicks').value"), "1");
    tools.ok("browser_click", json!({ "target": "#save" }));
    assert_eq!(tools.page("document.getElementById('clicks').value"), "2");
    tools.ok(
        "browser_click",
        json!({ "target": "#save", "doubleClick": true }),
    );
    assert_eq!(tools.page("document.getElementById('clicks').value"), "4");
    tools.ok(
        "browser_click",
        json!({ "target": "#menu", "button": "right" }),
    );
    assert_eq!(
        tools.page("document.getElementById('last').value"),
        "context"
    );
    // The old document's refs are stale once the page navigates.
    tools.ok(
        "browser_navigate",
        json!({ "url": tools.rig.server.url("tools2.html") }),
    );
    let stale = tools.refused("browser_click", json!({ "ref": save }));
    assert!(
        stale.contains("stale") || stale.contains("another tab"),
        "{stale}"
    );
    assert!(
        tools
            .refused("browser_click", json!({ "target": "#save" }))
            .contains("exactly one"),
        "no such element on the new page"
    );
    assert!(
        tools
            .refused("browser_click", json!({}))
            .contains("required")
    );
}

#[test]
fn a_click_scrolls_a_far_element_into_view_and_lands_on_it() {
    let mut tools = Tools::on_tools_page();
    assert_eq!(tools.page("scrollY"), "0");
    tools.ok("browser_click", json!({ "target": "#far" }));
    assert_eq!(
        tools.page("document.getElementById('last').value"),
        "far",
        "the click reached the element"
    );
    assert!(
        tools.page("scrollY").parse::<f64>().expect("a number") > 500.0,
        "the page scrolled to it"
    );
    // And back up: an element above the scrolled-to region is reached the same way.
    tools.ok("browser_click", json!({ "target": "#save" }));
    assert_eq!(tools.page("document.getElementById('clicks').value"), "1");
}

#[test]
fn a_tool_refuses_while_the_person_holds_the_browser_and_sends_nothing() {
    let mut tools = Tools::on_tools_page();
    tools
        .rig
        .exec_state("alpha", json!({ "kind": "control", "control": "human" }));
    let words = tools.refused("browser_snapshot", json!({}));
    assert!(
        words.contains("person") || words.contains("control"),
        "{words}"
    );
    assert_eq!(tools.page("document.getElementById('clicks').value"), "0");
}

#[test]
fn typing_fills_clears_types_slowly_and_submits() {
    let mut tools = Tools::on_tools_page();
    tools.ok(
        "browser_type",
        json!({ "target": "#name", "text": "hello" }),
    );
    assert_eq!(tools.page("document.getElementById('name').value"), "hello");
    // Filling replaces what was there; an empty text clears it.
    tools.ok(
        "browser_type",
        json!({ "target": "#name", "text": "again" }),
    );
    assert_eq!(tools.page("document.getElementById('name').value"), "again");
    tools.ok("browser_type", json!({ "target": "#name", "text": "" }));
    assert_eq!(tools.page("document.getElementById('name').value"), "");
    // Slowly: key by key, then Enter.
    tools.ok(
        "browser_type",
        json!({ "target": "#name", "text": "ab c", "slowly": true, "submit": true }),
    );
    assert_eq!(tools.page("document.getElementById('name').value"), "ab c");
    assert_eq!(
        tools.page("document.getElementById('status').textContent"),
        "submitted:ab c"
    );
    assert!(
        tools
            .refused("browser_type", json!({ "target": "#save", "text": "x" }))
            .contains("not editable")
    );
    assert!(
        tools
            .refused(
                "browser_type",
                json!({ "target": "#name", "text": "x".repeat(2001), "slowly": true })
            )
            .contains("2000")
    );
}

#[test]
fn pressing_keys_sends_chords_including_a_plus_and_releases_them() {
    let mut tools = Tools::on_tools_page();
    tools.ok("browser_click", json!({ "target": "#notes" }));
    tools.ok("browser_press_key", json!({ "key": "a" }));
    assert_eq!(tools.page("document.getElementById('notes').value"), "a");
    tools.ok("browser_press_key", json!({ "key": "+" }));
    assert_eq!(tools.page("document.getElementById('notes').value"), "a+");
    tools.ok("browser_press_key", json!({ "key": "Control+A" }));
    assert_eq!(
        tools.page("document.getElementById('last').value"),
        "key:Ctrl+A"
    );
    tools.ok("browser_press_key", json!({ "key": "Backspace" }));
    assert_eq!(
        tools.page("document.getElementById('last').value"),
        "key:Backspace"
    );
    assert!(
        tools
            .refused("browser_press_key", json!({ "key": "Hyper+A" }))
            .contains("modifier")
    );
}

#[test]
fn select_fill_form_and_hover_work_on_native_controls() {
    let mut tools = Tools::on_tools_page();
    tools.ok(
        "browser_select_option",
        json!({ "target": "#size", "values": ["medium"] }),
    );
    assert_eq!(
        tools.page("document.getElementById('size').value"),
        "medium"
    );
    assert!(
        tools
            .refused(
                "browser_select_option",
                json!({ "target": "#size", "values": ["huge"] })
            )
            .contains("missing")
    );
    assert!(
        tools
            .refused(
                "browser_select_option",
                json!({ "target": "#name", "values": ["x"] })
            )
            .contains("native select")
    );
    tools.ok(
        "browser_fill_form",
        json!({ "fields": [
            { "name": "Name", "type": "textbox", "target": "#name", "value": "Synthetic" },
            { "name": "Agree", "type": "checkbox", "target": "#agree", "value": "true" },
            { "name": "Plan", "type": "radio", "target": "#small", "value": "true" },
            { "name": "Size", "type": "combobox", "target": "#size", "value": "Large" },
            { "name": "Range", "type": "slider", "target": "#range", "value": "5" },
        ] }),
    );
    let state = tools.page("pageState()");
    for part in [
        "\"name\":\"Synthetic\"",
        "\"agree\":true",
        "\"small\":true",
        "\"size\":\"large\"",
        "\"range\":\"5\"",
    ] {
        assert!(state.contains(part), "{part} in {state}");
    }
    assert!(tools.refused("browser_fill_form", json!({ "fields": [{ "name": "R", "type": "slider", "target": "#range", "value": "99" }] })).contains("outside its range"));
    tools.ok("browser_hover", json!({ "target": "#save" }));
    assert_eq!(
        tools.page("document.getElementById('status').textContent"),
        "hovered"
    );
}

#[test]
fn drag_moves_a_held_button_between_elements_and_drop_delivers_data_and_files() {
    let mut tools = Tools::on_tools_page();
    tools.ok(
        "browser_drag",
        json!({ "startTarget": "#from", "endTarget": "#to" }),
    );
    assert_eq!(
        tools.page("document.getElementById('dragged').value"),
        "up-on:to"
    );
    // Drop text data, and a file from the bot's own folder.
    let upload = tools
        .rig
        .root
        .path()
        .join("bots")
        .join("alpha")
        .join("dropped.txt");
    std::fs::create_dir_all(upload.parent().unwrap()).unwrap();
    std::fs::write(&upload, "local").unwrap();
    tools.ok("browser_drop", json!({ "target": "#zone", "data": { "text/plain": "local data" }, "paths": [upload.to_string_lossy()] }));
    assert_eq!(
        tools.page("document.getElementById('dropped').value"),
        "text=local data;files=dropped.txt"
    );
    let outside = tools.rig.root.path().join("outside.txt");
    std::fs::write(&outside, "secret").unwrap();
    assert!(
        tools
            .refused(
                "browser_drop",
                json!({ "target": "#zone", "paths": [outside.to_string_lossy()] })
            )
            .contains("inside this agent's folders")
    );
    assert!(
        tools
            .refused("browser_drop", json!({ "target": "#zone" }))
            .contains("Supply paths or MIME data")
    );
}

#[test]
fn evaluate_runs_page_javascript_with_or_without_an_element_and_can_save_its_output() {
    let mut tools = Tools::on_tools_page();
    assert_eq!(
        tools.ok("browser_evaluate", json!({ "function": "() => 6 * 7" })),
        "42"
    );
    assert_eq!(
        tools
            .ok(
                "browser_evaluate",
                json!({ "function": "async () => ({ a: [1, 2] })" })
            )
            .replace(char::is_whitespace, ""),
        r#"{"a":[1,2]}"#
    );
    assert_eq!(
        tools.ok(
            "browser_evaluate",
            json!({ "target": "#save", "function": "(element) => element.textContent" })
        ),
        "\"Save\""
    );
    assert_eq!(
        tools.ok("browser_evaluate", json!({ "function": "() => undefined" })),
        "null"
    );
    assert!(
        tools
            .refused(
                "browser_evaluate",
                json!({ "function": "() => { throw new Error('boom'); }" })
            )
            .contains("boom")
    );
    let saved = tools.ok(
        "browser_evaluate",
        json!({ "function": "() => 'to a file'", "filename": "out.txt" }),
    );
    assert!(
        saved.starts_with("Saved browser output: ") && saved.ends_with("out.txt"),
        "{saved}"
    );
    assert_eq!(
        std::fs::read_to_string(
            tools
                .rig
                .root
                .path()
                .join("bots")
                .join("alpha")
                .join("browser-files")
                .join("out.txt")
        )
        .unwrap(),
        "\"to a file\""
    );
    // No Node or desktop API is in the page.
    assert_eq!(
        tools.ok(
            "browser_evaluate",
            json!({ "function": "() => typeof require + typeof process" })
        ),
        "\"undefinedundefined\""
    );
}

#[test]
fn a_hung_evaluation_times_out_and_is_ended_and_a_dialog_interrupts_one() {
    let mut tools = Tools::on_tools_page();
    let started = std::time::Instant::now();
    let words = tools.refused(
        "browser_evaluate",
        json!({ "function": "() => slowForever()" }),
    );
    assert!(words.contains("timed out"), "{words}");
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "bounded to about 10 s, took {:?}",
        started.elapsed()
    );
    // The page is usable again afterwards.
    assert_eq!(
        tools.ok("browser_evaluate", json!({ "function": "() => 1 + 1" })),
        "2"
    );
    // A dialog opened by the page ends a waiting tool and names the way out.
    let words = tools.refused(
        "browser_evaluate",
        json!({ "function": "() => { alert('stop'); return 1; }" }),
    );
    assert!(words.contains("dialog"), "{words}");
    let dialog = tools.ok("browser_handle_dialog", json!({ "accept": true }));
    // The answer is the page after the dialog: it no longer lists a dialog to handle.
    assert!(
        !dialog.is_empty() && !dialog.contains("Page dialog"),
        "the dialog is closed: {dialog}"
    );
    assert_eq!(
        tools.ok("browser_evaluate", json!({ "function": "() => 1 + 2" })),
        "3"
    );
}

#[test]
fn navigation_tools_load_go_back_and_forward_reload_and_manage_tabs() {
    let mut tools = Tools::on_tools_page();
    let second = tools.rig.server.url("tools2.html");
    let loaded = tools.ok("browser_navigate", json!({ "url": second }));
    assert!(
        loaded.contains("Title: \"Second page\""),
        "a navigation answers with the new page: {loaded}"
    );
    assert!(
        tools
            .refused(
                "browser_navigate",
                json!({ "url": "file:///C:/Windows/win.ini" })
            )
            .contains("only supports HTTP")
    );
    assert!(
        tools
            .ok("browser_navigate_back", json!({}))
            .contains("Title: \"Tools fixture\"")
    );
    assert!(
        tools
            .ok("browser_navigate_forward", json!({}))
            .contains("Title: \"Second page\"")
    );
    assert!(
        tools
            .ok("browser_reload", json!({}))
            .contains("Title: \"Second page\"")
    );
    // Tabs: list, open one at an address, switch, close.
    let listed = tools.ok("browser_tabs", json!({ "action": "list" }));
    assert!(listed.contains("0: (current) \"Second page\""), "{listed}");
    let first = tools.rig.server.url("tools.html");
    let opened = tools.ok("browser_tabs", json!({ "action": "new", "url": first }));
    assert!(opened.contains("Title: \"Tools fixture\""), "{opened}");
    let listed = tools.ok("browser_tabs", json!({ "action": "list" }));
    assert!(listed.contains("1: (current)"), "{listed}");
    let selected = tools.ok("browser_tabs", json!({ "action": "select", "index": 0 }));
    assert!(selected.contains("0: (current)"), "{selected}");
    assert!(
        tools
            .refused("browser_tabs", json!({ "action": "select" }))
            .contains("tab index")
    );
    assert!(
        tools
            .refused("browser_tabs", json!({ "action": "close", "index": 9 }))
            .contains("existing tab")
    );
    tools.ok("browser_tabs", json!({ "action": "close", "index": 1 }));
    assert!(
        !tools
            .ok("browser_tabs", json!({ "action": "list" }))
            .contains("- 1:")
    );
    let closed = tools.ok("browser_close", json!({}));
    assert!(closed.contains("No open tabs"), "{closed}");
}

#[test]
fn a_dialog_is_reported_in_snapshots_and_handled_by_browser_handle_dialog() {
    let mut tools = Tools::on_tools_page();
    tools.ok("browser_click", json!({ "target": "#alert" }));
    tools.rig.ui.settle(Duration::from_millis(500));
    let snapshot = tools.ok("browser_snapshot", json!({}));
    assert!(
        snapshot.contains("Page dialog (alert): a dialog")
            && snapshot.contains("browser_handle_dialog"),
        "{snapshot}"
    );
    tools.ok("browser_handle_dialog", json!({ "accept": true }));
    assert!(
        tools
            .refused("browser_handle_dialog", json!({ "accept": true }))
            .contains("No page dialog")
    );
    assert!(
        tools
            .ok("browser_snapshot", json!({}))
            .contains("Tools fixture")
    );
}

#[test]
fn the_person_gets_the_native_file_picker_back_when_they_take_control() {
    let mut tools = Tools::on_tools_page();
    let intercepting = |tools: &mut Tools| {
        let guard = tools.rig.begin("alpha");
        let on = guard.tab().expect("a tab").intercepts_file_chooser();
        guard.end();
        on
    };
    assert!(
        !intercepting(&mut tools),
        "nothing is intercepted before a tool runs"
    );
    tools.ok("browser_snapshot", json!({}));
    assert!(
        intercepting(&mut tools),
        "a bot's tool has file choosers report to it"
    );
    tools
        .rig
        .exec_state("alpha", json!({ "kind": "control", "control": "human" }));
    assert!(
        !intercepting(&mut tools),
        "the person's click opens the native picker again"
    );
    tools
        .rig
        .exec_state("alpha", json!({ "kind": "control", "control": "bot" }));
    tools.ok("browser_snapshot", json!({}));
    assert!(
        intercepting(&mut tools),
        "and the bot's next tool turns it back on"
    );
}

#[test]
fn a_dialog_that_opens_while_the_answering_snapshot_waits_is_reported_not_a_failure() {
    let mut tools = Tools::on_tools_page();
    // The page is busy for 300 ms and then opens a dialog: the snapshot that answers the click is already waiting on it.
    let words = tools.ok("browser_click", json!({ "target": "#busyalert" }));
    assert!(
        words.contains("Page dialog (alert): late dialog")
            && words.contains("browser_handle_dialog"),
        "{words}"
    );
    tools.ok("browser_handle_dialog", json!({ "accept": true }));
}

#[test]
fn a_click_that_blocks_the_page_on_a_dialog_still_succeeds_and_reports_it() {
    let mut tools = Tools::on_tools_page();
    // The handler opens the dialog while the click is still being delivered: the page answers nothing until it is closed.
    let words = tools.ok("browser_click", json!({ "target": "#alertnow" }));
    assert!(
        words.contains("Page dialog (alert): sync dialog")
            && words.contains("browser_handle_dialog"),
        "{words}"
    );
    // Tools that need the page say so, and name the way out.
    assert!(
        tools
            .refused("browser_click", json!({ "target": "#save" }))
            .contains("dialog")
    );
    tools.ok("browser_handle_dialog", json!({ "accept": true }));
    tools.ok("browser_click", json!({ "target": "#save" }));
    assert_eq!(tools.page("document.getElementById('clicks').value"), "1");
}

#[test]
fn console_and_network_observations_are_bounded_redacted_and_only_from_the_bots_turn() {
    let mut tools = Tools::on_tools_page();
    tools.ok("browser_snapshot", json!({})); // the first tool turns observation on
    tools.ok("browser_click", json!({ "target": "#log" }));
    tools.ok("browser_click", json!({ "target": "#fetch" }));
    tools.wait_page(
        "the fetch",
        "document.getElementById('status').textContent",
        "fetched",
    );
    let console = tools.ok("browser_console_messages", json!({ "level": "debug" }));
    assert!(
        console.contains("[log] logged-message") && console.contains("[error] error-message"),
        "{console}"
    );
    let only_errors = tools.ok("browser_console_messages", json!({ "level": "error" }));
    assert!(
        only_errors.contains("error-message") && !only_errors.contains("logged-message"),
        "{only_errors}"
    );
    let requests = tools.ok("browser_network_requests", json!({}));
    assert!(
        requests.contains("GET") && requests.contains("fetched=1") && requests.contains("=> 200"),
        "{requests}"
    );
    let index: u32 = requests
        .lines()
        .find(|line| line.contains("fetched=1"))
        .and_then(|line| line.split('.').next())
        .and_then(|number| number.trim().parse().ok())
        .expect("the fetch's index");
    let detail = tools.ok("browser_network_request", json!({ "index": index }));
    assert!(
        detail.contains("[redacted]")
            && detail.contains("text/html")
            && !detail.contains("SECRET-TOKEN"),
        "{detail}"
    );
    let body = tools.ok(
        "browser_network_request",
        json!({ "index": index, "part": "response-body" }),
    );
    assert!(body.contains("Second page"), "{body}");
    assert!(
        tools
            .ok(
                "browser_network_request",
                json!({ "index": index, "part": "request-body" })
            )
            .contains("not retained")
    );
    assert!(
        tools
            .ok("browser_network_requests", json!({ "filter": "/nomatch/" }))
            .contains("No matching")
    );
    assert!(
        tools
            .refused("browser_network_requests", json!({ "filter": "(a)(b)" }))
            .contains("simple regular expression")
    );
    assert!(
        tools
            .refused("browser_network_request", json!({ "index": 9999 }))
            .contains("no longer in the bounded")
    );
    // Console output while the person held the browser is not kept for the bot.
    tools
        .rig
        .exec_state("alpha", json!({ "kind": "control", "control": "human" }));
    tools.page("console.log('human-secret'); 'ok'");
    tools
        .rig
        .exec_state("alpha", json!({ "kind": "control", "control": "bot" }));
    let after = tools.ok(
        "browser_console_messages",
        json!({ "level": "debug", "all": true }),
    );
    assert!(!after.contains("human-secret"), "{after}");
}

#[test]
fn a_file_chooser_the_bot_opened_takes_files_from_its_own_folders_only() {
    let mut tools = Tools::on_tools_page();
    let home = tools.rig.root.path().join("bots").join("alpha");
    std::fs::create_dir_all(&home).unwrap();
    let allowed = home.join("upload.txt");
    std::fs::write(&allowed, "local").unwrap();
    let denied = tools.rig.root.path().join("secret.txt");
    std::fs::write(&denied, "outside").unwrap();
    assert!(
        tools
            .refused(
                "browser_file_upload",
                json!({ "paths": [allowed.to_string_lossy()] })
            )
            .contains("No bot-opened file chooser")
    );
    tools.ok("browser_click", json!({ "target": "#upload" }));
    tools.rig.ui.settle(Duration::from_millis(500));
    assert!(
        tools
            .refused(
                "browser_file_upload",
                json!({ "paths": [denied.to_string_lossy()] })
            )
            .contains("inside this agent's folders")
    );
    tools.ok(
        "browser_file_upload",
        json!({ "paths": [allowed.to_string_lossy()] }),
    );
    assert_eq!(
        tools.page("document.getElementById('uploaded').value"),
        "upload.txt:5"
    );
    assert!(
        tools
            .refused(
                "browser_file_upload",
                json!({ "paths": [allowed.to_string_lossy()] })
            )
            .contains("No bot-opened"),
        "the chooser is used up"
    );
    // Cancelling: no paths.
    tools.ok("browser_click", json!({ "target": "#upload" }));
    tools.rig.ui.settle(Duration::from_millis(500));
    tools.ok("browser_file_upload", json!({}));
}

#[test]
fn waiting_and_finding_use_the_page_and_its_snapshot() {
    let mut tools = Tools::on_tools_page();
    tools.ok("browser_click", json!({ "target": "#later" }));
    tools.ok("browser_wait_for", json!({ "text": "later-arrived" }));
    tools.ok(
        "browser_wait_for",
        json!({ "textGone": "no such text anywhere" }),
    );
    let started = std::time::Instant::now();
    tools.ok("browser_wait_for", json!({ "time": 0.3 }));
    assert!(started.elapsed() >= Duration::from_millis(300));
    assert!(
        tools
            .refused("browser_wait_for", json!({ "text": "never shows up" }))
            .contains("did not become true")
    );
    let found = tools.ok("browser_find", json!({ "text": "SAVE" }));
    assert!(found.contains("button \"Save\""), "{found}");
    let regex = tools.ok("browser_find", json!({ "regex": "/Dou.le/i" }));
    assert!(regex.contains("Double"), "{regex}");
    assert!(
        tools
            .ok("browser_find", json!({ "text": "nothing like this" }))
            .contains("No matches")
    );
    assert!(
        tools
            .refused("browser_find", json!({ "regex": "(a)" }))
            .contains("simple regular expression")
    );
}

fn image_of(reply: &ToolReply) -> String {
    reply
        .content
        .iter()
        .find_map(|item| match item {
            ToolContent::Image { data, .. } => Some(data.clone()),
            ToolContent::Text { .. } => None,
        })
        .unwrap_or_else(|| panic!("an image in the reply, which said: {}", reply.words()))
}

#[test]
fn screenshots_are_bounded_typed_and_saved_only_as_new_plain_files() {
    let mut tools = Tools::on_tools_page();
    let reply = tools.call("browser_take_screenshot", json!({}));
    assert!(!reply.is_error, "{}", reply.words());
    assert!(image_of(&reply).starts_with("iVBOR"), "a PNG by default");
    for (kind, magic) in [("jpeg", "/9j/"), ("webp", "UklGR")] {
        let reply = tools.call("browser_take_screenshot", json!({ "type": kind }));
        assert!(!reply.is_error, "{kind}: {}", reply.words());
        let data = image_of(&reply);
        assert!(
            data.starts_with(magic),
            "{kind}: {}",
            data.chars().take(8).collect::<String>()
        );
    }
    let save = tools.ref_of("Save");
    tools.ok("browser_take_screenshot", json!({ "ref": save }));
    tools.ok(
        "browser_take_screenshot",
        json!({ "fullPage": true, "type": "jpeg", "filename": "page.jpg" }),
    );
    let saved = tools
        .rig
        .root
        .path()
        .join("bots")
        .join("alpha")
        .join("browser-files")
        .join("page.jpg");
    assert!(std::fs::metadata(&saved).unwrap().len() > 100);
    assert!(
        tools
            .refused(
                "browser_take_screenshot",
                json!({ "filename": "../escape.png" })
            )
            .contains("plain filename")
    );
    assert!(
        tools
            .refused("browser_take_screenshot", json!({ "filename": "page.jpg" }))
            .contains("Could not save"),
        "an existing file is never overwritten"
    );
    assert!(
        tools
            .refused(
                "browser_take_screenshot",
                json!({ "fullPage": true, "target": "#save" })
            )
            .contains("not both")
    );
}

#[test]
fn an_element_screenshot_is_taken_where_the_element_is_after_it_scrolls_into_view() {
    let mut tools = Tools::on_tools_page();
    tools.page("scrollTo(0, 0); 'top'");
    // The element is far below the fold: taking it scrolls it into view first.
    let first = image_of(&tools.call("browser_take_screenshot", json!({ "target": "#far" })));
    assert!(
        tools.page("scrollY").parse::<f64>().expect("a number") > 500.0,
        "the element was scrolled into view"
    );
    // Already in view, the same element is the same picture; a clip measured before the scroll would differ.
    let second = image_of(&tools.call("browser_take_screenshot", json!({ "target": "#far" })));
    assert_eq!(first, second);
}

#[test]
fn resize_and_emulated_media_hold_across_calls_and_reset_with_null() {
    let mut tools = Tools::on_tools_page();
    let words = tools.ok("browser_resize", json!({ "width": 800, "height": 600 }));
    assert!(
        words.contains("Tools fixture") || words.contains("(current)"),
        "{words}"
    );
    assert_eq!(tools.page("innerWidth + 'x' + innerHeight"), "800x600");
    let dark = "matchMedia('(prefers-color-scheme: dark)').matches";
    let both = "matchMedia('(prefers-color-scheme: dark)').matches + '|' + matchMedia('(prefers-reduced-motion: reduce)').matches";
    // What the PC itself prefers is the baseline a reset goes back to (it may well be dark).
    let own = tools.page(dark);
    tools.ok(
        "browser_emulate_media",
        json!({ "colorScheme": "dark", "reducedMotion": "reduce" }),
    );
    assert_eq!(tools.page(both), "true|true");
    tools.ok("browser_emulate_media", json!({ "colorScheme": "light" }));
    assert_eq!(
        tools.page(both),
        "false|true",
        "a later call that sets one keeps the other"
    );
    // null resets one override to the PC's own preference and still keeps the other.
    tools.ok("browser_emulate_media", json!({ "colorScheme": null }));
    assert_eq!(tools.page(both), format!("{own}|true"));
    tools.ok("browser_emulate_media", json!({ "media": "print" }));
    assert_eq!(tools.page("matchMedia('print').matches"), "true");
    tools.ok(
        "browser_emulate_media",
        json!({ "media": null, "reducedMotion": null }),
    );
    assert_eq!(tools.page("matchMedia('print').matches"), "false");
}

#[test]
fn an_element_that_is_covered_is_refused_instead_of_clicked_through() {
    let mut tools = Tools::on_tools_page();
    tools.page("cover(true); 'covered'");
    let words = tools.refused("browser_click", json!({ "target": "#save" }));
    assert!(words.contains("covers"), "{words}");
    assert_eq!(
        tools.page("document.getElementById('clicks').value"),
        "0",
        "nothing was clicked"
    );
    tools.page("cover(false); 'open'");
    tools.ok("browser_click", json!({ "target": "#save" }));
    assert_eq!(tools.page("document.getElementById('clicks').value"), "1");
}

#[test]
fn cancelling_a_running_tool_stops_it_before_its_next_step() {
    let mut tools = Tools::on_tools_page();
    let waiting = tools.start("browser_wait_for", json!({ "time": 15 }));
    tools.rig.ui.settle(Duration::from_millis(600));
    assert!(waiting.borrow().is_none(), "the tool is still waiting");
    tools.cancel();
    let reply = tools.finish(&waiting);
    assert!(
        reply.is_error && reply.words().contains("cancelled"),
        "{}",
        reply.words()
    );
    // The shared page is untouched, and the next tool works.
    assert!(!tools.call("browser_snapshot", json!({})).is_error);
}

#[test]
fn cancelling_leaves_an_open_dialog_for_handle_dialog() {
    let mut tools = Tools::on_tools_page();
    tools.ok("browser_click", json!({ "target": "#alert" }));
    tools.rig.ui.settle(Duration::from_millis(500));
    let waiting = tools.start("browser_wait_for", json!({ "time": 15 }));
    tools.rig.ui.settle(Duration::from_millis(300));
    tools.cancel();
    assert!(tools.finish(&waiting).words().contains("cancelled"));
    assert!(
        tools
            .ok("browser_snapshot", json!({}))
            .contains("Page dialog (alert)"),
        "the dialog is still open"
    );
    tools.ok("browser_handle_dialog", json!({ "accept": true }));
}

#[test]
fn a_queued_navigation_is_cancelled_before_it_runs() {
    let mut tools = Tools::on_tools_page();
    let other = tools.rig.server.url("tools2.html");
    // The first tool holds the bot's turn; the navigation waits behind it.
    let holding = tools.start("browser_wait_for", json!({ "time": 15 }));
    tools.rig.ui.settle(Duration::from_millis(300));
    let queued = tools.start("browser_navigate", json!({ "url": other }));
    tools.rig.ui.settle(Duration::from_millis(300));
    assert!(queued.borrow().is_none(), "the navigation is queued");
    tools.cancel();
    let navigation = tools.finish(&queued);
    assert!(
        navigation.is_error && navigation.words().contains("cancelled"),
        "{}",
        navigation.words()
    );
    tools.finish(&holding);
    assert!(
        tools.page("location.pathname").ends_with("tools.html"),
        "the page never navigated: {}",
        tools.page("location.href")
    );
}

#[test]
fn a_drag_in_progress_releases_the_button_when_the_person_takes_control() {
    let mut tools = Tools::on_tools_page();
    let drag = tools.start(
        "browser_drag",
        json!({ "startTarget": "#from", "endTarget": "#to" }),
    );
    // Wait until the button is down and the pointer moving, then take the browser.
    let mut waited = Duration::ZERO;
    while !tools
        .page("document.getElementById('dragged').value")
        .starts_with("move:")
    {
        assert!(waited < Duration::from_secs(10), "the drag never began");
        tools.rig.ui.settle(Duration::from_millis(5));
        waited += Duration::from_millis(5);
    }
    tools
        .rig
        .exec_state("alpha", json!({ "kind": "control", "control": "human" }));
    tools.finish(&drag);
    let seen = tools.page("document.getElementById('dragged').value");
    assert!(
        seen.starts_with("up-on:"),
        "the page saw the button go up, not stay held: {seen}"
    );
}

#[test]
fn a_person_taking_control_discards_what_the_bot_knew_of_the_page() {
    let mut tools = Tools::on_tools_page();
    let save = tools.ref_of("Save");
    tools
        .rig
        .exec_state("alpha", json!({ "kind": "control", "control": "human" }));
    tools
        .rig
        .exec_state("alpha", json!({ "kind": "control", "control": "bot" }));
    let words = tools.refused("browser_click", json!({ "ref": save }));
    assert!(
        words.contains("stale") || words.contains("another tab"),
        "refs do not survive a person's turn: {words}"
    );
}

#[test]
fn an_unknown_tool_or_bad_arguments_are_refused_in_words() {
    let mut tools = Tools::on_tools_page();
    assert!(
        tools
            .refused("browser_run_code", json!({}))
            .contains("Unsupported")
    );
    assert!(
        tools
            .refused("browser_click", json!({ "ref": "x", "shell": "bad" }))
            .contains("Invalid arguments")
    );
}
