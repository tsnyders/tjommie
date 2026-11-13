//! `browser.stream`: a bot's browser, seen live. The desktop's screencast posts each frame to `/browser-host`; this hub
//! hands it to the connections that turned the stream on, and only to them. A connection holds at most two frames it has
//! not acknowledged, and a newer frame replaces one still waiting for room: frames are dropped, never queued, so a client
//! that stops acknowledging costs no memory. When the desktop ends a stream (a private interaction began, the browser
//! closed) the watchers are told once, with a `null` frame, and the stream is over until they ask again.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use protocol::browser::{BrowserFrame, BrowserStreamFrame};
use protocol::native_browser::NativeBrowserStreamMessage;
use protocol::{ErrorCode, Event};

use crate::server::{RequestContext, RpcError};

/// Sends an event to one connection by its id. False when that connection is gone.
pub type EventTo = Arc<dyn Fn(&str, &Event) -> bool + Send + Sync>;

/// Frames a connection may hold without having acknowledged them.
pub const MAX_UNACKED: usize = 2;

#[derive(Default)]
struct Watcher {
    /// Sent and not yet acknowledged, oldest first.
    unacked: Vec<u64>,
    /// The newest frame that found the connection full. The next newer frame replaces it.
    waiting: Option<(u64, Arc<Event>)>,
}

#[derive(Default)]
struct State {
    /// Bot, then connection id, then what that connection has outstanding.
    bots: HashMap<String, HashMap<String, Watcher>>,
    /// Connections whose end is already being waited for.
    connections: HashSet<String>,
}

pub struct BrowserStreams {
    state: Mutex<State>,
    to: Mutex<Option<EventTo>>,
}

impl BrowserStreams {
    pub fn new() -> Arc<BrowserStreams> {
        Arc::new(BrowserStreams {
            state: Mutex::default(),
            to: Mutex::new(None),
        })
    }

    /// Where events go. Until this is set the stream is unavailable and `watch` answers `unknown_method`, as an older
    /// daemon does, so a client falls back to polling `browser.frame`.
    pub fn connect(&self, to: EventTo) {
        *self.to.lock().unwrap_or_else(PoisonError::into_inner) = Some(to);
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn sink(&self) -> Option<EventTo> {
        self.to
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Sends to a connection; one that is gone stops watching.
    fn deliver(&self, bot: &str, connection: &str, event: &Event) {
        if !self.sink().is_some_and(|to| to(connection, event)) {
            self.unwatch(bot, connection);
        }
    }

    /// How many connections are watching a bot.
    pub fn watching(&self, bot: &str) -> usize {
        self.state().bots.get(bot).map_or(0, HashMap::len)
    }

    /// A connection turns the stream on for a bot. Asking again changes nothing. When the connection ends, so does its watching.
    pub fn watch(self: &Arc<Self>, bot: &str, context: &RequestContext) -> Result<(), RpcError> {
        if self.sink().is_none() {
            return Err(RpcError::new(ErrorCode::UnknownMethod, "Unknown method"));
        }
        let first = {
            let mut state = self.state();
            state
                .bots
                .entry(bot.to_owned())
                .or_default()
                .entry(context.connection_id.clone())
                .or_default();
            state.connections.insert(context.connection_id.clone())
        };
        if first {
            let (streams, connection, signal) = (
                self.clone(),
                context.connection_id.clone(),
                context.signal.clone(),
            );
            tokio::spawn(async move {
                signal.cancelled().await;
                streams.forget(&connection);
            });
        }
        Ok(())
    }

    /// A connection turned the stream off for a bot.
    pub fn unwatch(&self, bot: &str, connection: &str) {
        let mut state = self.state();
        if let Some(watchers) = state.bots.get_mut(bot) {
            watchers.remove(connection);
            if watchers.is_empty() {
                state.bots.remove(bot);
            }
        }
    }

    /// A connection ended: it watches nothing.
    fn forget(&self, connection: &str) {
        let mut state = self.state();
        state.connections.remove(connection);
        state.bots.retain(|_, watchers| {
            watchers.remove(connection);
            !watchers.is_empty()
        });
    }

    /// The connection drew a frame. Acknowledging a frame acknowledges every older one too. A frame that waited for
    /// room goes out now.
    pub fn ack(&self, bot: &str, connection: &str, sequence: u64) {
        let next = {
            let mut state = self.state();
            let Some(watcher) = state
                .bots
                .get_mut(bot)
                .and_then(|watchers| watchers.get_mut(connection))
            else {
                return;
            };
            watcher.unacked.retain(|sent| *sent > sequence);
            let next = if watcher.unacked.len() < MAX_UNACKED {
                watcher.waiting.take()
            } else {
                None
            };
            if let Some((sent, _)) = &next {
                watcher.unacked.push(*sent);
            }
            next
        };
        if let Some((_, event)) = next {
            self.deliver(bot, connection, &event);
        }
    }

    /// A frame from the desktop, for the connections watching this bot. False when none is: the desktop should stop.
    pub fn frame(&self, bot: &str, frame: BrowserFrame) -> bool {
        let sequence = frame.sequence;
        let event = Arc::new(Event::BrowserStreamFrame(BrowserStreamFrame {
            bot_id: bot.to_owned(),
            frame: Some(frame),
        }));
        let ready: Vec<String> = {
            let mut state = self.state();
            let Some(watchers) = state.bots.get_mut(bot) else {
                return false;
            };
            let mut ready = Vec::new();
            for (connection, watcher) in watchers.iter_mut() {
                if watcher.unacked.len() < MAX_UNACKED {
                    watcher.unacked.push(sequence);
                    ready.push(connection.clone());
                } else {
                    // Replaces an older frame that never got room: dropped, not queued.
                    watcher.waiting = Some((sequence, event.clone()));
                }
            }
            ready
        };
        for connection in ready {
            self.deliver(bot, &connection, &event);
        }
        true
    }

    /// The stream of a bot is over: every watcher hears of it once, and none is left watching.
    pub fn ended(&self, bot: &str) {
        let Some(watchers) = self.state().bots.remove(bot) else {
            return;
        };
        let Some(to) = self.sink() else { return };
        let event = Event::BrowserStreamFrame(BrowserStreamFrame {
            bot_id: bot.to_owned(),
            frame: None,
        });
        for connection in watchers.keys() {
            to(connection, &event);
        }
    }

    /// What the desktop posted to `/browser-host`. False (the desktop is told 404) when nobody watches the frame's bot.
    pub fn ingest(&self, message: NativeBrowserStreamMessage) -> bool {
        match message {
            NativeBrowserStreamMessage::StreamFrame { bot_id, frame } => self.frame(&bot_id, frame),
            NativeBrowserStreamMessage::StreamEnd { bot_id, .. } => {
                self.ended(&bot_id);
                true
            }
        }
    }
}
