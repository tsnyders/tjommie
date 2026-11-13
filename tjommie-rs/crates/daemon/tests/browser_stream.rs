//! `browser.stream`'s fan-out (M8, spec 3.9): frames go only to the connections watching a bot, at most two are unacknowledged
//! per connection, a newer frame replaces one still waiting, and a client that stops acknowledging costs no memory.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use daemon::browser_stream::{BrowserStreams, EventTo, MAX_UNACKED};
use daemon::server::{Principal, RequestContext};
use protocol::browser::BrowserFrame;
use protocol::native_browser::{NativeBrowserStreamMessage, StreamEnd};
use protocol::{ErrorCode, Event};
use serde_json::json;
use tokio_util::sync::CancellationToken;

type Sent = Arc<Mutex<Vec<(String, Event)>>>;
type Gone = Arc<Mutex<HashSet<String>>>;

struct Hub {
    streams: Arc<BrowserStreams>,
    sent: Sent,
    /// Connections whose delivery fails, as if they had left.
    gone: Gone,
}

fn hub() -> Hub {
    let (sent, gone): (Sent, Gone) = (Arc::default(), Arc::default());
    let streams = BrowserStreams::new();
    let (record, leave) = (sent.clone(), gone.clone());
    let to: EventTo = Arc::new(move |connection, event| {
        if leave.lock().unwrap().contains(connection) {
            return false;
        }
        record
            .lock()
            .unwrap()
            .push((connection.to_owned(), event.clone()));
        true
    });
    streams.connect(to);
    Hub {
        streams,
        sent,
        gone,
    }
}

fn context(connection: &str) -> RequestContext {
    RequestContext {
        connection_id: connection.into(),
        principal: Principal::Local,
        signal: CancellationToken::new(),
    }
}

fn frame(sequence: u64) -> BrowserFrame {
    serde_json::from_value(json!({
        "data": "image", "mime": "image/jpeg", "width": 1280, "height": 800, "sequence": sequence, "streamId": "stream"
    }))
    .unwrap()
}

impl Hub {
    /// The sequences a connection was sent, in order; `None` marks the end of the stream.
    fn received(&self, connection: &str) -> Vec<Option<u64>> {
        self.sent
            .lock()
            .unwrap()
            .iter()
            .filter(|(to, _)| to == connection)
            .map(|(_, event)| match event {
                Event::BrowserStreamFrame(sent) => sent.frame.as_ref().map(|frame| frame.sequence),
                other => panic!("only stream frames are sent here, got {other:?}"),
            })
            .collect()
    }

    fn sequences(&self, connection: &str) -> Vec<u64> {
        self.received(connection).into_iter().flatten().collect()
    }
}

async fn eventually(mut condition: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !condition() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the condition holds in time");
}

#[tokio::test]
async fn sends_a_frame_only_to_the_connections_watching_that_bot() {
    let hub = hub();
    hub.streams.watch("bot-1", &context("a")).unwrap();
    hub.streams.watch("bot-2", &context("b")).unwrap();
    // "c" is connected but watches nothing.
    assert!(hub.streams.frame("bot-1", frame(1)));
    assert!(!hub.streams.frame("bot-3", frame(1)));
    assert_eq!(hub.sequences("a"), [1]);
    assert!(hub.received("b").is_empty() && hub.received("c").is_empty());
    assert_eq!(hub.streams.watching("bot-1"), 1);
}

#[tokio::test]
async fn holds_at_most_two_frames_unacknowledged_and_sends_the_newest_waiting_one_when_one_is_acknowledged()
 {
    let hub = hub();
    hub.streams.watch("bot", &context("a")).unwrap();
    for sequence in 1..=6 {
        hub.streams.frame("bot", frame(sequence));
    }
    assert_eq!(MAX_UNACKED, 2);
    assert_eq!(
        hub.sequences("a"),
        [1, 2],
        "3 to 5 were replaced by 6, which waits"
    );
    hub.streams.ack("bot", "a", 1);
    assert_eq!(
        hub.sequences("a"),
        [1, 2, 6],
        "the waiting frame is the newest"
    );
    hub.streams.ack("bot", "a", 2);
    assert_eq!(
        hub.sequences("a"),
        [1, 2, 6],
        "nothing waits, so nothing more goes out"
    );
    hub.streams.frame("bot", frame(7));
    assert_eq!(hub.sequences("a"), [1, 2, 6, 7]);
    for sequence in [8, 9] {
        hub.streams.frame("bot", frame(sequence));
    }
    // An acknowledgement covers every older frame too: 6 and 7 are both done, so 9 (8 was replaced) takes one slot.
    hub.streams.ack("bot", "a", 7);
    assert_eq!(hub.sequences("a"), [1, 2, 6, 7, 9]);
}

