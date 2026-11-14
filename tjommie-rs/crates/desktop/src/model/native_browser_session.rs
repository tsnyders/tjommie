//! Port of `packages/ui/lib/native-browser-session.ts` (T1.4): one bot's native browser as the panel sees it. The desktop app
//! draws the page itself, so this session has no picture: it keeps the state, sends commands and the page's bounds, and
//! hands the browser back to the bot on close or suspend. The TypeScript reaches the app through `window.desktop`; here that
//! is the `NativeBrowserBridge` trait, and time (the mount's retry pauses) is the `Clock` trait.
//!
//! What the TypeScript does not do: a state event arrives through `on_state` and a focus return through `on_focus_return`,
//! because the caller subscribes to the desktop's events (the session says so through `listening()`).

use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use protocol::browser::{BrowserControl, BrowserState, HistoryAction, TabAction};
use protocol::native_browser::{NativeBrowserBounds, NativeBrowserMount, NativeBrowserUiCommand};
use tokio::sync::watch;

use super::browser_session::{BrowserView, Closes, locked, wait};
use super::labels::error_text;

/// The pause before a failed mount is tried again, and how many times a failed mount is retried by itself.
const MOUNT_RETRY: Duration = Duration::from_millis(500);
const MOUNT_RETRIES: u32 = 6;

/// The desktop app's native browser, as the panel calls it. Each call is answered with the bot's browser state (or an error
/// message, shown as the view's error).
pub trait NativeBrowserBridge {
    /// Runs one command on a bot's browser: open, state, control, navigate, history, tab, or a dialog answer.
    fn command(
        &self,
        bot_id: &str,
        command: NativeBrowserUiCommand,
    ) -> impl Future<Output = Result<BrowserState, String>> + Send;

    /// Puts the bot's page at these bounds and shows it, or hides it (`visible: false`), keeping its context.
    fn mount(
        &self,
        bot_id: &str,
        mount: NativeBrowserMount,
    ) -> impl Future<Output = Result<BrowserState, String>> + Send;
}

/// The session's clock: the browser session's trait. `sleep` paces the retries of a failed hide.
pub use super::browser_session::Clock;

/// What the shared browser chrome shows for a native state: no picture, since the page is drawn by the desktop app.
pub fn native_view(state: Option<BrowserState>, error: Option<String>) -> BrowserView {
    BrowserView {
        state,
        frame: None,
        error,
        busy: false,
    }
}

/// Whether two bounds are the same place. A hidden view (`None`) is the same only as another hidden view.
pub fn same_rect(a: Option<&NativeBrowserBounds>, b: Option<&NativeBrowserBounds>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => {
            a.x == b.x && a.y == b.y && a.width == b.width && a.height == b.height
        }
        (None, None) => true,
        _ => false,
    }
}

/// One bot's native browser as the panel sees it. Commands (open, control, navigate, tabs, the hand-back) go through one lane,
/// so they reach the app in the order the user acted. Where the page is shown (the mount) has a lane of its own, so hiding it
/// for a menu never waits behind a slow navigation.
pub struct NativeBrowserSession {
    bot_id: String,
    closes: Arc<Closes>,
    inner: Mutex<Inner>,
}

/// A task of the session's lane. A release runs even while closing.
struct Task {
    job: Job,
    release: bool,
    /// Set when the task has run. `suspend` waits on it.
    done: Option<Arc<watch::Sender<bool>>>,
}

enum Job {
    Open {
        earlier: Option<watch::Receiver<bool>>,
    },
    /// A command the desktop app answers. `asked` is a take-control, which stops counting as asked once it has run.
    Command {
        command: NativeBrowserUiCommand,
        asked: bool,
    },
    /// The state, read again after an event overtook a reply that had no revision.
    Refresh,
    HandBack,
    /// Hands the browser back and hides the view. `done` ends the close; `listen` is the subscription this close owns;
    /// `token` is this close's own registry entry.
    Close {
        done: Arc<watch::Sender<bool>>,
        listen: u64,
        token: u64,
    },
}

/// Clears the lane's `running` flag when a drain is dropped before the queue empties, so a cancelled caller cannot wedge the
/// lane. Disarmed once `next_task` has cleared the flag itself.
struct LaneGuard<'a> {
    inner: &'a Mutex<Inner>,
    armed: bool,
}

impl Drop for LaneGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            locked(self.inner).running = false;
        }
    }
}

/// Ends the mount loop when it is dropped before it finishes: clears `mounting` and wakes whoever waits for the mount lane to
/// go idle (`hidden`), so a cancelled mount cannot leave the hide waiting for good (M8).
struct MountGuard<'a> {
    inner: &'a Mutex<Inner>,
    armed: bool,
}

impl Drop for MountGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let mut s = locked(self.inner);
            s.mounting = false;
            s.mount_idle.send_replace(true);
        }
    }
}

/// One mount: what to send, and what it will be recorded as once the app has accepted it.
struct MountStep {
    mount: NativeBrowserMount,
    want: Option<NativeBrowserBounds>,
    occluded: bool,
    seen: u64,
}

/// The session's state. Everything that needs no call to the app happens here, under the lock.
struct Inner {
    view: BrowserView,
    changes: u64,
    hide_error: Option<String>,
    tasks: VecDeque<Task>,
    running: bool,
    closing: bool,
    done: Option<Arc<watch::Sender<bool>>>,
    listening: bool,
    /// Which subscription is current: a close only stops the one it started with.
    listen_id: u64,
    opened: bool,
    /// Where the view should be, as last asked, and whether it is occluded.
    rect: Option<NativeBrowserBounds>,
    occluded: bool,
    sent_occluded: bool,
    /// Where the page was last mounted: `None` never, `Some(None)` hidden.
    sent: Option<Option<NativeBrowserBounds>>,
    last: NativeBrowserBounds,
    mounting: bool,
    mount_idle: watch::Sender<bool>,
    /// A freshly opened browser's first mount: the drain that ran the open sends it once the lane is free (M6).
    mount_wanted: bool,
    retry_at: Option<Instant>,
    retry_timer: Option<Instant>,
    failures: u32,
    /// State events seen, to tell a reply that an event overtook.
    events: u64,
    latest: Option<BrowserState>,
    human: bool,
    asking: u32,
}

