//! Another PC's bots' browsers in the panel (Phase 3 plan T3.7 over T1.4's `BrowserSession`; browser-panel.tsx
//! `RemotePane`). Each bot the panel shows gets a session on that PC's daemon connection. It opens when the Browser pane
//! shows and closes (handing the browser back) when it goes; its picture comes from the live stream, each frame
//! acknowledged once it is drawn (`browser.streamAck`), or, from a daemon without the stream (`unknown_method`), by
//! polling `browser.frame`. Its view is published into `BrowserData::remote` for the panel to draw.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use client::Rpc;
use gpui::{App, BackgroundExecutor, Global};
use protocol::browser::BrowserStreamFrame;

use super::{BrowserCommand, BrowserData};
use crate::guard::guarded_async;
use crate::model::browser_session::{BrowserSession, Clock, Closes};

/// The sessions' time: now, and the app's timers (a browser session reads only `now`; the trait has both).
#[derive(Clone)]
struct AppClock(BackgroundExecutor);

impl Clock for AppClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send {
        self.0.timer(duration)
    }
}

/// The sessions on one PC's connection, by bot.
struct Driver<R> {
    rpc: Arc<R>,
    clock: AppClock,
    closes: Arc<Closes>,
    /// The connection carries the stream's events (the TypeScript's `listen`).
    live: bool,
    sessions: RefCell<HashMap<String, Arc<BrowserSession>>>,
    /// The view change last published, and the picture timer last set, by bot.
    published: RefCell<HashMap<String, u64>>,
    scheduled: RefCell<HashMap<String, Instant>>,
}

/// Hands a streamed frame to the wired connection's sessions.
type OnFrame = Rc<dyn Fn(BrowserStreamFrame, &mut App)>;

/// Where `stream_frame` hands a frame.
struct Frames(OnFrame);

impl Global for Frames {}

/// Wires the panel to another PC's daemon: `rpc` is the connection, `live` whether it carries the stream's events.
pub fn wire<R: Rpc + 'static>(rpc: Arc<R>, live: bool, cx: &mut App) {
    let driver = Rc::new(Driver {
        rpc,
        clock: AppClock(cx.background_executor().clone()),
        closes: Arc::default(),
        live,
        sessions: RefCell::default(),
        published: RefCell::default(),
        scheduled: RefCell::default(),
    });
    let commands = driver.clone();
    cx.default_global::<BrowserData>().on_remote = Some(Rc::new(move |bot, command, _, cx| {
        commands.command(bot, command, cx)
    }));
    cx.set_global(Frames(Rc::new(move |data, cx| driver.frame(data, cx))));
}

/// A `browser.streamFrame` event from that PC's connection: its bot's session takes it.
pub fn stream_frame(data: BrowserStreamFrame, cx: &mut App) {
    if let Some(frames) = cx.try_global::<Frames>().map(|frames| frames.0.clone()) {
        frames(data, cx);
    }
}

impl<R: Rpc + 'static> Driver<R> {
    fn command(self: &Rc<Self>, bot: &str, command: BrowserCommand, cx: &mut App) {
        if command == BrowserCommand::Hide {
            self.close(bot, cx);
            return;
        }
        let session = match command {
            BrowserCommand::Show | BrowserCommand::Retry => self.session(bot),
            _ => match self.sessions.borrow().get(bot).cloned() {
                Some(session) => session,
                None => return,
            },
        };
        let (driver, bot) = (self.clone(), bot.to_owned());
        cx.spawn(async move |cx| {
            guarded_async("run a remote browser command", async move {
                let (rpc, clock) = (&*driver.rpc, &driver.clock);
                match command {
                    BrowserCommand::Show | BrowserCommand::Retry => session.open(rpc, clock).await,
                    BrowserCommand::SetControl(control) => {
                        session.set_control(rpc, clock, control).await
                    }
                    BrowserCommand::Navigate(url) => session.navigate(rpc, clock, &url).await,
                    BrowserCommand::History(action) => session.history(rpc, clock, action).await,
                    BrowserCommand::Tab { action, page_id } => {
                        session.tab(rpc, clock, action, page_id.as_deref()).await
                    }
                    BrowserCommand::Dialog {
                        accept,
                        prompt_text,
                    } => {
                        session
                            .dialog(rpc, clock, accept, prompt_text.as_deref())
                            .await
                    }
                    BrowserCommand::Drawn(frame) => session.drawn(rpc, &frame).await,
                    BrowserCommand::Hide => {}
                }
                cx.update(|cx| driver.after(&bot, cx));
            })
            .await
        })
        .detach();
    }

    /// The bot's session, new if it has none.
    fn session(&self, bot: &str) -> Arc<BrowserSession> {
        self.sessions
            .borrow_mut()
            .entry(bot.to_owned())
            .or_insert_with(|| Arc::new(BrowserSession::new(bot, self.closes.clone(), self.live)))
            .clone()
    }

    /// The panel let go of the bot's browser: its session ends, handing the browser back to the bot.
    fn close(&self, bot: &str, cx: &mut App) {
        let Some(session) = self.sessions.borrow_mut().remove(bot) else {
            return;
        };
        self.published.borrow_mut().remove(bot);
        self.scheduled.borrow_mut().remove(bot);
        cx.default_global::<BrowserData>().remote.remove(bot);
        let (rpc, clock) = (self.rpc.clone(), self.clock.clone());
        cx.spawn(async move |_| {
            guarded_async("close a remote browser", async move {
                session.close(&*rpc, &clock).await
            })
            .await
        })
        .detach();
    }

    /// A streamed frame for a session that listens.
    fn frame(self: &Rc<Self>, data: BrowserStreamFrame, cx: &mut App) {
        let Some(session) = self.sessions.borrow().get(&data.bot_id).cloned() else {
            return;
        };
        if !session.listening() {
            return;
        }
        let (driver, bot) = (self.clone(), data.bot_id.clone());
        cx.spawn(async move |cx| {
            guarded_async("take a streamed frame", async move {
                session.stream(&*driver.rpc, data).await;
                cx.update(|cx| driver.after(&bot, cx));
            })
            .await
        })
        .detach();
    }

    /// After a session call: its view, if it moved, goes to the panel, and its picture timer is set.
    fn after(self: &Rc<Self>, bot: &str, cx: &mut App) {
        let Some(session) = self.sessions.borrow().get(bot).cloned() else {
            return;
        };
        let changes = session.changes();
        if self.published.borrow().get(bot) != Some(&changes) {
            self.published.borrow_mut().insert(bot.to_owned(), changes);
            cx.default_global::<BrowserData>()
                .remote
                .insert(bot.to_owned(), session.view());
            cx.refresh_windows();
        }
        let Some(due) = session.next_timer() else {
            return;
        };
        if self.scheduled.borrow().get(bot) == Some(&due) {
            return;
        }
        self.scheduled.borrow_mut().insert(bot.to_owned(), due);
        let timer = cx
            .background_executor()
            .timer(due.saturating_duration_since(Instant::now()));
        let (driver, bot) = (self.clone(), bot.to_owned());
        cx.spawn(async move |cx| {
            guarded_async("pull a remote picture", async move {
                timer.await;
                let session = driver.sessions.borrow().get(&bot).cloned();
                if let Some(session) = session {
                    // unguarded: the session's timer method, not a GPUI handler; this task runs in guarded_async
                    session.on_timer(&*driver.rpc, &driver.clock).await;
                    cx.update(|cx| driver.after(&bot, cx));
                }
            })
            .await
        })
        .detach();
    }
}