#[tokio::test]
async fn a_client_that_never_acknowledges_gets_two_frames_and_costs_nothing_more() {
    let hub = hub();
    hub.streams.watch("bot", &context("slow")).unwrap();
    for sequence in 1..=10_000 {
        assert!(hub.streams.frame("bot", frame(sequence)));
    }
    // 9,998 frames were offered and not one of them was queued: the sink saw two.
    assert_eq!(hub.sequences("slow"), [1, 2]);
    // When it finally catches up it gets the newest frame, not a backlog.
    hub.streams.ack("bot", "slow", 2);
    assert_eq!(hub.sequences("slow"), [1, 2, 10_000]);
    hub.streams.frame("bot", frame(10_001));
    assert_eq!(hub.sequences("slow"), [1, 2, 10_000, 10_001]);
}

#[tokio::test]
async fn acknowledging_frees_only_the_connection_that_drew_the_frame() {
    let hub = hub();
    hub.streams.watch("bot", &context("a")).unwrap();
    hub.streams.watch("bot", &context("b")).unwrap();
    for sequence in 1..=4 {
        hub.streams.frame("bot", frame(sequence));
    }
    hub.streams.ack("bot", "b", 2);
    assert_eq!(hub.sequences("b"), [1, 2, 4]);
    assert_eq!(hub.sequences("a"), [1, 2], "a has acknowledged nothing");
    // Acknowledgements from a connection that watches nothing, or for a bot nobody watches, do nothing.
    hub.streams.ack("bot", "stranger", 4);
    hub.streams.ack("other", "a", 4);
    assert_eq!(hub.sequences("a"), [1, 2]);
}

#[tokio::test]
async fn tells_watchers_once_when_the_stream_ends_and_sends_nothing_after() {
    let hub = hub();
    hub.streams.watch("bot", &context("a")).unwrap();
    hub.streams.frame("bot", frame(1));
    hub.streams.ended("bot");
    assert_eq!(hub.received("a"), [Some(1), None]);
    assert_eq!(hub.streams.watching("bot"), 0);
    assert!(
        !hub.streams.frame("bot", frame(2)),
        "the desktop is told nobody is watching"
    );
    hub.streams.ended("bot");
    assert_eq!(
        hub.received("a"),
        [Some(1), None],
        "a second end says nothing"
    );
}

#[tokio::test]
async fn a_connection_that_ends_stops_watching() {
    let hub = hub();
    let ending = context("ending");
    hub.streams.watch("bot", &ending).unwrap();
    hub.streams.watch("bot", &context("staying")).unwrap();
    assert_eq!(hub.streams.watching("bot"), 2);
    ending.signal.cancel();
    eventually(|| hub.streams.watching("bot") == 1).await;
    hub.streams.frame("bot", frame(1));
    assert!(hub.received("ending").is_empty());
    assert_eq!(hub.sequences("staying"), [1]);
}

#[tokio::test]
async fn a_connection_that_has_left_stops_watching_when_a_send_to_it_fails() {
    let hub = hub();
    hub.streams.watch("bot", &context("gone")).unwrap();
    hub.streams.watch("bot", &context("here")).unwrap();
    hub.gone.lock().unwrap().insert("gone".into());
    assert!(hub.streams.frame("bot", frame(1)));
    assert_eq!(hub.streams.watching("bot"), 1);
    assert_eq!(hub.sequences("here"), [1]);
}

#[tokio::test]
async fn stops_sending_to_a_connection_that_turned_the_stream_off() {
    let hub = hub();
    hub.streams.watch("bot", &context("a")).unwrap();
    hub.streams.watch("bot", &context("b")).unwrap();
    hub.streams.unwatch("bot", "a");
    hub.streams.frame("bot", frame(1));
    assert!(hub.received("a").is_empty());
    assert_eq!(hub.sequences("b"), [1]);
    hub.streams.unwatch("bot", "b");
    assert!(!hub.streams.frame("bot", frame(2)));
}

#[tokio::test]
async fn is_unavailable_until_events_are_connected_like_an_older_daemon() {
    let streams = BrowserStreams::new();
    let refused = streams.watch("bot", &context("a")).unwrap_err();
    assert_eq!(refused.code, ErrorCode::UnknownMethod);
    assert!(!streams.frame("bot", frame(1)));
}

#[tokio::test]
async fn takes_what_the_desktop_posts() {
    let hub = hub();
    let posted = |sequence| NativeBrowserStreamMessage::StreamFrame {
        bot_id: "bot".into(),
        frame: frame(sequence),
    };
    assert!(!hub.streams.ingest(posted(1)), "nobody watches yet");
    hub.streams.watch("bot", &context("a")).unwrap();
    assert!(hub.streams.ingest(posted(2)));
    assert!(hub.streams.ingest(NativeBrowserStreamMessage::StreamEnd {
        bot_id: "bot".into(),
        reason: StreamEnd::Private,
    }));
    assert_eq!(hub.received("a"), [Some(2), None]);
}