impl Inner {
    fn new() -> Self {
        Self {
            view: BrowserView::default(),
            changes: 0,
            hide_error: None,
            tasks: VecDeque::new(),
            running: false,
            closing: false,
            done: None,
            listening: false,
            listen_id: 0,
            opened: false,
            rect: None,
            occluded: false,
            sent_occluded: false,
            sent: None,
            last: NativeBrowserBounds {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
            mounting: false,
            mount_idle: watch::channel(true).0,
            mount_wanted: false,
            retry_at: None,
            retry_timer: None,
            failures: 0,
            events: 0,
            latest: None,
            human: false,
            asking: 0,
        }
    }

    /// The view's one update. Every update is counted, as the TypeScript notifies on each.
    fn set(&mut self, state: Option<BrowserState>, error: Option<String>) {
        if self.closing {
            return;
        }
        self.view = native_view(state, error);
        self.changes += 1;
    }

    /// A state from the app is shown unless something newer has been seen. A reply that an event overtook is dropped, and a
    /// reply without a revision asks for the state again (not the mount's replies, which are incidental and frequent).
    fn accept(&mut self, state: BrowserState, seen: Option<u64>, refresh: bool) {
        if let Some(now) = &self.view.state
            && now.generation == state.generation
            && now.revision.is_some()
            && state.revision.is_some()
            && state.revision < now.revision
        {
            return;
        }
        if let Some(seen) = seen
            && seen != self.events
        {
            let newer = self.latest.as_ref().is_some_and(|latest| {
                latest.generation == state.generation
                    && latest.revision.is_some()
                    && state.revision.is_some()
                    && state.revision > latest.revision
            });
            if !newer {
                if refresh && state.revision.is_none() && !self.closing {
                    self.tasks.push_back(Task {
                        job: Job::Refresh,
                        release: false,
                        done: None,
                    });
                }
                return;
            }
        }
        self.human = state.control == BrowserControl::Human;
        self.set(Some(state), None);
    }

    /// Queues a task. Returns whether the caller must run the lane (no one else is).
    fn submit(&mut self, task: Task) -> bool {
        if self.closing && !task.release {
            return false;
        }
        self.tasks.push_back(task);
        if self.running {
            false
        } else {
            self.running = true;
            true
        }
    }

    fn next_task(&mut self) -> Option<Task> {
        let next = self.tasks.pop_front();
        if next.is_none() {
            self.running = false;
        }
        next
    }

    /// A command at the page the user is looking at (`#onPage`).
    fn on_page(&mut self, make: impl FnOnce(String, String) -> NativeBrowserUiCommand) -> bool {
        let Some(state) = self.view.state.clone() else {
            return false;
        };
        let Some(page_id) = state.active_page_id.clone().filter(|page| !page.is_empty()) else {
            return false;
        };
        let command = make(state.generation.clone(), page_id);
        self.submit(Task {
            job: Job::Command {
                command,
                asked: false,
            },
            release: false,
            done: None,
        })
    }

    /// The next mount to send, if the app doesn't have the wanted place yet, and a failed mount's pause is over.
    fn next_mount(&mut self, now: Instant) -> Option<MountStep> {
        let pending = match &self.sent {
            None => true,
            Some(sent) => {
                !same_rect(self.rect.as_ref(), sent.as_ref()) || self.occluded != self.sent_occluded
            }
        };
        if !pending {
            return None;
        }
        if !self.closing && self.retry_at.is_some_and(|at| now < at) {
            return None;
        }
        let want = self.rect.clone();
        if let Some(bounds) = &want {
            self.last = bounds.clone();
        }
        let bounds = want.clone().unwrap_or_else(|| self.last.clone());
        Some(MountStep {
            mount: NativeBrowserMount {
                visible: want.is_some(),
                bounds,
                occluded: self.occluded.then_some(true),
            },
            want,
            occluded: self.occluded,
            seen: self.events,
        })
    }

    /// A failed mount is tried again by itself, a bounded number of times, whether or not the viewport asks again.
    fn retry_later(&mut self, now: Instant) {
        if self.retry_timer.is_some() || self.failures > MOUNT_RETRIES {
            return;
        }
        self.retry_timer = Some(now + MOUNT_RETRY);
    }
}

impl NativeBrowserSession {
    /// A session for `bot_id`. The closes are the app's registry for native sessions (the app holds one `Closes` for each
    /// session kind, and shares it across that kind's sessions).
    pub fn new(bot_id: &str, closes: Arc<Closes>) -> Self {
        Self {
            bot_id: bot_id.to_owned(),
            closes,
            inner: Mutex::new(Inner::new()),
        }
    }

    fn with<T>(&self, step: impl FnOnce(&mut Inner) -> T) -> T {
        step(&mut locked(&self.inner))
    }

    pub fn view(&self) -> BrowserView {
        self.with(|s| s.view.clone())
    }

    /// Set if a close couldn't get the page hidden: the page may still cover the window.
    pub fn hide_error(&self) -> Option<String> {
        self.with(|s| s.hide_error.clone())
    }

    /// Whether the state events should be subscribed to: the caller listens while this is true.
    pub fn listening(&self) -> bool {
        self.with(|s| s.listening)
    }

    /// When a failed mount is tried again by itself. The caller calls `on_retry` at that instant.
    pub fn retry_due(&self) -> Option<Instant> {
        self.with(|s| s.retry_timer)
    }

    /// A state event from the app, for any bot: only this bot's is taken.
    pub fn on_state(&self, state: BrowserState) {
        self.with(|s| {
            if state.bot_id != self.bot_id {
                return;
            }
            s.events += 1;
            s.latest = Some(state.clone());
            s.accept(state, None, true);
        });
    }

    /// The page let go of the keyboard (Shift+Tab). Returns whether it was this bot's page and the panel should take focus.
    pub fn on_focus_return(&self, bot_id: &str) -> bool {
        self.with(|s| bot_id == self.bot_id && !s.closing)
    }

    /// The retry of a failed mount is due.
    pub async fn on_retry(&self, bridge: &impl NativeBrowserBridge, clock: &impl Clock) {
        self.with(|s| s.retry_timer = None);
        self.sync_mount(bridge, clock).await;
    }

    /// Opens the bot's browser and shows the view where it was asked to be. Opening again rebinds the subscription, and
    /// waits for an earlier close of the bot.
    pub async fn open(&self, bridge: &impl NativeBrowserBridge, clock: &impl Clock) {
        let earlier = self.closes.get(&self.bot_id);
        let drain = self.with(|s| {
            s.closing = false;
            s.done = None;
            s.listening = true;
            s.listen_id += 1;
            s.submit(Task {
                job: Job::Open { earlier },
                release: false,
                done: None,
            })
        });
        if drain {
            self.drain(bridge, clock).await;
        }
    }

    /// Where the page goes: the viewport's rectangle in the window, or `None` to hide it. Only a change is sent, and only the
    /// newest while one is in flight.
    pub async fn set_bounds(
        &self,
        bridge: &impl NativeBrowserBridge,
        clock: &impl Clock,
        rect: Option<NativeBrowserBounds>,
        occluded: bool,
    ) {
        let opened = self.with(|s| {
            s.occluded = rect.is_some() && occluded;
            s.rect = rect;
            s.opened
        });
        if opened {
            self.sync_mount(bridge, clock).await;
        }
    }

    pub async fn set_control(
        &self,
        bridge: &impl NativeBrowserBridge,
        clock: &impl Clock,
        control: BrowserControl,
    ) {
        let drain = self.with(|s| {
            let asked = control == BrowserControl::Human;
            if asked {
                s.asking += 1;
            }
            s.submit(Task {
                job: Job::Command {
                    command: NativeBrowserUiCommand::Control { control },
                    asked,
                },
                release: false,
                done: None,
            })
        });
        if drain {
            self.drain(bridge, clock).await;
        }
    }

