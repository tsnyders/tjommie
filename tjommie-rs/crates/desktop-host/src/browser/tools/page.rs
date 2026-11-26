//! What the tools remember about a tab between calls (`PageState` in `native-browser-automation.ts`): the element refs
//! of the last snapshot, the console and network log while the bot had the browser, a pending file chooser, and the media
//! overrides. Events reach it as text from WebView2 callbacks and are applied here, on the tool's own turn.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use async_channel::{Receiver, Sender};

use super::protocol::{
    ConsoleApiCalled, ExceptionThrown, FileChooserOpened, Headers, RequestWillBeSent,
    ResponseReceived, Scalar,
};
use super::reply::{ToolError, bounded};
use crate::browser::engine::TabHandle;

const CONSOLE_KEPT: usize = 200;
const REQUESTS_KEPT: usize = 200;
/// Events waiting for the next tool call; a page that logs faster than that loses the newest the inbox could not hold.
const INBOX: usize = 2048;

#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub level: String,
    pub text: String,
    /// The document it was seen in; after a navigation only the new document's are listed by default.
    pub document: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub id: String,
    pub index: u32,
    pub url: String,
    pub method: String,
    pub kind: String,
    pub status: Option<u32>,
    pub request_headers: Vec<(String, String)>,
    pub response_headers: Option<Vec<(String, String)>>,
}

#[derive(Debug, Default)]
pub struct PageState {
    pub document: String,
    pub next_ref: u32,
    /// Ref to backend node id, and back.
    pub refs: HashMap<String, i64>,
    pub reverse: HashMap<i64, String>,
    pub console: VecDeque<Observation>,
    pub requests: VecDeque<Request>,
    pub next_request: u32,
    /// The input a bot-opened file chooser is waiting on.
    pub file_input: Option<i64>,
    /// The media overrides set so far (`prefers-color-scheme` and the like), kept across calls.
    pub media: Vec<(String, String)>,
    pub media_type: Option<String>,
    /// The control and private epochs this state was last known good under: a person's turn at the browser, or a
    /// private interaction, ends it.
    pub epoch: Option<(u64, u64)>,
}

impl PageState {
    fn fresh() -> Self {
        Self {
            document: new_document(),
            ..Self::default()
        }
    }

    /// A new document: refs, requests and a pending chooser belong to the old one.
    pub fn reset_document(&mut self) {
        self.document = new_document();
        self.next_ref = 0;
        self.refs.clear();
        self.reverse.clear();
        self.file_input = None;
        self.requests.clear();
        self.next_request = 0;
    }
}

fn new_document() -> String {
    uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(8)
        .collect()
}

/// Credential-bearing header names are shown as `[redacted]`; at most 100 headers, each value at most 2000 characters.
pub fn safe_headers(headers: &Headers) -> Vec<(String, String)> {
    headers
        .0
        .iter()
        .take(100)
        .map(|(name, value)| {
            let lower = name.to_lowercase();
            let secret = [
                "authorization",
                "cookie",
                "token",
                "secret",
                "api-key",
                "api_key",
                "apikey",
            ]
            .iter()
            .any(|key| lower.contains(key));
            (
                name.clone(),
                if secret {
                    "[redacted]".to_string()
                } else {
                    bounded(value, 2000)
                },
            )
        })
        .collect()
}

enum Raw {
    Event(&'static str, String),
    NewDocument,
}

/// One tab as the tools see it. Created the first time a tool needs the tab.
pub struct Page {
    pub state: RefCell<PageState>,
    inbox: Receiver<Raw>,
    /// The DevTools domains are enabled.
    pub ready: Cell<bool>,
}

/// The DevTools events the tools watch.
const EVENTS: [&str; 6] = [
    "Runtime.consoleAPICalled",
    "Runtime.exceptionThrown",
    "Network.requestWillBeSent",
    "Network.responseReceived",
    "Page.fileChooserOpened",
    "DOM.documentUpdated",
];

impl Page {
    /// Starts watching `tab`. The handlers only hand text to the inbox; nothing is decoded in a callback.
    pub fn attach(tab: &TabHandle) -> Result<Rc<Page>, ToolError> {
        let (sender, inbox): (Sender<Raw>, Receiver<Raw>) = async_channel::bounded(INBOX);
        let events = sender.clone();
        tab.subscribe(
            &EVENTS,
            Rc::new(move |event, json| {
                let _ = events.try_send(Raw::Event(event, json));
            }),
        )?;
        tab.on_new_document(move || {
            let _ = sender.try_send(Raw::NewDocument);
        })?;
        Ok(Rc::new(Page {
            state: RefCell::new(PageState::fresh()),
            inbox,
            ready: Cell::new(false),
        }))
    }