    pub async fn navigate(&self, bridge: &impl NativeBrowserBridge, clock: &impl Clock, url: &str) {
        let drain = self.with(|s| {
            s.on_page(|generation, page_id| NativeBrowserUiCommand::Navigate {
                generation,
                page_id,
                url: url.to_owned(),
            })
        });
        if drain {
            self.drain(bridge, clock).await;
        }
    }

    pub async fn history(
        &self,
        bridge: &impl NativeBrowserBridge,
        clock: &impl Clock,
        action: HistoryAction,
    ) {
        let drain = self.with(|s| {
            s.on_page(|generation, page_id| NativeBrowserUiCommand::History {
                generation,
                page_id,
                action,
            })
        });
        if drain {
            self.drain(bridge, clock).await;
        }
    }

    pub async fn tab(
        &self,
        bridge: &impl NativeBrowserBridge,
        clock: &impl Clock,
        action: TabAction,
        page_id: Option<&str>,
    ) {
        let drain = self.with(|s| {
            let Some(generation) = s
                .view
                .state
                .as_ref()
                .map(|state| state.generation.clone())
                .filter(|g| !g.is_empty())
            else {
                return false;
            };
            s.submit(Task {
                job: Job::Command {
                    command: NativeBrowserUiCommand::Tab {
                        generation,
                        action,
                        page_id: page_id.map(str::to_owned),
                    },
                    asked: false,
                },
                release: false,
                done: None,
            })
        });
        if drain {
            self.drain(bridge, clock).await;
        }
    }

    /// Answers the dialog that is open, on the page and generation it belongs to.
    pub async fn dialog(
        &self,
        bridge: &impl NativeBrowserBridge,
        clock: &impl Clock,
        accept: bool,
        prompt_text: Option<&str>,
    ) {
        let drain = self.with(|s| {
            let Some(state) = s.view.state.clone() else {
                return false;
            };
            let Some(open) = state.dialog.clone() else {
                return false;
            };
            s.submit(Task {
                job: Job::Command {
                    command: NativeBrowserUiCommand::Dialog {
                        generation: state.generation.clone(),
                        page_id: open.page_id.clone(),
                        accept,
                        prompt_text: prompt_text.map(str::to_owned),
                    },
                    asked: false,
                },
                release: false,
                done: None,
            })
        });
        if drain {
            self.drain(bridge, clock).await;
        }
    }

    /// Leaving the Browser pane: the hand-back is queued and waited for. The browser stays open for the bot.
    pub async fn suspend(&self, bridge: &impl NativeBrowserBridge, clock: &impl Clock) {
        let (tx, rx) = watch::channel(false);
        let drain = self.with(|s| {
            s.submit(Task {
                job: Job::HandBack,
                release: false,
                done: Some(Arc::new(tx)),
            })
        });
        if drain {
            self.drain(bridge, clock).await;
        }
        wait(Some(rx)).await;
    }

    /// Terminal: hides the view at once, hands the browser back, and stops listening. Anything queued but not yet sent is
    /// dropped, except the hand-back. Resolves when the hide and the hand-back are done.
    pub async fn close(&self, bridge: &impl NativeBrowserBridge, clock: &impl Clock) {
        // Each close mints its own registry token (M7): a later close of this session must not remove this close's entry.
        let token = self
            .closes
            .issued
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (waiter, opened, drain) = self.with(|s| {
            if let Some(done) = &s.done {
                return (Some(done.subscribe()), false, false);
            }
            s.tasks.retain(|task| task.release);
            let (tx, rx) = watch::channel(false);
            let done = Arc::new(tx);
            self.closes.put(&self.bot_id, token, rx.clone());
            s.done = Some(done.clone());
            let listen = s.listen_id;
            s.retry_timer = None;
            s.closing = true;
            s.rect = None;
            s.occluded = false;
            let opened = s.opened;
            let drain = s.submit(Task {
                job: Job::Close {
                    done,
                    listen,
                    token,
                },
                release: true,
                done: None,
            });
            (Some(rx), opened, drain)
        });
        // The hide and the hand-back run together, as the TypeScript's two `void` calls do: a hung mount cannot keep the bot
        // from getting its browser back (M6). The close resolves when both have finished.
        let hide = async {
            if opened {
                self.sync_mount(bridge, clock).await;
            }
        };
        let hand_back = async {
            if drain {
                self.drain(bridge, clock).await;
            }
        };
        tokio::join!(hide, hand_back);
        wait(waiter).await;
    }

    /// Runs the lane: each queued task in order, until the queue is empty. A failure is shown, and the queue goes on.
    /// A drain dropped before the queue empties releases the lane (`LaneGuard`). A browser's first mount is sent here, once
    /// the lane is free: a slow mount holds up no queued command (M6).
    async fn drain(&self, bridge: &impl NativeBrowserBridge, clock: &impl Clock) {
        let mut lane = LaneGuard {
            inner: &self.inner,
            armed: true,
        };
        while let Some(task) = self.with(|s| s.next_task()) {
            let Task { job, done, .. } = task;
            if let Err(error) = self.run(bridge, clock, job).await {
                self.with(|s| {
                    let state = s.view.state.clone();
                    s.set(state, Some(error_text(&error)));
                });
            }
            if let Some(done) = done {
                done.send_replace(true);
            }
        }
        lane.armed = false; // the queue is empty, and `next_task` has cleared `running`
        if self.with(|s| std::mem::take(&mut s.mount_wanted)) {
            self.sync_mount(bridge, clock).await;
        }
    }

    async fn run(
        &self,
        bridge: &impl NativeBrowserBridge,
        clock: &impl Clock,
        job: Job,
    ) -> Result<(), String> {
        match job {
            Job::Open { earlier } => {
                wait(earlier).await;
                self.run_command(bridge, NativeBrowserUiCommand::Open)
                    .await?;
                self.with(|s| {
                    s.opened = true;
                    s.mount_wanted = true; // sent by the drain once the lane is free, not inside it (M6)
                });
                Ok(())
            }
            Job::Command { command, asked } => {
                let outcome = self.run_command(bridge, command).await;
                if asked {
                    self.with(|s| s.asking = s.asking.saturating_sub(1));
                }
                outcome
            }
            Job::Refresh => {
                let state = bridge
                    .command(&self.bot_id, NativeBrowserUiCommand::State)
                    .await?;
                self.with(|s| s.accept(state, None, true));
                Ok(())
            }
            Job::HandBack => self.hand_back(bridge).await,
            Job::Close {
                done,
                listen,
                token,
            } => {
                let _ = self.hand_back(bridge).await; // best effort at a terminal close
                self.hidden(bridge, clock).await;
                self.with(|s| {
                    if s.listen_id == listen {
                        s.listening = false;
                    }
                });
                done.send_replace(true);
                self.closes.drop_if(&self.bot_id, token);
                Ok(())
            }
        }
    }

    async fn run_command(
        &self,
        bridge: &impl NativeBrowserBridge,
        command: NativeBrowserUiCommand,
    ) -> Result<(), String> {
        let seen = self.with(|s| s.events);
        let state = bridge.command(&self.bot_id, command).await?;
        self.with(|s| s.accept(state, Some(seen), true));
        Ok(())
    }

    /// From the app's own word on where things stand, since a take-control still in flight is not in the local view. If the
    /// state can't be asked, a take-control that was asked for (or seen) counts, so the hand-back is still tried.
    async fn hand_back(&self, bridge: &impl NativeBrowserBridge) -> Result<(), String> {
        let seen = self.with(|s| s.events);
        let state = bridge
            .command(&self.bot_id, NativeBrowserUiCommand::State)
            .await
            .ok();
        if let Some(state) = &state {
            self.with(|s| s.accept(state.clone(), Some(seen), true));
        }
        let human = match &state {
            Some(state) => state.control == BrowserControl::Human,
            None => self.with(|s| s.human || s.asking > 0),
        };
        if !human {
            return Ok(());
        }
        if let Some(state) = &state
            && let Some(dialog) = &state.dialog
        {
            let command = NativeBrowserUiCommand::Dialog {
                generation: state.generation.clone(),
                page_id: dialog.page_id.clone(),
                accept: false,
                prompt_text: None,
            };
            let _ = bridge.command(&self.bot_id, command).await;
        }
        let asked = self.with(|s| s.events);
        let reply = bridge
            .command(
                &self.bot_id,
                NativeBrowserUiCommand::Control {
                    control: BrowserControl::Bot,
                },
            )
            .await?;
        self.with(|s| s.accept(reply, Some(asked), true));
        Ok(())
    }

    /// Waits until the mount lane is idle (a mount in flight has finished).
    async fn wait_idle(&self) {
        let idle = self.with(|s| s.mount_idle.subscribe());
        wait(Some(idle)).await;
    }

    /// Waits until the app has accepted the hide, trying again a few times, and says so if it never did.
    async fn hidden(&self, bridge: &impl NativeBrowserBridge, clock: &impl Clock) {
        if !self.with(|s| s.opened) {
            return;
        }
        self.wait_idle().await;
        let mut tries = 0;
        while !self.with(|s| matches!(s.sent, Some(None))) && tries < MOUNT_RETRIES {
            clock.sleep(MOUNT_RETRY).await;
            self.sync_mount(bridge, clock).await;
            self.wait_idle().await;
            tries += 1;
        }
        let hidden = self.with(|s| matches!(s.sent, Some(None)));
        self.with(|s| {
            s.hide_error = if hidden {
                None
            } else {
                Some("The browser couldn't be hidden.".to_owned())
            };
        });
    }