    /// Notes the epochs a call runs under. If they changed since the last call, a person (or a private interaction) had
    /// the browser: what the bot knew of the page (refs, log, requests) is dropped, and the DevTools domains are
    /// enabled afresh, so a console replay is discarded.
    pub fn sync(&self, epoch: (u64, u64)) {
        let mut state = self.state.borrow_mut();
        match state.epoch {
            Some(known) if known == epoch => {}
            known => {
                state.epoch = Some(epoch);
                if known.is_some() {
                    while self.inbox.try_recv().is_ok() {}
                    state.reset_document();
                    state.console.clear();
                    state.file_input = None;
                    self.ready.set(false);
                }
            }
        }
    }

    /// Applies what the page reported since the last look. `observing` is false while a person holds the browser (their
    /// sign-in traffic and console output are not kept for the bot).
    pub fn drain(&self, observing: bool) {
        while let Ok(raw) = self.inbox.try_recv() {
            let mut state = self.state.borrow_mut();
            match raw {
                Raw::NewDocument | Raw::Event("DOM.documentUpdated", _) => state.reset_document(),
                Raw::Event(_, _) if !observing => {}
                Raw::Event(event, json) => apply(&mut state, event, &json),
            }
        }
    }
}

fn apply(state: &mut PageState, event: &str, json: &str) {
    let document = state.document.clone();
    match event {
        "Runtime.consoleAPICalled" => {
            let Ok(call) = serde_json::from_str::<ConsoleApiCalled>(json) else {
                return;
            };
            let text = call
                .args
                .iter()
                .map(|arg| {
                    arg.description
                        .clone()
                        .filter(|d| !d.is_empty())
                        .unwrap_or_else(|| match &arg.value {
                            Some(Scalar::Text(text)) => text.clone(),
                            Some(Scalar::Bool(flag)) => flag.to_string(),
                            Some(Scalar::Number(number)) => number.to_string(),
                            _ => String::new(),
                        })
                })
                .collect::<Vec<_>>()
                .join(" ");
            push_console(
                state,
                Observation {
                    level: call.kind,
                    text: bounded(&text, 4000),
                    document,
                },
            );
        }
        "Runtime.exceptionThrown" => {
            let Ok(thrown) = serde_json::from_str::<ExceptionThrown>(json) else {
                return;
            };
            push_console(
                state,
                Observation {
                    level: "error".into(),
                    text: bounded(&thrown.exception_details.text, 4000),
                    document,
                },
            );
        }
        "Network.requestWillBeSent" => {
            let Ok(sent) = serde_json::from_str::<RequestWillBeSent>(json) else {
                return;
            };
            state.next_request += 1;
            let request = Request {
                id: sent.request_id,
                index: state.next_request,
                url: bounded(&sent.request.url, 8192),
                method: sent.request.method,
                kind: sent.kind.unwrap_or_default(),
                status: None,
                request_headers: safe_headers(&sent.request.headers),
                response_headers: None,
            };
            state.requests.retain(|old| old.id != request.id);
            state.requests.push_back(request);
            while state.requests.len() > REQUESTS_KEPT {
                state.requests.pop_front();
            }
        }
        "Network.responseReceived" => {
            let Ok(received) = serde_json::from_str::<ResponseReceived>(json) else {
                return;
            };
            if let Some(request) = state
                .requests
                .iter_mut()
                .find(|r| r.id == received.request_id)
            {
                request.status = received.response.status.map(|status| status as u32);
                request.response_headers = Some(safe_headers(&received.response.headers));
            }
        }
        "Page.fileChooserOpened" => {
            if let Ok(FileChooserOpened {
                backend_node_id: Some(node),
            }) = serde_json::from_str(json)
            {
                state.file_input = Some(node);
            }
        }
        _ => {}
    }
}

fn push_console(state: &mut PageState, observation: Observation) {
    state.console.push_back(observation);
    while state.console.len() > CONSOLE_KEPT {
        state.console.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hostile() -> Headers {
        Headers(vec![
            ("Authorization".into(), "Bearer SECRET".into()),
            ("X-Api-Key".into(), "key".into()),
            ("Set-Cookie".into(), "a=b".into()),
            ("Accept".into(), "text/html".into()),
            ("X-Long".into(), "y".repeat(3000)),
        ])
    }

    #[test]
    fn credential_headers_are_redacted_and_long_values_cut() {
        let shown = safe_headers(&hostile());
        assert_eq!(shown.first().map(|h| h.1.as_str()), Some("[redacted]"));
        assert_eq!(shown.get(1).map(|h| h.1.as_str()), Some("[redacted]"));
        assert_eq!(shown.get(2).map(|h| h.1.as_str()), Some("[redacted]"));
        assert_eq!(shown.get(3).map(|h| h.1.as_str()), Some("text/html"));
        assert!(
            shown
                .get(4)
                .is_some_and(|h| h.1.ends_with("[truncated]") && h.1.chars().count() < 2100)
        );
        let many = Headers((0..150).map(|n| (format!("h{n}"), "v".into())).collect());
        assert_eq!(safe_headers(&many).len(), 100);
    }

    #[test]
    fn the_log_is_bounded_and_a_new_document_forgets_requests_but_not_the_console() {
        let mut state = PageState::fresh();
        for n in 0..250 {
            apply(
                &mut state,
                "Runtime.consoleAPICalled",
                &format!(r#"{{"type":"log","args":[{{"value":"m{n}"}}]}}"#),
            );
            apply(
                &mut state,
                "Network.requestWillBeSent",
                &format!(
                    r#"{{"requestId":"r{n}","type":"Fetch","request":{{"url":"http://x/{n}","method":"GET","headers":{{}}}}}}"#
                ),
            );
        }
        assert_eq!(state.console.len(), 200);
        assert_eq!(state.requests.len(), 200);
        assert_eq!(
            state.console.front().map(|o| o.text.as_str()),
            Some("m50"),
            "the oldest 50 were dropped"
        );
        apply(
            &mut state,
            "Network.responseReceived",
            r#"{"requestId":"r249","response":{"status":404,"headers":{"A":"b"}}}"#,
        );
        assert_eq!(state.requests.back().and_then(|r| r.status), Some(404));
        let old = state.document.clone();
        state.reset_document();
        assert!(state.requests.is_empty() && state.refs.is_empty());
        assert_eq!(
            state.console.len(),
            200,
            "the console keeps entries, each marked with its document"
        );
        assert_ne!(state.document, old);
    }

    #[test]
    fn events_that_do_not_decode_are_ignored_not_fatal() {
        let mut state = PageState::fresh();
        apply(&mut state, "Runtime.consoleAPICalled", "not json");
        apply(&mut state, "Network.responseReceived", "{}");
        apply(
            &mut state,
            "Page.fileChooserOpened",
            r#"{"mode":"selectSingle"}"#,
        );
        assert!(state.console.is_empty() && state.file_input.is_none());
        apply(
            &mut state,
            "Page.fileChooserOpened",
            r#"{"backendNodeId":7}"#,
        );
        assert_eq!(state.file_input, Some(7));
    }

    #[test]
    fn a_changed_epoch_drops_what_the_bot_knew_and_the_same_one_keeps_it() {
        let (sender, inbox) = async_channel::bounded(8);
        let page = Page {
            state: RefCell::new(PageState::fresh()),
            inbox,
            ready: Cell::new(true),
        };
        page.sync((1, 1));
        {
            let mut state = page.state.borrow_mut();
            state.refs.insert("n00000000_1".into(), 7);
            state.reverse.insert(7, "n00000000_1".into());
            state.file_input = Some(3);
            state
                .media
                .push(("prefers-color-scheme".into(), "dark".into()));
            state.console.push_back(Observation {
                level: "log".into(),
                text: "seen".into(),
                document: "d".into(),
            });
        }
        let _ = sender.try_send(Raw::Event("Runtime.consoleAPICalled", "{}".into()));
        page.sync((1, 1));
        assert!(
            page.ready.get() && !page.state.borrow().refs.is_empty(),
            "the same epochs keep everything"
        );

        // A person had the browser: refs, the log, a pending chooser and queued events go, and the domains are enabled
        // again (a console replay is then discarded). What the bot set on the page, such as media, stays.
        page.sync((2, 1));
        assert!(!page.ready.get());
        {
            let state = page.state.borrow();
            assert!(state.refs.is_empty() && state.reverse.is_empty() && state.console.is_empty());
            assert_eq!(state.file_input, None);
            assert_eq!(state.media.len(), 1);
        }
        assert!(
            page.inbox.try_recv().is_err(),
            "events from the person's turn are not kept"
        );

        // A private interaction is the same.
        page.ready.set(true);
        page.state.borrow_mut().refs.insert("n00000000_2".into(), 8);
        page.sync((2, 2));
        assert!(!page.ready.get() && page.state.borrow().refs.is_empty());
    }
}