    /// The mount lane: sends the wanted place until the app has it. A failed mount is shown and tried again after a pause,
    /// not treated as sent. Returns at once if a mount is already in flight (its loop picks up the newest bounds).
    async fn sync_mount(&self, bridge: &impl NativeBrowserBridge, clock: &impl Clock) {
        let started = self.with(|s| {
            if s.mounting {
                return false;
            }
            s.mounting = true;
            s.mount_idle.send_replace(false);
            true
        });
        if !started {
            return;
        }
        let mut mount_guard = MountGuard {
            inner: &self.inner,
            armed: true,
        };
        tokio::task::yield_now().await; // the TypeScript lane's first step waits a microtask, so bounds set meanwhile count
        loop {
            let now = clock.now();
            let Some(step) = self.with(|s| s.next_mount(now)) else {
                break;
            };
            let MountStep {
                mount,
                want,
                occluded,
                seen,
            } = step;
            match bridge.mount(&self.bot_id, mount).await {
                Ok(state) => self.with(|s| {
                    s.accept(state, Some(seen), false);
                    s.sent = Some(want);
                    s.sent_occluded = occluded;
                    s.failures = 0;
                }),
                Err(error) => {
                    let at = clock.now();
                    self.with(|s| {
                        s.failures += 1;
                        s.retry_at = Some(at + MOUNT_RETRY);
                        let state = s.view.state.clone();
                        s.set(state, Some(error_text(&error)));
                        s.retry_later(at);
                    });
                    break;
                }
            }
        }
        mount_guard.armed = false;
        self.with(|s| {
            s.mounting = false;
            s.mount_idle.send_replace(true);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::cell::Cell;
    use std::sync::Mutex;
    use tokio::sync::watch;

    /// The TypeScript `state()`: one tab, the bot in control, with `patch` merged over it.
    fn state(patch: Value) -> BrowserState {
        let mut base = json!({
            "botId": "b1", "generation": "g1", "control": "bot", "activePageId": "p1",
            "tabs": [{ "id": "p1", "url": "https://example.com/", "title": "Example" }], "dialog": null
        });
        if let (Some(base), Value::Object(patch)) = (base.as_object_mut(), patch) {
            base.extend(patch);
        }
        serde_json::from_value(base).expect("a browser state")
    }

    fn rect(x: f64) -> NativeBrowserBounds {
        NativeBrowserBounds {
            x,
            y: 100.0,
            width: 400.0,
            height: 300.0,
        }
    }

    fn json_str(value: impl serde::Serialize) -> String {
        serde_json::to_value(value)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    }

    fn kind_name(command: &NativeBrowserUiCommand) -> &'static str {
        match command {
            NativeBrowserUiCommand::Open => "open",
            NativeBrowserUiCommand::State => "state",
            NativeBrowserUiCommand::Control { .. } => "control",
            NativeBrowserUiCommand::Navigate { .. } => "navigate",
            NativeBrowserUiCommand::History { .. } => "history",
            NativeBrowserUiCommand::Tab { .. } => "tab",
            NativeBrowserUiCommand::Dialog { .. } => "dialog",
        }
    }

    /// The desktop app as the tests see it: it answers from a state, logs what it was asked, can hold a call until the test
    /// opens its gate, and can fail a kind of call. Each call yields once, as a promise does.
    struct Bridge {
        inner: Mutex<BridgeInner>,
    }

    struct BridgeInner {
        current: BrowserState,
        /// Every call the session made, including the ones held at the gate.
        calls: usize,
        log: Vec<String>,
        fail: Vec<&'static str>,
        gates: Vec<(&'static str, watch::Receiver<bool>)>,
    }

    fn locked<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    impl Bridge {
        fn new(current: BrowserState) -> Self {
            Self {
                inner: Mutex::new(BridgeInner {
                    current,
                    calls: 0,
                    log: Vec::new(),
                    fail: Vec::new(),
                    gates: Vec::new(),
                }),
            }
        }

        /// Holds the calls of `gate` (`control`, `navigate`, `mount` or `state`) until the returned sender is set to `true`.
        fn hold(&self, gate: &'static str) -> watch::Sender<bool> {
            let (open, wait) = watch::channel(false);
            locked(&self.inner).gates.push((gate, wait));
            open
        }

        fn gate(&self, name: &str) -> Option<watch::Receiver<bool>> {
            locked(&self.inner)
                .gates
                .iter()
                .rev()
                .find(|(gate, _)| *gate == name)
                .map(|(_, wait)| wait.clone())
        }

        fn fail_on(&self, kinds: &[&'static str]) {
            locked(&self.inner).fail = kinds.to_vec();
        }

        fn set_state(&self, state: BrowserState) {
            locked(&self.inner).current = state;
        }

        fn log(&self) -> Vec<String> {
            locked(&self.inner).log.clone()
        }

        fn clear(&self) {
            locked(&self.inner).log.clear();
        }

        fn calls(&self) -> usize {
            locked(&self.inner).calls
        }

        fn failing(&self, kind: &str) -> bool {
            locked(&self.inner).fail.contains(&kind_static(kind))
        }

        fn note_command(&self, command: NativeBrowserUiCommand) -> BrowserState {
            let mut inner = locked(&self.inner);
            let entry = match command {
                NativeBrowserUiCommand::Open => "open".to_owned(),
                NativeBrowserUiCommand::State => "state".to_owned(),
                NativeBrowserUiCommand::Control { control } => {
                    inner.current.control = control;
                    format!("control:{}", json_str(control))
                }
                NativeBrowserUiCommand::Navigate {
                    generation,
                    page_id,
                    url,
                } => format!("navigate:{generation}:{page_id}:{url}"),
                NativeBrowserUiCommand::History {
                    generation,
                    page_id,
                    action,
                } => {
                    format!("history:{generation}:{page_id}:{}", json_str(action))
                }
                NativeBrowserUiCommand::Tab {
                    generation, action, ..
                } => format!("tab:{generation}:{}", json_str(action)),
                NativeBrowserUiCommand::Dialog {
                    generation,
                    page_id,
                    accept,
                    ..
                } => {
                    inner.current.dialog = None;
                    format!("dialog:{generation}:{page_id}:{accept}")
                }
            };
            inner.log.push(entry);
            inner.current.clone()
        }
    }

    /// The `&'static str` for a kind the tests fail on.
    fn kind_static(kind: &str) -> &'static str {
        match kind {
            "control" => "control",
            "navigate" => "navigate",
            "history" => "history",
            "tab" => "tab",
            "dialog" => "dialog",
            "state" => "state",
            "open" => "open",
            _ => "mount",
        }
    }

    impl NativeBrowserBridge for Bridge {
        fn command(
            &self,
            _bot_id: &str,
            command: NativeBrowserUiCommand,
        ) -> impl Future<Output = Result<BrowserState, String>> + Send {
            let kind = kind_name(&command);
            locked(&self.inner).calls += 1;
            let failed = self.failing(kind);
            let gate = match kind {
                "control" | "navigate" | "state" => self.gate(kind),
                _ => None,
            };
            async move {
                if failed {
                    return Err(format!("failed: {kind}"));
                }
                tokio::task::yield_now().await;
                if let Some(mut gate) = gate {
                    let _ = gate.wait_for(|open| *open).await;
                }
                Ok(self.note_command(command))
            }
        }

        fn mount(
            &self,
            _bot_id: &str,
            mount: NativeBrowserMount,
        ) -> impl Future<Output = Result<BrowserState, String>> + Send {
            locked(&self.inner).calls += 1;
            let gate = self.gate("mount");
            async move {
                tokio::task::yield_now().await;
                if let Some(mut gate) = gate {
                    let _ = gate.wait_for(|open| *open).await;
                }
                let failing = locked(&self.inner).fail.contains(&"mount");
                if failing {
                    return Err("failed: mount".to_owned());
                }
                let entry = if !mount.visible {
                    "bounds:null".to_owned()
                } else if mount.occluded == Some(true) {
                    "occluded".to_owned()
                } else {
                    format!("bounds:{}", mount.bounds.x)
                };
                let mut inner = locked(&self.inner);
                inner.log.push(entry);
                Ok(inner.current.clone())
            }
        }
    }

    /// The clock the tests move by hand: `sleep` returns once the clock has reached its deadline.
    struct VirtualClock {
        now: watch::Sender<Instant>,
    }

    impl Clock for VirtualClock {
        fn now(&self) -> Instant {
            *self.now.borrow()
        }

        fn sleep(&self, duration: Duration) -> impl Future<Output = ()> + Send {
            let deadline = self.now() + duration;
            let mut wake = self.now.subscribe();
            async move {
                let _ = wake.wait_for(|now| *now >= deadline).await;
            }
        }
    }

    /// A session with its bridge and clock.
    struct Rig {
        session: NativeBrowserSession,
        bridge: Bridge,
        clock: VirtualClock,
        closes: Arc<Closes>,
    }

    impl Rig {
        fn new(initial: BrowserState) -> Self {
            let closes = Arc::new(Closes::default());
            Self {
                session: NativeBrowserSession::new("b1", closes.clone()),
                bridge: Bridge::new(initial),
                clock: VirtualClock {
                    now: watch::channel(Instant::now()).0,
                },
                closes,
            }
        }

        /// A second session for the same bot, as a reopened panel is.
        fn again(&self) -> NativeBrowserSession {
            NativeBrowserSession::new("b1", self.closes.clone())
        }

        async fn open(&self) {
            self.session.open(&self.bridge, &self.clock).await;
        }

        async fn bounds(&self, rect: Option<NativeBrowserBounds>, occluded: bool) {
            self.session
                .set_bounds(&self.bridge, &self.clock, rect, occluded)
                .await;
        }

        /// `vi.advanceTimersByTimeAsync(ms)`: the clock moves a millisecond at a time, and the mount's retry fires when due.
        async fn advance(&self, ms: u64) {
            let target = self.clock.now() + Duration::from_millis(ms);
            while self.clock.now() < target {
                let next = self.clock.now() + Duration::from_millis(1);
                self.clock.now.send_replace(next);
                for _ in 0..3 {
                    tokio::task::yield_now().await;
                }
                if self.session.retry_due().is_some_and(|due| due <= next) {
                    self.session.on_retry(&self.bridge, &self.clock).await;
                }
            }
        }
    }

    #[tokio::test]
    async fn opens_the_bots_browser_and_shows_its_state_with_no_picture() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        assert_eq!(t.bridge.log()[0], "open");
        let view = t.session.view();
        assert_eq!(
            view.state.as_ref().map(|s| (
                s.bot_id.as_str(),
                s.control,
                s.active_page_id.as_deref()
            )),
            Some(("b1", BrowserControl::Bot, Some("p1")))
        );
        assert_eq!(view.frame, None);
    }

    #[tokio::test]
    async fn follows_the_desktop_apps_state_events_for_its_own_bot_only() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.session.on_state(state(json!({ "control": "human" })));
        assert_eq!(
            t.session.view().state.map(|s| s.control),
            Some(BrowserControl::Human)
        );
        t.session
            .on_state(state(json!({ "botId": "b2", "control": "bot" })));
        assert_eq!(
            t.session.view().state.map(|s| s.control),
            Some(BrowserControl::Human)
        );
    }

    #[tokio::test]
    async fn rebinds_its_listeners_when_opened_again_without_leaking_the_old_ones() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.open().await;
        t.open().await;
        assert!(t.session.listening()); // one subscription, not one per open
        t.session.close(&t.bridge, &t.clock).await;
        assert!(!t.session.listening());
    }

    #[tokio::test]
    async fn sends_bounds_only_when_they_differ_from_what_was_accepted_and_only_the_newest_while_one_is_in_flight()
     {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.bridge.clear();
        let open = t.bridge.hold("mount");
        let before = t.bridge.calls();
        tokio::join!(
            t.bounds(Some(rect(1.0)), false), // in flight, held at the app
            async {
                while t.bridge.calls() == before {
                    tokio::task::yield_now().await;
                }
                t.bounds(Some(rect(2.0)), false).await;
                t.bounds(Some(rect(3.0)), false).await;
                t.bounds(Some(rect(3.0)), false).await;
                open.send_replace(true);
            },
        );
        assert_eq!(t.bridge.log(), ["bounds:1", "bounds:3"]);
        t.bounds(Some(rect(3.0)), false).await; // idle and the same: nothing
        t.bounds(Some(rect(3.0)), false).await;
        assert_eq!(t.bridge.log(), ["bounds:1", "bounds:3"]);
    }

    #[tokio::test]
    async fn tries_a_mount_again_after_it_fails_instead_of_treating_the_bounds_as_sent() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.advance(0).await;
        t.bridge.clear();
        t.bridge.fail_on(&["mount"]);
        t.bounds(Some(rect(7.0)), false).await;
        assert_eq!(t.session.view().error.as_deref(), Some("mount"));
        t.bridge.fail_on(&[]);
        t.bounds(Some(rect(7.0)), false).await; // too soon: it waits out the pause
        assert!(t.bridge.log().is_empty());
        t.advance(600).await;
        t.bounds(Some(rect(7.0)), false).await; // the next update retries the same bounds
        assert_eq!(t.bridge.log(), ["bounds:7"]);
        assert_eq!(t.session.view().error, None);
    }

    #[tokio::test]
    async fn temporarily_covers_the_page_without_releasing_control_then_restores_it_at_the_same_bounds()
     {
        let t = Rig::new(state(
            json!({ "control": "human", "canControl": true, "controlEpoch": 7 }),
        ));
        t.open().await;
        t.bounds(Some(rect(5.0)), false).await;
        t.bridge.clear();
        t.bounds(Some(rect(5.0)), true).await;
        t.bounds(Some(rect(5.0)), false).await;
        assert_eq!(t.bridge.log(), ["occluded", "bounds:5"]);
        let view = t.session.view();
        assert_eq!(
            view.state.as_ref().map(|s| (s.control, s.control_epoch)),
            Some((BrowserControl::Human, Some(7)))
        );
        t.session.close(&t.bridge, &t.clock).await;
        let log = t.bridge.log();
        assert!(log.contains(&"control:bot".to_owned()));
        assert!(log.contains(&"bounds:null".to_owned()));
    }

    #[tokio::test]
    async fn hides_the_view_at_once_not_behind_a_slow_navigation_and_mounts_the_latest_bounds_when_shown_again()
     {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.bounds(Some(rect(5.0)), false).await;
        t.bridge.clear();
        let slow = t.bridge.hold("navigate");
        tokio::join!(
            t.session
                .navigate(&t.bridge, &t.clock, "https://slow.test/"), // held
            async {
                for _ in 0..2 {
                    tokio::task::yield_now().await;
                }
                t.bounds(None, false).await; // the pane was hidden
                assert_eq!(t.bridge.log(), ["bounds:null"]); // hidden while the navigation is still pending
                tokio::join!(
                    t.bounds(Some(rect(8.0)), false),
                    t.bounds(Some(rect(9.0)), false)
                );
                assert_eq!(t.bridge.log(), ["bounds:null", "bounds:9"]); // shown again at the latest, not at each step
                slow.send_replace(true);
            },
        );
        assert_eq!(
            t.bridge.log().last().map(String::as_str),
            Some("navigate:g1:p1:https://slow.test/")
        );
    }

    #[tokio::test]
    async fn takes_and_gives_back_control_through_the_desktop_app() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.bridge.clear();
        t.session
            .set_control(&t.bridge, &t.clock, BrowserControl::Human)
            .await;
        t.session
            .set_control(&t.bridge, &t.clock, BrowserControl::Bot)
            .await;
        assert_eq!(t.bridge.log(), ["control:human", "control:bot"]);
    }

    #[tokio::test]
    async fn acts_on_the_generation_and_page_the_user_saw_in_the_order_they_acted() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.bridge.clear();
        t.session
            .navigate(&t.bridge, &t.clock, "https://a.test/")
            .await;
        t.session
            .history(&t.bridge, &t.clock, HistoryAction::Back)
            .await;
        t.session
            .tab(&t.bridge, &t.clock, TabAction::New, None)
            .await;
        assert_eq!(
            t.bridge.log(),
            [
                "navigate:g1:p1:https://a.test/",
                "history:g1:p1:back",
                "tab:g1:new"
            ]
        );
    }

    #[tokio::test]
    async fn answers_the_dialog_on_the_page_it_belongs_to_not_the_page_in_front() {
        let dialog =
            json!({ "pageId": "p2", "type": "confirm", "message": "Sure?", "defaultValue": "" });
        let tabs = json!([{ "id": "p1", "url": "a", "title": "A" }, { "id": "p2", "url": "b", "title": "B" }]);
        let t = Rig::new(state(
            json!({ "control": "human", "tabs": tabs, "dialog": dialog }),
        ));
        t.open().await;
        t.bridge.clear();
        t.session.dialog(&t.bridge, &t.clock, true, None).await;
        assert_eq!(t.bridge.log(), ["dialog:g1:p2:true"]);
        t.bridge.clear();
        t.session.dialog(&t.bridge, &t.clock, true, None).await; // no dialog open any more: nothing to answer
        assert!(t.bridge.log().is_empty());
    }

    #[tokio::test]
    async fn never_shows_an_older_state_than_the_one_shown_within_a_generation() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.session
            .on_state(state(json!({ "revision": 5, "control": "human" })));
        t.session
            .on_state(state(json!({ "revision": 4, "control": "bot" })));
        let view = t.session.view();
        assert_eq!(
            view.state.as_ref().map(|s| (s.revision, s.control)),
            Some((Some(5), BrowserControl::Human))
        );
        t.session.on_state(state(
            json!({ "generation": "g2", "revision": 1, "control": "bot" }),
        )); // a new generation starts its own count
        assert_eq!(
            t.session.view().state.map(|s| (s.generation, s.control)),
            Some(("g2".to_owned(), BrowserControl::Bot))
        );
    }

    #[tokio::test]
    async fn drops_a_reply_that_an_event_overtook_no_revision_and_reads_the_state_again() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.bridge.clear();
        let slow = t.bridge.hold("navigate");
        tokio::join!(
            t.session.navigate(&t.bridge, &t.clock, "https://a.test/"),
            async {
                for _ in 0..2 {
                    tokio::task::yield_now().await;
                }
                t.session.on_state(state(json!({ "control": "human" }))); // the newer word, while the reply is in flight
                t.bridge.set_state(state(json!({ "control": "bot" }))); // what the old reply will say
                slow.send_replace(true);
            },
        );
        assert!(t.bridge.log().contains(&"state".to_owned())); // asked again instead of trusting the older reply
    }

    #[tokio::test]
    async fn tells_the_panel_when_the_page_let_go_of_the_keyboard_for_its_own_bot() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        let mut returned = 0;
        for bot in ["b2", "b1"] {
            if t.session.on_focus_return(bot) {
                returned += 1;
            }
        }
        assert_eq!(returned, 1);
    }

    #[tokio::test]
    async fn suspends_from_the_desktop_apps_own_state_dismisses_the_dialog_then_hands_the_browser_back_keeping_the_session()
     {
        let dialog =
            json!({ "pageId": "p1", "type": "confirm", "message": "Sure?", "defaultValue": "" });
        let t = Rig::new(state(json!({ "control": "human", "dialog": dialog })));
        t.open().await;
        t.bridge.clear();
        t.session.suspend(&t.bridge, &t.clock).await;
        assert_eq!(
            t.bridge.log(),
            ["state", "dialog:g1:p1:false", "control:bot"]
        );
        let view = t.session.view();
        assert_eq!(
            view.state.as_ref().map(|s| (s.control, s.dialog.is_none())),
            Some((BrowserControl::Bot, true))
        );
        assert!(t.session.listening());
    }

    #[tokio::test]
    async fn suspends_a_bot_controlled_browser_without_touching_it() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.bridge.clear();
        t.session.suspend(&t.bridge, &t.clock).await;
        assert_eq!(t.bridge.log(), ["state"]);
    }

    #[tokio::test]
    async fn still_hands_control_back_when_the_state_cant_be_read_but_take_control_was_asked_for() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.session
            .set_control(&t.bridge, &t.clock, BrowserControl::Human)
            .await;
        t.bridge.clear();
        t.bridge.fail_on(&["state"]);
        t.session.suspend(&t.bridge, &t.clock).await;
        assert_eq!(t.bridge.log(), ["control:bot"]);
    }

    #[tokio::test]
    async fn hands_back_a_take_control_that_was_still_in_flight_when_it_closed_dropping_queued_actions_and_hides_the_view()
     {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.bounds(Some(rect(2.0)), false).await;
        t.bridge.clear();
        let open = t.bridge.hold("control");
        tokio::join!(
            t.session
                .set_control(&t.bridge, &t.clock, BrowserControl::Human), // in flight
            t.session
                .navigate(&t.bridge, &t.clock, "https://dropped.test/"), // queued behind it
            async {
                tokio::join!(t.session.close(&t.bridge, &t.clock), async {
                    for _ in 0..3 {
                        tokio::task::yield_now().await;
                    }
                    assert_eq!(t.bridge.log(), ["bounds:null"]); // hidden at once, before the hand-back
                    open.send_replace(true);
                },);
            },
        );
        assert_eq!(
            t.bridge.log(),
            ["bounds:null", "control:human", "state", "control:bot"]
        );
        assert!(!t.session.listening());
    }

    #[tokio::test]
    async fn closes_even_when_the_hand_back_fails_hiding_the_view() {
        let t = Rig::new(state(json!({ "control": "human" })));
        t.open().await;
        t.bounds(Some(rect(1.0)), false).await;
        t.bridge.clear();
        t.bridge.fail_on(&["control"]);
        t.session.close(&t.bridge, &t.clock).await;
        assert!(t.bridge.log().contains(&"bounds:null".to_owned()));
        assert!(!t.session.listening());
    }

    #[tokio::test]
    async fn makes_a_quick_reopen_wait_for_the_old_panels_hand_back() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.bridge.clear();
        let open = t.bridge.hold("control");
        let next = t.again();
        let before = t.bridge.calls();
        tokio::join!(
            t.session
                .set_control(&t.bridge, &t.clock, BrowserControl::Human), // keeps the old close waiting
            t.session.close(&t.bridge, &t.clock),
            async {
                // The reopen comes after the close has registered, and after the take-control has reached the app.
                while t.bridge.calls() == before || t.closes.get("b1").is_none() {
                    tokio::task::yield_now().await;
                }
                next.open(&t.bridge, &t.clock).await;
            },
            async {
                for _ in 0..4 {
                    tokio::task::yield_now().await;
                }
                assert!(!t.bridge.log().contains(&"open".to_owned())); // the reopen waits
                open.send_replace(true);
            },
        );
        let log = t.bridge.log();
        let control_bot = log
            .iter()
            .position(|e| e == "control:bot")
            .expect("the old panel hands back");
        let reopened = log
            .iter()
            .position(|e| e == "open")
            .expect("the reopen opens");
        assert!(reopened > control_bot);
    }

    #[tokio::test]
    async fn drops_a_delayed_mount_reply_from_an_older_generation_after_an_event_of_a_newer_one() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        let open = t.bridge.hold("mount");
        let before = t.bridge.calls();
        tokio::join!(
            t.bounds(Some(rect(1.0)), false), // the mount is in flight
            async {
                while t.bridge.calls() == before {
                    tokio::task::yield_now().await;
                }
                t.session.on_state(state(
                    json!({ "generation": "g2", "revision": 1, "control": "human" }),
                )); // the newer word
                t.bridge
                    .set_state(state(json!({ "generation": "g1", "revision": 5 }))); // what the delayed reply will say
                open.send_replace(true);
            },
        );
        let view = t.session.view();
        assert_eq!(
            view.state
                .as_ref()
                .map(|s| (s.generation.as_str(), s.control)),
            Some(("g2", BrowserControl::Human))
        );
    }

    #[tokio::test]
    async fn doesnt_let_a_mount_reply_without_a_revision_overwrite_a_newer_event_and_doesnt_ask_again_for_it()
     {
        let t = Rig::new(state(json!({})));
        t.open().await;
        let open = t.bridge.hold("mount");
        let before = t.bridge.calls();
        tokio::join!(t.bounds(Some(rect(1.0)), false), async {
            while t.bridge.calls() == before {
                tokio::task::yield_now().await;
            }
            t.session.on_state(state(json!({ "control": "human" })));
            t.bridge.set_state(state(json!({ "control": "bot" })));
            t.bridge.clear();
            open.send_replace(true);
        },);
        assert_eq!(
            t.session.view().state.map(|s| s.control),
            Some(BrowserControl::Human)
        );
        assert!(!t.bridge.log().contains(&"state".to_owned()));
    }

    #[tokio::test]
    async fn still_takes_a_reply_that_is_plainly_newer_the_same_generation_a_higher_revision() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        let open = t.bridge.hold("mount");
        tokio::join!(t.bounds(Some(rect(1.0)), false), async {
            tokio::task::yield_now().await;
            t.session
                .on_state(state(json!({ "revision": 3, "control": "bot" })));
            t.bridge
                .set_state(state(json!({ "revision": 4, "control": "human" })));
            open.send_replace(true);
        },);
        assert_eq!(
            t.session.view().state.map(|s| (s.revision, s.control)),
            Some((Some(4), BrowserControl::Human))
        );
    }

    #[tokio::test]
    async fn is_tried_again_by_itself_without_the_viewport_asking_again() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.advance(0).await;
        t.bounds(Some(rect(1.0)), false).await;
        t.advance(0).await;
        t.bridge.clear();
        t.bridge.fail_on(&["mount"]);
        t.bounds(None, false).await; // the viewport asks once, then stops
        assert_eq!(t.session.view().error.as_deref(), Some("mount"));
        t.bridge.fail_on(&[]);
        t.advance(600).await;
        assert_eq!(t.bridge.log(), ["bounds:null"]);
    }

    #[tokio::test]
    async fn keeps_a_close_waiting_until_the_hide_is_accepted() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.advance(0).await;
        t.bounds(Some(rect(1.0)), false).await;
        t.advance(0).await;
        t.bridge.clear();
        t.bridge.fail_on(&["mount"]);
        let closed = Cell::new(false);
        tokio::join!(
            async {
                t.session.close(&t.bridge, &t.clock).await;
                closed.set(true);
            },
            async {
                t.advance(1200).await;
                assert!(!closed.get());
                t.bridge.fail_on(&[]);
                t.advance(600).await;
            },
        );
        assert!(closed.get());
        assert!(t.bridge.log().contains(&"bounds:null".to_owned()));
        assert_eq!(t.session.hide_error(), None);
    }

    #[tokio::test]
    async fn gives_up_after_a_bounded_number_of_tries_and_says_the_page_may_still_be_showing() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.advance(0).await;
        t.bounds(Some(rect(1.0)), false).await;
        t.advance(0).await;
        t.bridge.fail_on(&["mount"]);
        let closed = Cell::new(false);
        tokio::join!(
            async {
                t.session.close(&t.bridge, &t.clock).await;
                closed.set(true);
            },
            t.advance(20_000),
        );
        assert!(closed.get());
        assert!(
            t.session
                .hide_error()
                .is_some_and(|error| error.contains("hidden"))
        );
    }

    #[tokio::test]
    async fn starts_again_after_a_close_waiting_for_it_and_keeps_its_new_listeners() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        tokio::join!(
            t.session.close(&t.bridge, &t.clock),
            t.session.open(&t.bridge, &t.clock),
        );
        assert!(t.session.listening());
        t.session.on_state(state(json!({ "control": "human" })));
        assert_eq!(
            t.session.view().state.map(|s| s.control),
            Some(BrowserControl::Human)
        );
    }

    #[tokio::test]
    async fn close_hands_the_browser_back_while_its_hide_is_still_held() {
        let t = Rig::new(state(json!({ "control": "human" })));
        t.open().await;
        t.bounds(Some(rect(1.0)), false).await;
        t.bridge.clear();
        let mounting = t.bridge.hold("mount");
        tokio::join!(t.session.close(&t.bridge, &t.clock), async {
            let mut waited = 0;
            while !t.bridge.log().contains(&"control:bot".to_owned()) && waited < 200 {
                tokio::task::yield_now().await;
                waited += 1;
            }
            assert!(
                t.bridge.log().contains(&"control:bot".to_owned()),
                "the bot gets its browser back while the hide waits"
            );
            mounting.send_replace(true);
        },);
        assert!(!t.session.listening());
    }

    #[tokio::test]
    async fn a_slow_first_mount_does_not_hold_up_a_command() {
        let t = Rig::new(state(json!({})));
        let mounting = t.bridge.hold("mount");
        let before = t.bridge.calls();
        tokio::join!(t.session.open(&t.bridge, &t.clock), async {
            let mut waited = 0;
            while t.bridge.calls() < before + 2 && waited < 200 {
                tokio::task::yield_now().await;
                waited += 1;
            }
            t.session
                .navigate(&t.bridge, &t.clock, "https://a.test/")
                .await;
            assert!(
                t.bridge
                    .log()
                    .contains(&"navigate:g1:p1:https://a.test/".to_owned()),
                "a command runs while the first mount waits"
            );
            mounting.send_replace(true);
        },);
    }

    #[tokio::test]
    async fn a_dropped_drain_or_mount_does_not_wedge_the_lanes() {
        let t = Rig::new(state(json!({ "control": "human" })));
        t.open().await;
        t.bounds(Some(rect(1.0)), false).await;
        let navigating = t.bridge.hold("navigate");
        tokio::select! {
            _ = t.session.navigate(&t.bridge, &t.clock, "https://a.test/") => {}
            _ = tokio::task::yield_now() => {}
        }
        navigating.send_replace(true);
        let mounting = t.bridge.hold("mount");
        tokio::select! {
            _ = t.bounds(Some(rect(2.0)), false) => {}
            _ = tokio::task::yield_now() => {}
        }
        mounting.send_replace(true);
        tokio::time::timeout(
            Duration::from_secs(5),
            t.session.suspend(&t.bridge, &t.clock),
        )
        .await
        .expect("the hand-back still completes");
        tokio::time::timeout(Duration::from_secs(5), t.session.close(&t.bridge, &t.clock))
            .await
            .expect("the close still completes");
        assert!(!t.session.listening());
    }

    #[tokio::test]
    async fn each_close_keeps_its_own_registry_entry_until_it_ends() {
        let t = Rig::new(state(json!({ "control": "human" })));
        t.open().await;
        let first = t.bridge.hold("state"); // close 1 waits on the app's state
        let before = t.bridge.calls();
        let second_started = Cell::new(false);
        tokio::join!(
            t.session.close(&t.bridge, &t.clock),
            async {
                while t.bridge.calls() == before {
                    tokio::task::yield_now().await;
                }
                t.session.open(&t.bridge, &t.clock).await; // reopened while close 1 is still in flight
                second_started.set(true);
                t.session.close(&t.bridge, &t.clock).await; // close 2 registers, then waits for its own hand-back
            },
            async {
                let mut waited = 0;
                while !second_started.get() && waited < 200 {
                    tokio::task::yield_now().await;
                    waited += 1;
                }
                let second = t.bridge.hold("state"); // close 2 waits on the app's state as well
                first.send_replace(true);
                let mut waited = 0;
                while !t.bridge.log().contains(&"control:bot".to_owned()) && waited < 200 {
                    tokio::task::yield_now().await;
                    waited += 1;
                }
                assert!(
                    t.closes.get("b1").is_some(),
                    "close 2 is still registered after close 1 has ended"
                );
                second.send_replace(true);
            },
        );
    }

    #[test]
    fn shows_a_state_as_the_shared_chrome_expects_it_with_no_frame() {
        let view = native_view(Some(state(json!({}))), Some("oops".to_owned()));
        assert_eq!(view.frame, None);
        assert_eq!(view.error.as_deref(), Some("oops"));
        assert_eq!(view.state.as_ref().map(|s| s.tabs.len()), Some(1));
        assert_eq!(native_view(None, None).state, None);
    }

    #[test]
    fn compares_rectangles_with_null_as_hidden() {
        assert!(same_rect(Some(&rect(1.0)), Some(&rect(1.0))));
        assert!(!same_rect(Some(&rect(1.0)), Some(&rect(2.0))));
        assert!(same_rect(None, None));
        assert!(!same_rect(Some(&rect(1.0)), None));
    }
}
