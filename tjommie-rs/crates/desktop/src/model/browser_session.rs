//! Port of `packages/ui/lib/browser-session.ts` (T1.4): one bot's browser panel: the picture, the input, the commands, and the
//! hand-back of control to the bot. Pure state plus daemon calls: every call takes `&impl Rpc`, and the timers that the
//! TypeScript sets (the picture's poll, the active window) are read through `on_timer`, which the caller drives from its own
//! clock. Stream frames arrive through `stream`; the caller subscribes while `listening()` says it should.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use client::{Rpc, RpcError};
use protocol::browser::{
    BotTarget, BrowserControl, BrowserControlParams, BrowserDialogParams, BrowserFrame,
    BrowserFrameParams, BrowserHistory, BrowserInput, BrowserInputParams, BrowserNavigate,
    BrowserResize, BrowserState, BrowserStreamAckParams, BrowserStreamFrame, BrowserStreamParams,
    BrowserTabParams, FrameCursor, HistoryAction, KeyAction, MouseAction, MouseButton, PageTarget,
    TabAction,
};
use protocol::wire::ErrorCode;
use protocol::wire::method;
use tokio::sync::watch;

use super::browser_geometry::{Point, Rect, Size};
pub use super::browser_geometry::{address_to_url, key_route, mouse_button, wheel_pixels};
use super::labels::error_text;

/// What the view shows: the state, the picture, the error, and whether a command is busy.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BrowserView {
    pub state: Option<BrowserState>,
    pub frame: Option<BrowserFrame>,
    pub error: Option<String>,
    pub busy: bool,
}

/// Closes that haven't finished handing a bot's browser back, by bot: a reopened panel waits for the last one. The
/// TypeScript keeps this in a module-level map; here the caller holds one `Closes` for the whole app.
#[derive(Default)]
pub struct Closes {
    pub(super) issued: std::sync::atomic::AtomicU64,
    entries: std::sync::Mutex<Vec<(String, u64, watch::Receiver<bool>)>>,
}

/// The wait between picture pulls while the pointer is idle, while it is active, and after a failed pull.
const IDLE_POLL: Duration = Duration::from_millis(250);
const ACTIVE_POLL: Duration = Duration::from_millis(33);
const RETRY_POLL: Duration = Duration::from_millis(1000);
/// How long a press or wheel keeps the picture polling fast.
const ACTIVE_WINDOW: Duration = Duration::from_millis(1000);

pub(super) fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Where an input is aimed: the page, and the layout and controller it was made under.
#[derive(Debug, Clone, PartialEq)]
struct Target {
    generation: String,
    page_id: String,
    viewport_revision: Option<u64>,
    control_epoch: Option<u64>,
}

/// A button that is down: the target it was aimed at, and where the pointer was.
#[derive(Debug, Clone)]
struct Held {
    target: Target,
    x: f64,
    y: f64,
}

/// A JavaScript `Map` in insertion order: `set` keeps a key's place.
struct Ordered<K, V>(Vec<(K, V)>);

impl<K, V> Default for Ordered<K, V> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<K: PartialEq + Clone, V: Clone> Ordered<K, V> {
    fn set(&mut self, key: K, value: V) {
        match self.0.iter_mut().find(|(k, _)| *k == key) {
            Some(entry) => entry.1 = value,
            None => self.0.push((key, value)),
        }
    }

    fn get(&self, key: &K) -> Option<&V> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn delete(&mut self, key: &K) -> Option<V> {
        let index = self.0.iter().position(|(k, _)| k == key)?;
        Some(self.0.remove(index).1)
    }

    fn keys(&self) -> Vec<K> {
        self.0.iter().map(|(k, _)| k.clone()).collect()
    }

    fn entries(&self) -> Vec<(K, V)> {
        self.0.clone()
    }

    fn clear(&mut self) {
        self.0.clear();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Move,
    Wheel,
    Resize,
}

/// A command the page takes, with the params captured when the user acted.
enum Command {
    Control(BrowserControlParams),
    Navigate(BrowserNavigate),
    History(BrowserHistory),
    Tab(BrowserTabParams),
    Resize(BrowserResize),
    Dialog(BrowserDialogParams),
}

enum Job {
    /// Waits for an earlier close of this bot, then reads the browser and shows it.
    Open {
        earlier: Option<watch::Receiver<bool>>,
    },
    Input {
        target: Target,
        input: BrowserInput,
    },
    Command(Command),
    /// Hands the browser back to the bot and leaves the view hidden: `suspend`.
    Suspend,
    /// Hands the browser back, then ends the session: `close`.
    Close,
}

/// A unit of queued work, run in order by whichever call found the lane idle.
struct Task {
    job: Job,
    /// A release runs even while closing, and survives a close's discard.
    release: bool,
    /// Moves and wheels with the same key merge; resizes with the same key replace each other.
    kind: Option<Kind>,
    key: Option<String>,
}

impl Task {
    fn is_command(&self) -> bool {
        matches!(self.job, Job::Command(_))
    }
}

/// A call that is not awaited for its answer: an acknowledgement, or the live stream turned on or off.
enum Fire {
    Ack(u64),
    Stream { on: bool, token: u64 },
}

/// What a state update changes. A field left `None` keeps the view's value.
#[derive(Default)]
struct Patch {
    state: Option<BrowserState>,
    frame: Option<Option<BrowserFrame>>,
    error: Option<Option<String>>,
}

impl Patch {
    fn error(error: &impl std::fmt::Display) -> Self {
        Self {
            error: Some(Some(error_text(error))),
            ..Self::default()
        }
    }
}

/// The session's state. Every step that needs no daemon call happens here, under the lock, and never awaits.
#[derive(Default)]
struct Inner {
    view: BrowserView,
    changes: u64,
    tasks: VecDeque<Task>,
    running: bool,
    fences: u32,
    epoch: u64,
    cursor: Option<FrameCursor>,
    visible: bool,
    suspended: bool,
    suspending: Option<watch::Sender<bool>>,
    closing: bool,
    done: Option<watch::Sender<bool>>,
    human: bool,
    asked_human: bool,
    /// Keys and buttons pressed by the user, not yet released.
    keys: Ordered<String, Target>,
    buttons: Ordered<MouseButton, Held>,
    /// Keys and buttons the daemon has taken down, which a hand-back releases.
    held_keys: Ordered<String, Target>,
    held_buttons: Ordered<MouseButton, Held>,
    /// The live stream's subscription token while it is on.
    stream: Option<u64>,
    stream_token: u64,
    unsupported: bool,
    live: bool,
    /// The streamed frame handed to the view, acknowledged once drawn, replaced or dropped.
    awaiting: Option<BrowserFrame>,
    behind: bool,
    frame_pending: bool,
    timer: Option<Instant>,
    active_until: Option<Instant>,
    fires: VecDeque<Fire>,
}

/// Whether a frame is of the page, layout, controller and private session the state describes.
fn fits(frame: &BrowserFrame, state: &BrowserState) -> bool {
    (frame.page_id.as_deref().is_none_or(|page| page.is_empty())
        || frame.page_id == state.active_page_id)
        && frame
            .control_epoch
            .is_none_or(|epoch| state.control_epoch == Some(epoch))
        && frame
            .private_epoch
            .is_none_or(|epoch| state.private_epoch == Some(epoch))
        && frame
            .viewport_revision
            .is_none_or(|revision| state.viewport_revision == Some(revision))
}

/// The same picture: every field that matters, but not the sequence, so an unchanged picture keeps its object.
fn same_picture(a: &BrowserFrame, b: &BrowserFrame) -> bool {
    a.data == b.data
        && a.width == b.width
        && a.height == b.height
        && a.viewport_width == b.viewport_width
        && a.viewport_height == b.viewport_height
        && a.control_epoch == b.control_epoch
        && a.private_epoch == b.private_epoch
        && a.page_id == b.page_id
        && a.viewport_revision == b.viewport_revision
        && a.stream_id == b.stream_id
}

fn target_key(target: &Target) -> String {
    format!(
        "{}:{}:{}:{}",
        target.generation,
        target.page_id,
        target
            .viewport_revision
            .map_or(String::new(), |v| v.to_string()),
        target
            .control_epoch
            .map_or(String::new(), |v| v.to_string())
    )
}

/// Adds a wheel to the one queued before it, when the deltas stay within the daemon's limit.
fn merge_wheel(queued: &mut BrowserInput, next: &BrowserInput) -> bool {
    match (queued, next) {
        (
            BrowserInput::Wheel {
                x,
                y,
                delta_x,
                delta_y,
            },
            BrowserInput::Wheel {
                x: next_x,
                y: next_y,
                delta_x: next_dx,
                delta_y: next_dy,
            },
        ) if *x == *next_x
            && *y == *next_y
            && (*delta_x + next_dx).abs() <= 10_000.0
            && (*delta_y + next_dy).abs() <= 10_000.0 =>
        {
            *delta_x += next_dx;
            *delta_y += next_dy;
            true
        }
        _ => false,
    }
}

fn is_unknown_method(error: &RpcError) -> bool {
    matches!(
        error,
        RpcError::Daemon {
            code: ErrorCode::UnknownMethod,
            ..
        }
    )
}

pub(super) async fn wait(latch: Option<watch::Receiver<bool>>) {
    if let Some(mut latch) = latch {
        let _ = latch.wait_for(|done| *done).await;
    }
}

impl Inner {
    fn fresh(&self, state: &BrowserState) -> bool {
        match &self.view.state {
            None => true,
            Some(before) => {
                before.generation != state.generation
                    || before.revision.is_none()
                    || state.revision.is_none()
                    || state.revision >= before.revision
            }
        }
    }

    fn apply(&mut self, state: BrowserState) {
        if self.fresh(&state) {
            self.set(Patch {
                state: Some(state),
                error: Some(None),
                ..Patch::default()
            });
        }
    }

    /// The view's one update. A changed view is counted, for the caller's redraw.
    fn set(&mut self, patch: Patch) {
        if let Some(state) = &patch.state {
            self.human = state.control == BrowserControl::Human;
            if !self.human {
                self.asked_human = false;
            }
            if state.private_active == Some(true) {
                self.stop_stream();
            }
        }
        if self.closing {
            return;
        }
        let before = self.view.clone();
        let state = patch.state.or_else(|| before.state.clone());
        let moved = state.as_ref().is_some_and(|next| {
            before.state.as_ref().is_none_or(|old| {
                next.generation != old.generation
                    || next.active_page_id != old.active_page_id
                    || next.viewport_revision != old.viewport_revision
            })
        });
        if moved {
            self.cursor = None;
        }
        let private = state
            .as_ref()
            .is_some_and(|s| s.private_active == Some(true));
        let frame = if private {
            None
        } else if let Some(frame) = patch.frame {
            frame
        } else if moved {
            None
        } else {
            before.frame.clone()
        };
        if self
            .awaiting
            .as_ref()
            .is_some_and(|awaiting| frame.as_ref() != Some(awaiting))
        {
            self.ack(); // replaced before it was drawn
        }
        let busy = self.fences > 0
            || self.suspending.is_some()
            || state.as_ref().is_some_and(|s| s.busy == Some(true));
        let error = match patch.error {
            Some(error) => error,
            None => before.error.clone(),
        };
        let next = BrowserView {
            state,
            frame,
            error,
            busy,
        };
        if next == before {
            return;
        }
        self.view = next;
        self.changes += 1;
    }

    fn ack(&mut self) {
        if let Some(frame) = self.awaiting.take() {
            self.fires.push_back(Fire::Ack(frame.sequence));
        }
    }

    fn end_stream(&mut self) {
        self.stream = None;
        self.ack();
    }

    fn stop_stream(&mut self) {
        if self.stream.is_none() {
            return;
        }
        self.end_stream();
        self.fires.push_back(Fire::Stream {
            on: false,
            token: 0,
        });
    }

    fn start_stream(&mut self) {
        if !self.live || self.stream.is_some() || self.unsupported {
            return;
        }
        self.stream_token += 1;
        let token = self.stream_token;
        self.stream = Some(token);
        self.fires.push_back(Fire::Stream { on: true, token });
    }

    /// The stream keeps the picture current, so the poll reads only the state.
    fn current(&self) -> bool {
        self.stream.is_some()
            && !self.behind
            && self
                .view
                .frame
                .as_ref()
                .zip(self.view.state.as_ref())
                .is_some_and(|(frame, state)| fits(frame, state))
    }

    fn stop_timer(&mut self) {
        self.timer = None;
    }

    /// Sets the picture timer, unless a pull is out or the session is not shown.
    fn poll(&mut self, now: Instant, delay: Duration) {
        if self.closing
            || self.suspended
            || self.suspending.is_some()
            || !self.visible
            || self.frame_pending
            || self.timer.is_some()
            || self.view.state.is_none()
        {
            return;
        }
        self.timer = Some(now + delay);
    }

    fn wake(&mut self, now: Instant) {
        if self.current() {
            return;
        }
        self.active_until = Some(now + ACTIVE_WINDOW);
        self.stop_timer();
        self.poll(now, Duration::ZERO);
    }

    /// A command's fence comes down: its reply (or its cancel) is applied and the picture may poll again.
    fn finish(&mut self, now: Instant) {
        self.fences = self.fences.saturating_sub(1);
        self.epoch += 1;
        self.set(Patch::default());
        self.wake(now);
    }

    /// A close or suspend drops the presses not yet sent, and cancels the commands among them.
    fn discard_presses(&mut self, now: Instant) {
        let cancelled = self
            .tasks
            .iter()
            .filter(|task| !task.release && task.is_command())
            .count();
        for _ in 0..cancelled {
            self.finish(now);
        }
        self.tasks.retain(|task| task.release);
        self.keys.clear();
        self.buttons.clear();
    }

    /// Queues a task. Returns whether the caller must run the queue (no one else is).
    fn submit(&mut self, task: Task, now: Instant) -> bool {
        if self.closing && !task.release {
            if task.is_command() {
                self.finish(now);
            }
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

    fn command(
        &mut self,
        command: Command,
        reshape: bool,
        kind: Option<Kind>,
        key: Option<String>,
        now: Instant,
    ) -> bool {
        if self.closing || self.suspended {
            return false;
        }
        self.epoch += 1;
        self.fences += 1;
        if reshape {
            self.cursor = None;
        }
        self.set(if reshape {
            Patch {
                frame: Some(None),
                ..Patch::default()
            }
        } else {
            Patch::default()
        });
        let task = Task {
            job: Job::Command(command),
            release: false,
            kind,
            key,
        };
        let replaces = kind == Some(Kind::Resize)
            && self
                .tasks
                .back()
                .is_some_and(|last| last.kind == Some(Kind::Resize) && last.key == task.key);
        if replaces {
            self.finish(now); // the replaced resize is cancelled
            if let Some(last) = self.tasks.back_mut() {
                *last = task;
            }
            return false;
        }
        self.submit(task, now)
    }

    /// A command at the page the user is looking at (`#onPage`).
    fn on_page(
        &mut self,
        bot: &str,
        make: impl FnOnce(PageTarget) -> Command,
        reshape: bool,
        kind: Option<Kind>,
        now: Instant,
    ) -> bool {
        let Some(state) = self.view.state.clone() else {
            return false;
        };
        let Some(page_id) = state.active_page_id.clone().filter(|page| !page.is_empty()) else {
            return false;
        };
        let key = format!("{}:{}", state.generation, page_id);
        let page = PageTarget {
            bot_id: bot.to_owned(),
            generation: state.generation.clone(),
            control_epoch: state.control_epoch,
            page_id,
        };
        self.command(make(page), reshape, kind, Some(key), now)
    }

    /// Accepts a user's input if it is for the page, then queues it (`input`). Returns whether the caller must run the queue.
    fn prepare_input(&mut self, input: BrowserInput, now: Instant) -> bool {
        let Some(state) = self.view.state.clone() else {
            return false;
        };
        let Some(page_id) = state.active_page_id.clone() else {
            return false;
        };
        if state.control != BrowserControl::Human
            || state.can_control == Some(false)
            || state.private_active == Some(true)
            || self.closing
        {
            return false;
        }
        let release = matches!(
            input,
            BrowserInput::Key {
                action: KeyAction::Up,
                ..
            } | BrowserInput::Mouse {
                action: MouseAction::Up,
                ..
            }
        );
        if !release && (self.view.frame.is_none() || self.view.busy || self.suspended) {
            return false;
        }
        if !release
            && self.view.frame.as_ref().is_some_and(|frame| {
                frame
                    .control_epoch
                    .is_some_and(|epoch| state.control_epoch != Some(epoch))
                    || frame
                        .private_epoch
                        .is_some_and(|epoch| state.private_epoch != Some(epoch))
            })
        {
            return false;
        }
        let mut target = Target {
            generation: state.generation.clone(),
            page_id,
            viewport_revision: state.viewport_revision,
            control_epoch: state.control_epoch,
        };
        if let BrowserInput::Key { action, key } = &input {
            match action {
                KeyAction::Down => self.keys.set(key.clone(), target.clone()),
                KeyAction::Up => match self.keys.delete(key) {
                    Some(held) => target = held,
                    None => return false,
                },
            }
        }
        if let BrowserInput::Mouse {
            action,
            x,
            y,
            button: Some(button),
        } = &input
        {
            match action {
                MouseAction::Down => self.buttons.set(
                    *button,
                    Held {
                        target: target.clone(),
                        x: *x,
                        y: *y,
                    },
                ),
                MouseAction::Up => {
                    if let Some(held) = self.buttons.get(button) {
                        target = held.target.clone();
                    }
                    self.buttons.delete(button);
                }
                MouseAction::Move => {}
            }
        }
        let kind = match &input {
            BrowserInput::Wheel { .. } => Some(Kind::Wheel),
            BrowserInput::Mouse {
                action: MouseAction::Move,
                ..
            } => Some(Kind::Move),
            _ => None,
        };
        let key = Some(target_key(&target));
        let mut pending = Some(Task {
            job: Job::Input {
                target,
                input: input.clone(),
            },
            release,
            kind,
            key: key.clone(),
        });
        if let Some(last) = self.tasks.back_mut() {
            let same_key = last.kind == kind && last.key == key;
            if same_key && kind == Some(Kind::Move) {
                if let Some(task) = pending.take() {
                    *last = task; // only the newest of queued moves is sent
                }
            } else if same_key && kind == Some(Kind::Wheel) {
                let merged = match &mut last.job {
                    Job::Input { input: queued, .. } => merge_wheel(queued, &input),
                    _ => false,
                };
                if merged {
                    pending = None;
                }
            }
        }
        let drain = match pending {
            Some(task) => self.submit(task, now),
            None => false,
        };
        self.wake(now);
        drain
    }

    /// Records what the daemon took down or released, after a successful send.
    fn note_sent(&mut self, target: &Target, input: &BrowserInput) {
        match input {
            BrowserInput::Key {
                action: KeyAction::Down,
                key,
            } => self.held_keys.set(key.clone(), target.clone()),
            BrowserInput::Key {
                action: KeyAction::Up,
                key,
            } => {
                self.held_keys.delete(key);
            }
            BrowserInput::Mouse {
                action: MouseAction::Down,
                x,
                y,
                button: Some(button),
            } => self.held_buttons.set(
                *button,
                Held {
                    target: target.clone(),
                    x: *x,
                    y: *y,
                },
            ),
            BrowserInput::Mouse {
                action: MouseAction::Up,
                button: Some(button),
                ..
            } => {
                self.held_buttons.delete(button);
            }
            _ => {}
        }
    }

    /// A streamed frame from the desktop: shown when it fits the view, else held as behind.
    fn streamed(&mut self, bot: &str, data: BrowserStreamFrame) {
        if data.bot_id != bot {
            return;
        }
        let Some(frame) = data.frame else {
            self.end_stream(); // it ended: a private interaction, a closed browser, or a stop
            return;
        };
        self.ack(); // the frame before it goes first
        self.awaiting = Some(frame.clone());
        let shows = self.fences == 0
            && self
                .view
                .state
                .as_ref()
                .is_some_and(|state| fits(&frame, state));
        if shows {
            self.behind = false;
            self.set(Patch {
                frame: Some(Some(frame)),
                ..Patch::default()
            });
        } else {
            self.behind = true;
            self.ack();
        }
    }

    /// Starts a suspend: the browser is hidden now, and the hand-back runs in the lane. Returns the latch to wait on, and
    /// whether the caller must run the queue.
    fn begin_suspend(&mut self, now: Instant) -> (Option<watch::Receiver<bool>>, bool) {
        if self.closing {
            return (self.done.as_ref().map(|done| done.subscribe()), false);
        }
        self.visible = false;
        self.suspended = true;
        self.epoch += 1;
        self.stop_timer();
        self.stop_stream();
        self.discard_presses(now);
        if let Some(pending) = &self.suspending {
            return (Some(pending.subscribe()), false);
        }
        let (pending, waiter) = watch::channel(false);
        self.suspending = Some(pending);
        self.set(Patch {
            frame: Some(None),
            ..Patch::default()
        });
        let drain = self.submit(
            Task {
                job: Job::Suspend,
                release: true,
                kind: None,
                key: None,
            },
            now,
        );
        (Some(waiter), drain)
    }

    /// Starts a close: the picture stops, the stream stops, and the hand-back is queued as a release.
    fn begin_close(
        &mut self,
        closes: &Closes,
        bot: &str,
        token: u64,
        now: Instant,
    ) -> (Option<watch::Receiver<bool>>, bool) {
        if let Some(done) = &self.done {
            return (Some(done.subscribe()), false);
        }
        self.stop_timer();
        self.stop_stream();
        self.epoch += 1;
        self.discard_presses(now);
        let (done, waiter) = watch::channel(false);
        closes.put(bot, token, waiter.clone());
        self.done = Some(done);
        self.closing = true;
        let drain = self.submit(
            Task {
                job: Job::Close,
                release: true,
                kind: None,
                key: None,
            },
            now,
        );
        (Some(waiter), drain)
    }
}

impl Closes {
    pub(super) fn put(&self, bot: &str, token: u64, done: watch::Receiver<bool>) {
        let mut entries = locked(&self.entries);
        entries.retain(|(name, _, _)| name != bot);
        entries.push((bot.to_owned(), token, done));
    }

    pub(super) fn get(&self, bot: &str) -> Option<watch::Receiver<bool>> {
        locked(&self.entries)
            .iter()
            .find(|(name, _, _)| name == bot)
            .map(|(_, _, done)| done.clone())
    }

    /// Removes the bot's entry, if it is still this close's.
    pub(super) fn drop_if(&self, bot: &str, token: u64) {
        locked(&self.entries).retain(|(name, held, _)| !(name == bot && *held == token));
    }
}

/// One bot's browser panel. Every call takes the daemon (`&impl Rpc`) and the instant it happens at. Calls that find the lane
/// busy return at once: the call that is running the lane runs their work too, as the TypeScript's pump does.
pub struct BrowserSession {
    bot_id: String,
    closes: Arc<Closes>,
    token: u64,
    inner: Mutex<Inner>,
}

impl BrowserSession {
    /// A session for `bot_id`. `live` says whether the connection offers the live stream (the TypeScript's `listen`).
    pub fn new(bot_id: &str, closes: Arc<Closes>, live: bool) -> Self {
        let token = closes.issued.fetch_add(1, Ordering::Relaxed);
        Self {
            bot_id: bot_id.to_owned(),
            closes,
            token,
            inner: Mutex::new(Inner {
                visible: true,
                live,
                ..Inner::default()
            }),
        }
    }

    fn with<T>(&self, step: impl FnOnce(&mut Inner) -> T) -> T {
        step(&mut locked(&self.inner))
    }

    pub fn view(&self) -> BrowserView {
        self.with(|s| s.view.clone())
    }

    /// How many times the view has changed: the caller redraws when it moves.
    pub fn changes(&self) -> u64 {
        self.with(|s| s.changes)
    }

    /// Whether the live stream should be subscribed to: the caller listens while this is true.
    pub fn listening(&self) -> bool {
        self.with(|s| s.stream.is_some())
    }

    /// When the picture timer is due. The caller calls `on_timer` at that instant.
    pub fn next_timer(&self) -> Option<Instant> {
        self.with(|s| s.timer)
    }

    fn bot(&self) -> BotTarget {
        BotTarget {
            bot_id: self.bot_id.clone(),
        }
    }

    fn page(&self, target: &Target) -> PageTarget {
        PageTarget {
            bot_id: self.bot_id.clone(),
            generation: target.generation.clone(),
            control_epoch: target.control_epoch,
            page_id: target.page_id.clone(),
        }
    }

    /// Runs the lane: each queued task in order, until the queue is empty.
    async fn drain(&self, rpc: &impl Rpc, now: Instant) {
        while let Some(task) = self.with(|s| s.next_task()) {
            if let Err(error) = self.run(rpc, task, now).await {
                self.with(|s| s.set(Patch::error(&error)));
            }
        }
        self.flush(rpc).await;
    }

    async fn run(&self, rpc: &impl Rpc, task: Task, now: Instant) -> Result<(), RpcError> {
        match task.job {
            Job::Open { earlier } => {
                wait(earlier).await;
                let state = rpc.call::<method::BrowserOpen>(self.bot()).await?;
                self.with(|s| {
                    s.apply(state);
                    s.poll(now, Duration::ZERO);
                });
                Ok(())
            }
            Job::Input { target, input } => {
                self.send(rpc, &target, input).await?;
                self.with(|s| s.wake(now));
                Ok(())
            }
            Job::Command(command) => {
                let state = self.command_call(rpc, command).await;
                self.with(|s| {
                    if let Ok(state) = &state {
                        s.apply(state.clone());
                    }
                    s.finish(now);
                });
                state.map(|_| ())
            }
            Job::Suspend => {
                let outcome = self.hand_back(rpc).await;
                self.with(|s| {
                    if let Err(error) = &outcome {
                        s.set(Patch::error(error));
                    }
                    let pending = s.suspending.take();
                    s.epoch += 1;
                    s.set(Patch::default());
                    if let Some(pending) = pending {
                        pending.send_replace(true);
                    }
                    s.poll(now, Duration::ZERO);
                });
                Ok(())
            }
            Job::Close => {
                let _ = self.hand_back(rpc).await; // best effort at a terminal close
                self.flush(rpc).await;
                self.with(|s| {
                    if let Some(done) = &s.done {
                        done.send_replace(true);
                    }
                });
                self.closes.drop_if(&self.bot_id, self.token);
                Ok(())
            }
        }
    }

    async fn command_call(
        &self,
        rpc: &impl Rpc,
        command: Command,
    ) -> Result<BrowserState, RpcError> {
        match command {
            Command::Control(params) => rpc.call::<method::BrowserControl>(params).await,
            Command::Navigate(params) => rpc.call::<method::BrowserNavigate>(params).await,
            Command::History(params) => rpc.call::<method::BrowserHistory>(params).await,
            Command::Tab(params) => rpc.call::<method::BrowserTab>(params).await,
            Command::Resize(params) => rpc.call::<method::BrowserResize>(params).await,
            Command::Dialog(params) => rpc.call::<method::BrowserDialog>(params).await,
        }
    }

    async fn send(
        &self,
        rpc: &impl Rpc,
        target: &Target,
        input: BrowserInput,
    ) -> Result<(), RpcError> {
        let params = BrowserInputParams {
            page: self.page(target),
            input: input.clone(),
            viewport_revision: target.viewport_revision,
        };
        rpc.call::<method::BrowserInput>(params).await?;
        self.with(|s| s.note_sent(target, &input));
        Ok(())
    }

    /// Dismisses an open page dialog, releases what the daemon holds down, and gives the browser back to the bot. Only when
    /// the user has it (from the daemon's word, or what was asked for when the answer can't be had).
    async fn hand_back(&self, rpc: &impl Rpc) -> Result<(), RpcError> {
        let state = rpc.call::<method::BrowserOpen>(self.bot()).await.ok();
        if let Some(state) = &state {
            self.with(|s| s.apply(state.clone()));
        }
        let human = match &state {
            Some(state) => {
                state.control == BrowserControl::Human && state.can_control != Some(false)
            }
            None => self.with(|s| s.human || s.asked_human),
        };
        if !human {
            return Ok(());
        }
        if let Some(state) = state.as_ref().filter(|state| {
            state
                .active_page_id
                .as_deref()
                .is_some_and(|page| !page.is_empty())
        }) {
            if let Some(dialog) = &state.dialog {
                let params = BrowserDialogParams {
                    page: PageTarget {
                        bot_id: self.bot_id.clone(),
                        generation: state.generation.clone(),
                        control_epoch: state.control_epoch,
                        page_id: dialog.page_id.clone(),
                    },
                    accept: false,
                    prompt_text: None,
                };
                let _ = rpc.call::<method::BrowserDialog>(params).await;
            }
            for (button, held) in self.with(|s| s.held_buttons.entries()) {
                let input = BrowserInput::Mouse {
                    action: MouseAction::Up,
                    x: held.x,
                    y: held.y,
                    button: Some(button),
                };
                let _ = self.send(rpc, &held.target, input).await;
            }
            for (key, target) in self.with(|s| s.held_keys.entries()) {
                let input = BrowserInput::Key {
                    action: KeyAction::Up,
                    key,
                };
                let _ = self.send(rpc, &target, input).await;
            }
        }
        let params = BrowserControlParams {
            bot_id: self.bot_id.clone(),
            control: BrowserControl::Bot,
            generation: state.as_ref().map(|s| s.generation.clone()),
            control_epoch: state.as_ref().and_then(|s| s.control_epoch),
            release_only: Some(true),
        };
        let result = rpc.call::<method::BrowserControl>(params).await?;
        self.with(|s| {
            s.apply(result);
            s.held_keys.clear();
            s.held_buttons.clear();
        });
        Ok(())
    }

    /// Runs the fire-and-forget calls in order: acknowledgements, and the stream turned on or off.
    async fn flush(&self, rpc: &impl Rpc) {
        while let Some(fire) = self.with(|s| s.fires.pop_front()) {
            match fire {
                Fire::Ack(sequence) => {
                    let params = BrowserStreamAckParams {
                        bot_id: self.bot_id.clone(),
                        sequence,
                    };
                    let _ = rpc.call::<method::BrowserStreamAck>(params).await;
                }
                Fire::Stream { on: false, .. } => {
                    let _ = self.stream_call(rpc, false).await;
                }
                Fire::Stream { on: true, token } => match self.stream_call(rpc, true).await {
                    Ok(state) => self.with(|s| {
                        if s.stream == Some(token) {
                            s.apply(state);
                        }
                    }),
                    Err(error) => self.with(|s| {
                        if s.stream == Some(token) {
                            s.end_stream();
                            s.unsupported = is_unknown_method(&error);
                        }
                    }),
                },
            }
        }
    }

    async fn stream_call(&self, rpc: &impl Rpc, on: bool) -> Result<BrowserState, RpcError> {
        let params = BrowserStreamParams {
            bot_id: self.bot_id.clone(),
            on,
            max_fps: None,
        };
        rpc.call::<method::BrowserStream>(params).await
    }

    /// Opens the browser's view: the first state, then the first picture.
    pub async fn open(&self, rpc: &impl Rpc, now: Instant) {
        let earlier = self.closes.get(&self.bot_id);
        let drain = self.with(|s| {
            s.submit(
                Task {
                    job: Job::Open { earlier },
                    release: false,
                    kind: None,
                    key: None,
                },
                now,
            )
        });
        if drain {
            self.drain(rpc, now).await;
        }
    }

    /// Shows or hides the panel. Hiding releases this device's input and control, and stops the pictures.
    pub async fn set_visible(&self, rpc: &impl Rpc, now: Instant, visible: bool) {
        self.with(|s| s.visible = visible);
        if visible {
            self.with(|s| {
                s.suspended = false;
                s.poll(now, Duration::ZERO);
            });
        } else {
            self.suspend(rpc, now).await;
        }
    }

    /// Hides the panel's pictures and hands the browser back to the bot. Resolves when the hand-back is done.
    pub async fn suspend(&self, rpc: &impl Rpc, now: Instant) {
        let (waiter, drain) = self.with(|s| s.begin_suspend(now));
        if drain {
            self.drain(rpc, now).await;
        }
        wait(waiter).await;
    }

    /// Ends the session: the picture and stream stop, presses are dropped, and the browser goes back to the bot (it stays
    /// open). Resolves when the hand-back is done.
    pub async fn close(&self, rpc: &impl Rpc, now: Instant) {
        let (waiter, drain) =
            self.with(|s| s.begin_close(&self.closes, &self.bot_id, self.token, now));
        if drain {
            self.drain(rpc, now).await;
        }
        wait(waiter).await;
    }

    /// The view drew this picture, so the stream may send the next.
    pub async fn drawn(&self, rpc: &impl Rpc, frame: &BrowserFrame) {
        self.with(|s| {
            if s.awaiting.as_ref() == Some(frame) {
                s.ack();
            }
        });
        self.flush(rpc).await;
    }

    /// A key, mouse, wheel or text input from the user.
    pub async fn input(&self, rpc: &impl Rpc, now: Instant, input: BrowserInput) {
        let drain = self.with(|s| s.prepare_input(input, now));
        if drain {
            self.drain(rpc, now).await;
        }
    }

    /// Releases every key and button the user holds (leaving the Browser pane, or the window losing focus).
    pub async fn release_all(&self, rpc: &impl Rpc, now: Instant) {
        let (buttons, keys) = self.with(|s| (s.buttons.entries(), s.keys.keys()));
        for (button, held) in buttons {
            let input = BrowserInput::Mouse {
                action: MouseAction::Up,
                x: held.x,
                y: held.y,
                button: Some(button),
            };
            self.input(rpc, now, input).await;
        }
        for key in keys {
            self.input(
                rpc,
                now,
                BrowserInput::Key {
                    action: KeyAction::Up,
                    key,
                },
            )
            .await;
        }
    }

    /// Takes or gives back control. The reply's state is shown.
    pub async fn set_control(&self, rpc: &impl Rpc, now: Instant, control: BrowserControl) {
        let drain = self.with(|s| {
            if s.suspended || s.closing {
                return false;
            }
            if control == BrowserControl::Human {
                s.asked_human = true;
            }
            let state = s.view.state.clone();
            let params = BrowserControlParams {
                bot_id: self.bot_id.clone(),
                control,
                generation: state.as_ref().map(|st| st.generation.clone()),
                control_epoch: state.as_ref().and_then(|st| st.control_epoch),
                release_only: None,
            };
            s.command(Command::Control(params), false, None, None, now)
        });
        if drain {
            self.drain(rpc, now).await;
        }
    }

    pub async fn navigate(&self, rpc: &impl Rpc, now: Instant, url: &str) {
        let drain = self.with(|s| {
            s.on_page(
                &self.bot_id,
                |page| {
                    Command::Navigate(BrowserNavigate {
                        page,
                        url: url.to_owned(),
                    })
                },
                true,
                None,
                now,
            )
        });
        if drain {
            self.drain(rpc, now).await;
        }
    }

    pub async fn history(&self, rpc: &impl Rpc, now: Instant, action: HistoryAction) {
        let drain = self.with(|s| {
            s.on_page(
                &self.bot_id,
                |page| Command::History(BrowserHistory { page, action }),
                true,
                None,
                now,
            )
        });
        if drain {
            self.drain(rpc, now).await;
        }
    }

    pub async fn resize(&self, rpc: &impl Rpc, now: Instant, width: u32, height: u32) {
        let drain = self.with(|s| {
            s.on_page(
                &self.bot_id,
                |page| {
                    Command::Resize(BrowserResize {
                        page,
                        width,
                        height,
                    })
                },
                true,
                Some(Kind::Resize),
                now,
            )
        });
        if drain {
            self.drain(rpc, now).await;
        }
    }

    /// Answers the page's open dialog, on the page it belongs to.
    pub async fn dialog(
        &self,
        rpc: &impl Rpc,
        now: Instant,
        accept: bool,
        prompt_text: Option<&str>,
    ) {
        let drain = self.with(|s| {
            s.on_page(
                &self.bot_id,
                |page| {
                    Command::Dialog(BrowserDialogParams {
                        page,
                        accept,
                        prompt_text: prompt_text.map(str::to_owned),
                    })
                },
                false,
                None,
                now,
            )
        });
        if drain {
            self.drain(rpc, now).await;
        }
    }

    pub async fn tab(
        &self,
        rpc: &impl Rpc,
        now: Instant,
        action: TabAction,
        page_id: Option<&str>,
    ) {
        let drain = self.with(|s| {
            let Some(state) = s.view.state.clone() else {
                return false;
            };
            if state.generation.is_empty() {
                return false;
            }
            let params = BrowserTabParams {
                bot_id: self.bot_id.clone(),
                generation: state.generation.clone(),
                control_epoch: state.control_epoch,
                action,
                page_id: page_id.map(str::to_owned),
            };
            s.command(Command::Tab(params), true, None, None, now)
        });
        if drain {
            self.drain(rpc, now).await;
        }
    }

    /// The picture timer ran out: one pull of the picture or the state, then the next timer.
    pub async fn on_timer(&self, rpc: &impl Rpc, now: Instant) {
        let epoch = self.with(|s| {
            s.timer = None;
            s.frame_pending = true;
            s.epoch
        });
        let outcome = self.pull(rpc, epoch).await;
        self.with(|s| {
            let failure = outcome.as_ref().err().filter(|_| epoch == s.epoch);
            if let Some(error) = failure {
                s.set(Patch::error(error));
            }
            s.frame_pending = false;
            let delay = if outcome.is_err() {
                RETRY_POLL
            } else if s.stream.is_none()
                && (s.active_until.is_some_and(|until| now < until) || s.view.busy)
            {
                ACTIVE_POLL
            } else {
                IDLE_POLL
            };
            s.poll(now, delay);
        });
        self.flush(rpc).await;
    }

    async fn pull(&self, rpc: &impl Rpc, epoch: u64) -> Result<(), RpcError> {
        if self.with(|s| s.current()) {
            // The tabs, address, dialog and control; the pictures come with the stream.
            let state = rpc.call::<method::BrowserOpen>(self.bot()).await?;
            self.with(|s| {
                if !s.closing && s.visible && epoch == s.epoch && s.fences == 0 {
                    s.apply(state);
                }
            });
            return Ok(());
        }
        let (shown, cursor) = self.with(|s| (s.view.frame.clone(), s.cursor.clone()));
        let result = rpc
            .call::<method::BrowserFrame>(BrowserFrameParams {
                bot_id: self.bot_id.clone(),
                thumbnail: None,
                after: cursor,
            })
            .await?;
        self.with(|s| {
            if s.closing
                || !s.visible
                || epoch != s.epoch
                || s.fences > 0
                || !s.fresh(&result.state)
            {
                return;
            }
            let mut frame = if result.unchanged == Some(true) {
                s.view.frame.clone()
            } else {
                result.frame.clone()
            };
            if s.stream.is_some() && s.view.frame != shown {
                frame = s.view.frame.clone(); // a streamed frame came meanwhile
            }
            if frame
                .as_ref()
                .is_some_and(|picture| !fits(picture, &result.state))
            {
                frame = None;
            }
            match &frame {
                Some(picture) => {
                    if let Some(stream_id) = &picture.stream_id {
                        s.cursor = Some(FrameCursor {
                            stream_id: stream_id.clone(),
                            sequence: picture.sequence,
                        });
                    }
                }
                None => s.cursor = None,
            }
            let before = s.view.frame.clone();
            let unchanged_picture = match (&frame, &before) {
                (Some(picture), Some(old)) => same_picture(picture, old),
                _ => false,
            };
            if unchanged_picture {
                frame = before;
            }
            s.behind = false;
            s.set(Patch {
                state: Some(result.state.clone()),
                frame: Some(frame),
                error: Some(None),
            });
            if result.state.private_active != Some(true) {
                s.start_stream();
            }
        });
        Ok(())
    }

    /// A streamed frame from the desktop's live stream (the caller passes the frames of the subscription).
    pub async fn stream(&self, rpc: &impl Rpc, data: BrowserStreamFrame) {
        self.with(|s| s.streamed(&self.bot_id, data));
        self.flush(rpc).await;
    }
}

/// The pointer as a page pixel, from a box and a frame: the whole rectangle (the legacy form, not the letterboxed picture).
pub fn page_point(boxed: Rect, frame: Size, client_x: f64, client_y: f64) -> Point {
    let at = |offset: f64, size: f64, of: f64| -> f64 {
        if size > 0.0 {
            (of - 1.0)
                .max(0.0)
                .min(((offset / size) * of + 0.5).floor().max(0.0))
        } else {
            0.0
        }
    };
    Point {
        x: at(client_x - boxed.left, boxed.width, frame.width),
        y: at(client_y - boxed.top, boxed.height, frame.height),
    }
}

/// The legacy viewport: a width, and the height at 4:3, both within the daemon's limits.
pub fn viewport_size(width: f64) -> Size {
    let w = (width + 0.5).floor().clamp(240.0, 2560.0);
    Size::new(w, (w * 3.0 / 4.0 + 0.5).floor().clamp(180.0, 1600.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use client::{ClientMethod, RpcError};
    use protocol::browser::{BrowserDialogParams, KeyAction, MouseAction};
    use serde_json::{Value, json};
    use std::cell::Cell;
    use std::time::Duration;

    /// The TypeScript `state()`: one tab, a human in control, with `patch` merged over it.
    fn state(patch: Value) -> BrowserState {
        let mut base = json!({
            "botId": "b1", "generation": "g1", "control": "human", "activePageId": "p1",
            "tabs": [{ "id": "p1", "url": "https://example.com/", "title": "Example" }], "dialog": null
        });
        if let (Some(base), Value::Object(patch)) = (base.as_object_mut(), patch) {
            base.extend(patch);
        }
        serde_json::from_value(base).expect("a browser state")
    }

    fn frame_json(sequence: u64) -> Value {
        json!({ "data": "AAAA", "mime": "image/jpeg", "width": 800, "height": 600, "sequence": sequence })
    }

    /// A frame of the live stream: `frame(sequence)` on page p1, from stream s1.
    fn live(sequence: u64) -> BrowserFrame {
        let mut value = frame_json(sequence);
        value["streamId"] = json!("s1");
        value["pageId"] = json!("p1");
        serde_json::from_value(value).expect("a frame")
    }

    fn key(action: KeyAction, key: &str) -> BrowserInput {
        BrowserInput::Key {
            action,
            key: key.to_owned(),
        }
    }

    fn mouse(action: MouseAction, x: f64, y: f64, button: Option<MouseButton>) -> BrowserInput {
        BrowserInput::Mouse {
            action,
            x,
            y,
            button,
        }
    }

    fn wheel(delta_y: f64) -> BrowserInput {
        BrowserInput::Wheel {
            x: 10.0,
            y: 20.0,
            delta_x: 0.0,
            delta_y,
        }
    }

    fn text(text: &str) -> BrowserInput {
        BrowserInput::Text {
            text: text.to_owned(),
        }
    }

    /// The `input` of each `browser.input` call, in order.
    fn inputs(daemon: &Daemon) -> Vec<Value> {
        daemon
            .calls("browser.input")
            .into_iter()
            .map(|params| params["input"].clone())
            .collect()
    }

    /// The `action` of each `browser.input` call, in order.
    fn actions(daemon: &Daemon) -> Vec<String> {
        inputs(daemon)
            .iter()
            .map(|input| input["action"].as_str().unwrap_or("").to_owned())
            .collect()
    }

    type Answer = Box<dyn FnMut(&'static str, &Value) -> Result<Value, RpcError> + Send>;

    /// The state the fake daemon answers from, as the TypeScript fake keeps it.
    struct Table {
        current: BrowserState,
        seq: u64,
        fail: Vec<&'static str>,
        streamed: bool,
        unsupported: bool,
    }

    fn table_answer(table: &mut Table, name: &str, params: &Value) -> Result<Value, RpcError> {
        match name {
            "browser.input" | "browser.streamAck" => Ok(json!({})),
            "browser.frame" => {
                table.seq += 1;
                let mut picture = frame_json(table.seq);
                if let (true, Some(epoch)) = (table.streamed, table.current.control_epoch) {
                    picture["controlEpoch"] = json!(epoch);
                }
                Ok(json!({ "state": table.current, "frame": picture }))
            }
            "browser.control" => {
                table.current.control =
                    serde_json::from_value(params["control"].clone()).expect("a control");
                if table.streamed {
                    table.current.control_epoch =
                        Some(table.current.control_epoch.unwrap_or(0) + 1);
                }
                Ok(json!(table.current))
            }
            "browser.dialog" => {
                table.current.dialog = None;
                Ok(json!(table.current))
            }
            "browser.stream" if table.unsupported => Err(RpcError::Daemon {
                code: ErrorCode::UnknownMethod,
                message: "Unknown method browser.stream".into(),
            }),
            _ => Ok(json!(table.current)),
        }
    }

    /// A daemon that answers from a table (or from a script, for the tests that need one) and logs every call. Calls to the
    /// held methods wait for the test's gate, as a promise the test resolves does; every call yields once, as a promise does.
    struct Daemon {
        table: std::sync::Mutex<Table>,
        custom: std::sync::Mutex<Option<Answer>>,
        log: std::sync::Mutex<Vec<(&'static str, Value)>>,
        gates: std::sync::Mutex<Vec<(&'static str, watch::Receiver<bool>)>>,
        flight: std::sync::Mutex<(usize, bool)>,
    }

    impl Daemon {
        fn with_table(table: Table, custom: Option<Answer>) -> Self {
            Self {
                table: std::sync::Mutex::new(table),
                custom: std::sync::Mutex::new(custom),
                log: std::sync::Mutex::default(),
                gates: std::sync::Mutex::default(),
                flight: std::sync::Mutex::default(),
            }
        }

        fn new(initial: BrowserState) -> Self {
            Self::with_table(
                Table {
                    current: initial,
                    seq: 0,
                    fail: Vec::new(),
                    streamed: false,
                    unsupported: false,
                },
                None,
            )
        }

        /// A daemon with the live stream; `unsupported` answers `browser.stream` as an older daemon does.
        fn streamed(initial: BrowserState, unsupported: bool) -> Self {
            Self::with_table(
                Table {
                    current: initial,
                    seq: 0,
                    fail: Vec::new(),
                    streamed: true,
                    unsupported,
                },
                None,
            )
        }

        fn scripted(
            answer: impl FnMut(&'static str, &Value) -> Result<Value, RpcError> + Send + 'static,
        ) -> Self {
            Self::with_table(
                Table {
                    current: state(json!({})),
                    seq: 0,
                    fail: Vec::new(),
                    streamed: false,
                    unsupported: false,
                },
                Some(Box::new(answer)),
            )
        }

        /// Holds calls to `methods` until the returned sender is set to `true`.
        fn hold(&self, methods: &[&'static str]) -> watch::Sender<bool> {
            let (open, gate) = watch::channel(false);
            locked(&self.gates).extend(methods.iter().map(|method| (*method, gate.clone())));
            open
        }

        fn set_state(&self, next: BrowserState) {
            locked(&self.table).current = next;
        }

        fn fail_on(&self, methods: &[&'static str]) {
            locked(&self.table).fail = methods.to_vec();
        }

        fn clear(&self) {
            locked(&self.log).clear();
        }

        /// The params of each call to `method`, in order.
        fn calls(&self, method: &str) -> Vec<Value> {
            locked(&self.log)
                .iter()
                .filter(|(name, _)| *name == method)
                .map(|(_, params)| params.clone())
                .collect()
        }

        fn methods(&self) -> Vec<&'static str> {
            locked(&self.log).iter().map(|(name, _)| *name).collect()
        }

        fn count(&self, method: &str) -> usize {
            self.calls(method).len()
        }

        fn log_len(&self) -> usize {
            locked(&self.log).len()
        }

        fn overlapped(&self) -> bool {
            locked(&self.flight).1
        }

        fn enter(&self, name: &'static str, params: &Value) -> Option<watch::Receiver<bool>> {
            locked(&self.log).push((name, params.clone()));
            {
                let mut flight = locked(&self.flight);
                flight.0 += 1;
                flight.1 |= flight.0 > 1;
            }
            locked(&self.gates)
                .iter()
                .rev()
                .find(|(method, _)| *method == name)
                .map(|(_, gate)| gate.clone())
        }

        /// The call has left the daemon; a method the test failed answers with an error.
        fn leave(&self, name: &'static str) -> Option<RpcError> {
            locked(&self.flight).0 -= 1;
            let failing = locked(&self.table).fail.contains(&name);
            failing.then(|| RpcError::Daemon {
                code: ErrorCode::Internal,
                message: format!("failed: {name}"),
            })
        }

        fn answer(&self, name: &'static str, params: &Value) -> Result<Value, RpcError> {
            let mut custom = locked(&self.custom);
            match custom.as_mut() {
                Some(answer) => answer(name, params),
                None => table_answer(&mut locked(&self.table), name, params),
            }
        }
    }

    impl Rpc for Daemon {
        fn call<M: ClientMethod>(
            &self,
            params: M::Params,
        ) -> impl Future<Output = Result<M::Result, RpcError>> + Send {
            let params = serde_json::to_value(params)
                .map_err(|error| RpcError::BadParams(error.to_string()));
            async move {
                let params = params?;
                let gate = self.enter(M::NAME, &params);
                tokio::task::yield_now().await;
                if let Some(error) = self.leave(M::NAME) {
                    return Err(error);
                }
                if let Some(mut gate) = gate {
                    let _ = gate.wait_for(|open| *open).await;
                }
                let value = self.answer(M::NAME, &params)?;
                serde_json::from_value(value)
                    .map_err(|error| RpcError::BadResult(error.to_string()))
            }
        }
    }

    fn locked<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
        mutex
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `vi.advanceTimersByTimeAsync(ms)` for the session's one timer: runs each poll that falls due, in time order, then
    /// stops at the target instant.
    async fn advance_session(
        session: &BrowserSession,
        daemon: &Daemon,
        clock: &Cell<Instant>,
        ms: u64,
    ) {
        let target = clock.get() + Duration::from_millis(ms);
        while let Some(due) = session.next_timer() {
            if due > target {
                break;
            }
            clock.set(due);
            session.on_timer(daemon, due).await;
        }
        clock.set(target);
    }

    /// A session with its daemon and a clock. `at()` is the clock's instant.
    struct Rig {
        session: BrowserSession,
        daemon: Daemon,
        closes: Arc<Closes>,
        clock: Cell<Instant>,
        live: bool,
    }

    impl Rig {
        fn of(daemon: Daemon, live: bool) -> Self {
            let closes = Arc::new(Closes::default());
            Self {
                session: BrowserSession::new("b1", closes.clone(), live),
                daemon,
                closes,
                clock: Cell::new(Instant::now()),
                live,
            }
        }

        fn new(initial: BrowserState) -> Self {
            Self::of(Daemon::new(initial), false)
        }

        fn streamed(initial: BrowserState) -> Self {
            Self::of(Daemon::streamed(initial, false), true)
        }

        fn at(&self) -> Instant {
            self.clock.get()
        }

        /// A second session for the same bot, as a reopened panel is.
        fn again(&self) -> BrowserSession {
            BrowserSession::new("b1", self.closes.clone(), self.live)
        }

        async fn open(&self) {
            self.session.open(&self.daemon, self.at()).await;
        }

        async fn flush(&self) {
            self.advance(0).await;
        }

        async fn advance(&self, ms: u64) {
            advance_session(&self.session, &self.daemon, &self.clock, ms).await;
        }

        async fn close(&self) {
            self.session.close(&self.daemon, self.at()).await;
        }

        async fn input(&self, input: BrowserInput) {
            self.session.input(&self.daemon, self.at(), input).await;
        }

        /// A frame pushed by the desktop's stream, taken only while a live stream is subscribed.
        async fn push(&self, frame: Option<BrowserFrame>) {
            if self.session.listening() {
                self.session
                    .stream(
                        &self.daemon,
                        BrowserStreamFrame {
                            bot_id: "b1".into(),
                            frame,
                        },
                    )
                    .await;
            }
        }
    }

    #[tokio::test]
    async fn does_not_send_input_for_another_controller_and_suppresses_private_pixels() {
        let other = Rig::new(state(json!({ "canControl": false, "controlEpoch": 4 })));
        other.open().await;
        other.flush().await;
        other.input(text("synthetic")).await;
        other.flush().await;
        assert!(!other.daemon.methods().contains(&"browser.input"));

        let private = Rig::new(state(
            json!({ "canControl": true, "controlEpoch": 5, "privateEpoch": 2, "privateActive": true }),
        ));
        private.open().await;
        private.flush().await;
        assert_eq!(private.session.view().frame, None);
        private
            .session
            .set_visible(&private.daemon, private.at(), false)
            .await;
        private.flush().await;
        let control = private.daemon.calls("browser.control");
        assert_eq!(
            control[0],
            json!({ "botId": "b1", "control": "bot", "generation": "g1", "controlEpoch": 5, "releaseOnly": true })
        );
    }

    #[tokio::test]
    async fn opens_then_pulls_frames_one_at_a_time_every_250_ms_when_idle_without_redrawing_identical_images()
     {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.flush().await;
        assert_eq!(t.daemon.methods(), vec!["browser.open", "browser.frame"]);
        t.advance(650).await;
        assert_eq!(t.daemon.count("browser.frame"), 3);
        assert!(!t.daemon.overlapped());
        assert_eq!(t.session.view().frame.map(|f| f.sequence), Some(1));
    }

    #[tokio::test]
    async fn stops_pulling_while_hidden_and_resumes_when_visible() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.flush().await;
        t.session.set_visible(&t.daemon, t.at(), false).await;
        t.advance(1000).await;
        let frames = t.daemon.count("browser.frame");
        assert!(frames <= 2);
        t.advance(1000).await;
        assert_eq!(t.daemon.count("browser.frame"), frames);
        t.session.set_visible(&t.daemon, t.at(), true).await;
        t.flush().await;
        assert_eq!(t.daemon.count("browser.frame"), frames + 1);
    }

    #[tokio::test]
    async fn keeps_pulling_after_a_failed_pull_slower_and_shows_the_problem() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.flush().await;
        t.daemon.fail_on(&["browser.frame"]);
        t.advance(250).await;
        // The TypeScript expects the bare "browser.frame": its errorText strips the "failed: " prefix. The Rust error text
        // carries the daemon's error code, so this checks that the message is shown.
        let error = t.session.view().error.expect("the failed pull is shown");
        assert!(error.contains("browser.frame"), "{error}");
        t.daemon.fail_on(&[]);
        t.advance(1100).await;
        assert_eq!(t.session.view().error, None);
    }

    #[tokio::test]
    async fn sends_input_in_order_with_the_generation_and_page_from_when_the_user_acted() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.flush().await;
        t.daemon.clear();
        t.input(key(KeyAction::Down, "a")).await;
        t.input(key(KeyAction::Up, "a")).await;
        t.daemon
            .set_state(state(json!({ "generation": "g2", "activePageId": "p2" })));
        t.advance(250).await;
        assert_eq!(actions(&t.daemon), vec!["down", "up"]);
        for params in t.daemon.calls("browser.input") {
            assert_eq!(
                (params["generation"].clone(), params["pageId"].clone()),
                (json!("g1"), json!("p1"))
            );
        }
    }

    #[tokio::test]
    async fn keeps_only_the_newest_of_queued_pointer_moves() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.flush().await;
        t.daemon.clear();
        // The press is in flight, so the moves queue behind it.
        tokio::join!(
            t.input(key(KeyAction::Down, "x")),
            t.input(mouse(MouseAction::Move, 1.0, 0.0, None)),
            t.input(mouse(MouseAction::Move, 2.0, 0.0, None)),
            t.input(mouse(MouseAction::Move, 3.0, 0.0, None)),
        );
        let moves: Vec<Value> = inputs(&t.daemon)
            .into_iter()
            .filter(|input| input["kind"] == "mouse")
            .collect();
        assert_eq!(moves.len(), 1);
        assert_eq!(moves[0]["x"].as_f64(), Some(3.0));
    }

    #[tokio::test]
    async fn sends_no_input_while_the_bot_is_in_control() {
        let t = Rig::new(state(json!({ "control": "bot" })));
        t.open().await;
        t.flush().await;
        t.daemon.clear();
        t.input(text("hi")).await;
        t.flush().await;
        assert!(!t.daemon.methods().contains(&"browser.input"));
    }

    #[tokio::test]
    async fn releases_held_keys_and_buttons_and_closing_releases_them_too_but_leaves_the_browser_open()
     {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.flush().await;
        t.input(key(KeyAction::Down, "Shift")).await;
        t.input(mouse(MouseAction::Down, 5.0, 6.0, Some(MouseButton::Left)))
            .await;
        t.flush().await;
        t.daemon.clear();
        t.session.release_all(&t.daemon, t.at()).await;
        t.flush().await;
        let sent: Vec<(String, String)> = inputs(&t.daemon)
            .iter()
            .map(|input| {
                (
                    input["kind"].as_str().unwrap_or("").to_owned(),
                    input["action"].as_str().unwrap_or("").to_owned(),
                )
            })
            .collect();
        assert_eq!(
            sent,
            vec![("mouse".into(), "up".into()), ("key".into(), "up".into())]
        );

        t.input(key(KeyAction::Down, "b")).await;
        t.flush().await;
        t.daemon.clear();
        t.close().await;
        t.advance(1000).await;
        assert_eq!(
            t.daemon.methods(),
            vec!["browser.open", "browser.input", "browser.control"]
        );
    }

    #[tokio::test]
    async fn releases_a_key_whose_press_was_still_in_flight_when_the_panel_closed() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.flush().await;
        let gate = t.daemon.hold(&["browser.input", "browser.control"]);
        tokio::join!(
            t.input(key(KeyAction::Down, "a")), // in flight, held at the daemon
            async {
                tokio::task::yield_now().await;
                let closing = t.session.close(&t.daemon, t.at()); // the release queues behind the press
                gate.send_replace(true);
                closing.await;
            },
        );
        assert_eq!(actions(&t.daemon), vec!["down", "up"]);
    }

    #[tokio::test]
    async fn still_sends_a_queued_release_at_close_and_drops_a_queued_press() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.flush().await;
        t.input(key(KeyAction::Down, "a")).await;
        t.flush().await;
        t.daemon.clear();
        let gate = t.daemon.hold(&["browser.input", "browser.control"]);
        tokio::join!(
            t.input(key(KeyAction::Down, "b")), // in flight
            t.input(key(KeyAction::Up, "a")),   // queued release
            t.input(key(KeyAction::Down, "c")), // queued press
            async {
                tokio::task::yield_now().await;
                let closing = t.session.close(&t.daemon, t.at());
                gate.send_replace(true);
                closing.await;
            },
        );
        let sent: Vec<String> = inputs(&t.daemon)
            .iter()
            .map(|input| {
                format!(
                    "{}:{}",
                    input["action"].as_str().unwrap_or(""),
                    input["key"].as_str().unwrap_or("")
                )
            })
            .collect();
        assert_eq!(sent, vec!["down:b", "up:a", "up:b"]);
    }

    #[tokio::test]
    async fn hands_the_browser_back_to_the_bot_when_closed_and_not_when_the_bot_already_has_it() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.flush().await;
        t.daemon.clear();
        t.close().await;
        t.advance(500).await;
        assert_eq!(t.daemon.methods().last(), Some(&"browser.control"));
        assert_eq!(
            t.daemon.calls("browser.control").last(),
            Some(
                &json!({ "botId": "b1", "control": "bot", "generation": "g1", "releaseOnly": true })
            )
        );

        let bot = Rig::new(state(json!({ "control": "bot" })));
        bot.open().await;
        bot.flush().await;
        bot.daemon.clear();
        bot.close().await;
        bot.advance(500).await;
        assert!(!bot.daemon.methods().contains(&"browser.control"));
    }

    #[tokio::test]
    async fn does_not_leave_the_user_in_control_when_closed_while_take_control_is_still_in_flight()
    {
        let t = Rig::new(state(json!({ "control": "bot" })));
        t.open().await;
        t.flush().await;
        let gate = t.daemon.hold(&["browser.input", "browser.control"]);
        tokio::join!(
            t.session
                .set_control(&t.daemon, t.at(), BrowserControl::Human), // in flight
            async {
                tokio::task::yield_now().await;
                let closing = t.session.close(&t.daemon, t.at()); // the reply to Take control arrives after this, unshown
                gate.send_replace(true);
                closing.await;
            },
        );
        t.advance(500).await;
        let controls: Vec<Value> = t
            .daemon
            .calls("browser.control")
            .iter()
            .map(|params| params["control"].clone())
            .collect();
        assert_eq!(controls, vec![json!("human"), json!("bot")]);
        assert_eq!(
            t.session.view().state.map(|s| s.control),
            Some(BrowserControl::Bot)
        );
    }

    #[tokio::test]
    async fn hands_control_back_even_when_the_browser_cant_be_asked_if_take_control_was_in_flight()
    {
        let t = Rig::new(state(json!({ "control": "bot" })));
        t.open().await;
        t.flush().await;
        let gate = t.daemon.hold(&["browser.input", "browser.control"]);
        tokio::join!(
            t.session
                .set_control(&t.daemon, t.at(), BrowserControl::Human),
            async {
                tokio::task::yield_now().await;
                let closing = t.session.close(&t.daemon, t.at());
                t.daemon.fail_on(&["browser.frame", "browser.open"]);
                gate.send_replace(true);
                closing.await;
            },
        );
        t.advance(500).await;
        let controls: Vec<Value> = t
            .daemon
            .calls("browser.control")
            .iter()
            .map(|params| params["control"].clone())
            .collect();
        assert_eq!(controls, vec![json!("human"), json!("bot")]);
    }

    #[tokio::test]
    async fn reads_fresh_ownership_without_a_picture_and_hands_back_what_it_says_is_human() {
        let t = Rig::new(state(json!({ "control": "human" })));
        t.open().await;
        t.flush().await;
        t.daemon.clear();
        t.daemon.fail_on(&["browser.frame"]);
        t.close().await;
        t.advance(500).await;
        assert_eq!(t.daemon.methods(), vec!["browser.open", "browser.control"]);
    }

    #[tokio::test]
    async fn makes_a_quick_reopen_wait_for_the_old_panels_hand_back() {
        let t = Rig::new(state(json!({ "control": "human" })));
        t.open().await;
        t.flush().await;
        t.daemon.clear();
        let gate = t.daemon.hold(&["browser.input", "browser.control"]);
        let next = t.again();
        tokio::join!(
            t.input(key(KeyAction::Down, "a")), // keeps the old teardown waiting
            t.session.close(&t.daemon, t.at()), // not awaited by the TypeScript: it runs alongside the reopen
            next.open(&t.daemon, t.at()),
            async {
                for _ in 0..4 {
                    tokio::task::yield_now().await;
                }
                let before_release: Vec<&str> = t
                    .daemon
                    .methods()
                    .into_iter()
                    .filter(|m| *m != "browser.frame")
                    .collect();
                assert_eq!(before_release, vec!["browser.input"]);
                gate.send_replace(true);
            },
        );
        let order = t.daemon.methods();
        let control = order
            .iter()
            .position(|m| *m == "browser.control")
            .expect("the old panel hands back");
        assert!(
            order
                .iter()
                .rposition(|m| *m == "browser.open")
                .expect("the reopen reads")
                > control
        );
        assert_eq!(
            t.daemon
                .calls("browser.control")
                .first()
                .map(|p| p["control"].clone()),
            Some(json!("bot"))
        );
    }

    #[tokio::test]
    async fn clears_the_picture_when_the_page_is_resized_or_moved_and_takes_no_input_until_a_fresh_frame()
     {
        for action in 0..3 {
            let t = Rig::new(state(json!({})));
            t.open().await;
            t.flush().await;
            let old = t.session.view().frame.map_or(0, |f| f.sequence);
            match action {
                0 => t.session.resize(&t.daemon, t.at(), 500, 375).await,
                1 => {
                    t.session
                        .navigate(&t.daemon, t.at(), "https://b.test/")
                        .await
                }
                _ => {
                    t.session
                        .history(&t.daemon, t.at(), HistoryAction::Back)
                        .await
                }
            }
            assert_eq!(t.session.view().frame, None);
            t.daemon.clear();
            t.input(text("x")).await;
            t.flush().await;
            assert!(!t.daemon.methods().contains(&"browser.input"));
            t.advance(500).await;
            assert!(t.session.view().frame.map_or(0, |f| f.sequence) > old);
            t.input(text("x")).await;
            assert!(t.daemon.methods().contains(&"browser.input"));
        }
    }

    #[tokio::test]
    async fn keeps_the_picture_through_a_key_press_a_control_change_or_a_dialog_answer() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.flush().await;
        t.input(key(KeyAction::Down, "a")).await;
        t.session.dialog(&t.daemon, t.at(), false, None).await;
        t.session
            .set_control(&t.daemon, t.at(), BrowserControl::Human)
            .await;
        assert!(t.session.view().frame.is_some());
    }

    #[tokio::test]
    async fn dismisses_a_page_dialog_left_open_before_handing_the_browser_back() {
        let dialog =
            json!({ "pageId": "p1", "type": "confirm", "message": "Sure?", "defaultValue": "" });
        let t = Rig::new(state(json!({ "dialog": dialog })));
        t.open().await;
        t.flush().await;
        t.daemon.clear();
        t.close().await;
        t.advance(500).await;
        assert_eq!(
            t.daemon.methods(),
            vec!["browser.open", "browser.dialog", "browser.control"]
        );
        let dismissed: BrowserDialogParams =
            serde_json::from_value(t.daemon.calls("browser.dialog")[0].clone())
                .expect("dialog params");
        assert!(!dismissed.accept);
        assert_eq!(
            (
                dismissed.page.generation.as_str(),
                dismissed.page.page_id.as_str()
            ),
            ("g1", "p1")
        );
    }

    #[tokio::test]
    async fn ignores_a_key_up_for_a_key_it_never_saw_go_down() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.flush().await;
        t.daemon.clear();
        t.input(key(KeyAction::Up, "v")).await;
        t.flush().await;
        assert!(!t.daemon.methods().contains(&"browser.input"));
    }

    #[tokio::test]
    async fn does_nothing_after_close_even_for_a_pull_that_was_in_flight() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.flush().await;
        t.close().await;
        let seen = t.session.changes();
        t.advance(1000).await;
        assert_eq!(t.session.changes(), seen);
    }

    #[tokio::test]
    async fn drops_the_old_picture_when_the_page_or_generation_changes_and_takes_no_input_until_a_new_one_shows()
     {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.flush().await;
        assert!(t.session.view().frame.is_some());
        t.session.tab(&t.daemon, t.at(), TabAction::New, None).await;
        assert_eq!(t.session.view().frame, None);
        t.daemon.clear();
        t.input(text("x")).await;
        t.daemon.set_state(state(json!({ "activePageId": "p2" })));
        t.flush().await;
        assert!(!t.daemon.methods().contains(&"browser.input"));
        assert_eq!(
            t.session.view().state.and_then(|s| s.active_page_id),
            Some("p2".to_owned())
        );
    }

    #[tokio::test]
    async fn switches_control_and_applies_the_returned_state() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.flush().await;
        t.session
            .set_control(&t.daemon, t.at(), BrowserControl::Bot)
            .await;
        t.flush().await;
        assert_eq!(
            t.session.view().state.map(|s| s.control),
            Some(BrowserControl::Bot)
        );
        assert_eq!(
            t.daemon.calls("browser.control").first(),
            Some(&json!({ "botId": "b1", "control": "bot", "generation": "g1" }))
        );
    }

    #[tokio::test]
    async fn sends_navigation_history_tab_resize_and_dialog_calls_for_the_active_page() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.flush().await;
        t.daemon.clear();
        t.session
            .navigate(&t.daemon, t.at(), "https://a.test/")
            .await;
        t.session
            .history(&t.daemon, t.at(), HistoryAction::Back)
            .await;
        t.session
            .tab(&t.daemon, t.at(), TabAction::Select, Some("p1"))
            .await;
        t.session.resize(&t.daemon, t.at(), 800, 600).await;
        t.session.dialog(&t.daemon, t.at(), true, Some("x")).await;
        t.flush().await;
        let calls: Vec<&str> = t
            .daemon
            .methods()
            .into_iter()
            .filter(|m| *m != "browser.frame")
            .collect();
        assert_eq!(
            calls,
            vec![
                "browser.navigate",
                "browser.history",
                "browser.tab",
                "browser.resize",
                "browser.dialog"
            ]
        );
        let navigate = &t.daemon.calls("browser.navigate")[0];
        assert_eq!(
            (
                navigate["generation"].clone(),
                navigate["pageId"].clone(),
                navigate["url"].clone()
            ),
            (json!("g1"), json!("p1"), json!("https://a.test/"))
        );
    }

    #[tokio::test]
    async fn keeps_one_frame_request_in_flight_while_ordered_input_and_a_modal_answer_proceed_independently()
     {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.flush().await;
        let gate = t.daemon.hold(&["browser.frame"]);
        tokio::join!(
            advance_session(&t.session, &t.daemon, &t.clock, 250), // the second frame pull is held
            async {
                for _ in 0..4 {
                    tokio::task::yield_now().await;
                }
                t.input(key(KeyAction::Down, "x")).await;
                t.input(key(KeyAction::Up, "x")).await;
                t.session.dialog(&t.daemon, t.at(), false, None).await;
                let order: Vec<&str> = t
                    .daemon
                    .methods()
                    .into_iter()
                    .filter(|m| *m == "browser.input" || *m == "browser.dialog")
                    .collect();
                assert_eq!(
                    order,
                    vec!["browser.input", "browser.input", "browser.dialog"]
                );
                assert_eq!(t.daemon.count("browser.frame"), 2);
                gate.send_replace(true);
            },
        );
        t.close().await;
    }

    #[derive(Clone, Copy)]
    enum Fence {
        Control,
        Tab,
        Navigate,
        Resize,
    }

    /// Fences an old in-flight frame across a command: the reply from before the command is dropped, and a lower server
    /// revision is never shown.
    async fn fences_an_old_in_flight_frame_across(action: Fence) {
        let initial = state(json!({ "revision": 1, "viewportRevision": 0 }));
        let current = Arc::new(std::sync::Mutex::new(initial.clone()));
        let reads = Arc::new(std::sync::Mutex::new(0u64));
        let (shared, counter, first) = (current.clone(), reads.clone(), initial.clone());
        let daemon = Daemon::scripted(move |name, _params| match name {
            "browser.frame" => {
                let read = {
                    let mut reads = locked(&counter);
                    *reads += 1;
                    *reads
                };
                if read == 2 {
                    return Ok(
                        json!({ "state": first, "frame": { "data": "obsolete", "mime": "image/jpeg", "width": 800, "height": 600, "sequence": 99 } }),
                    );
                }
                let now = locked(&shared).clone();
                Ok(json!({
                    "state": now,
                    "frame": {
                        "data": format!("fresh{read}"), "mime": "image/jpeg", "width": 800, "height": 600, "sequence": read,
                        "pageId": now.active_page_id, "viewportRevision": now.viewport_revision
                    }
                }))
            }
            "browser.open" => Ok(json!(*locked(&shared))),
            _ => {
                let next = state(json!({
                    "revision": 2,
                    "viewportRevision": if matches!(action, Fence::Navigate | Fence::Resize) { 1 } else { 0 },
                    "activePageId": if matches!(action, Fence::Tab) { "p2" } else { "p1" },
                    "control": if matches!(action, Fence::Control) { "bot" } else { "human" },
                }));
                *locked(&shared) = next.clone();
                Ok(json!(next))
            }
        });
        let t = Rig::of(daemon, false);
        t.open().await;
        t.flush().await;
        let gate = t.daemon.hold(&["browser.frame"]);
        tokio::join!(
            advance_session(&t.session, &t.daemon, &t.clock, 250), // the second frame pull is in flight
            async {
                for _ in 0..4 {
                    tokio::task::yield_now().await;
                }
                match action {
                    Fence::Control => {
                        t.session
                            .set_control(&t.daemon, t.at(), BrowserControl::Bot)
                            .await
                    }
                    Fence::Tab => t.session.tab(&t.daemon, t.at(), TabAction::New, None).await,
                    Fence::Navigate => {
                        t.session
                            .navigate(&t.daemon, t.at(), "https://example.com/new")
                            .await
                    }
                    Fence::Resize => t.session.resize(&t.daemon, t.at(), 700, 500).await,
                }
                t.flush().await;
                gate.send_replace(true); // the obsolete reply arrives now
            },
        );
        let view = t.session.view();
        assert_eq!(view.state.as_ref().and_then(|s| s.revision), Some(2));
        assert_ne!(
            view.frame.as_ref().map(|f| f.data.as_str()),
            Some("obsolete")
        );
        t.advance(40).await;
        let accepted = t.session.view();
        *locked(&current) = initial;
        t.advance(40).await;
        assert_eq!(t.session.view(), accepted);
        t.close().await;
    }

    #[tokio::test]
    async fn fences_an_old_in_flight_frame_across_control_and_rejects_lower_server_revisions() {
        fences_an_old_in_flight_frame_across(Fence::Control).await;
    }

    #[tokio::test]
    async fn fences_an_old_in_flight_frame_across_tab_and_rejects_lower_server_revisions() {
        fences_an_old_in_flight_frame_across(Fence::Tab).await;
    }

    #[tokio::test]
    async fn fences_an_old_in_flight_frame_across_navigate_and_rejects_lower_server_revisions() {
        fences_an_old_in_flight_frame_across(Fence::Navigate).await;
    }

    #[tokio::test]
    async fn fences_an_old_in_flight_frame_across_resize_and_rejects_lower_server_revisions() {
        fences_an_old_in_flight_frame_across(Fence::Resize).await;
    }

    #[tokio::test]
    async fn coalesces_adjacent_wheels_and_resizes_but_preserves_clicks_keys_and_release_barriers()
    {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.flush().await;
        t.daemon.clear();
        let gate = t.daemon.hold(&["browser.input", "browser.control"]);
        tokio::join!(
            t.input(key(KeyAction::Down, "x")), // in flight
            async {
                tokio::task::yield_now().await;
                t.input(wheel(5.0)).await;
                t.input(wheel(7.0)).await;
                t.input(mouse(
                    MouseAction::Down,
                    10.0,
                    20.0,
                    Some(MouseButton::Left),
                ))
                .await;
                t.input(wheel(9.0)).await;
                t.input(mouse(MouseAction::Up, 10.0, 20.0, Some(MouseButton::Left)))
                    .await;
                t.session.resize(&t.daemon, t.at(), 500, 400).await;
                t.session.resize(&t.daemon, t.at(), 600, 400).await;
                t.session.resize(&t.daemon, t.at(), 700, 400).await;
                t.session
                    .history(&t.daemon, t.at(), HistoryAction::Back)
                    .await;
                t.session.resize(&t.daemon, t.at(), 800, 400).await;
                gate.send_replace(true);
            },
        );
        t.flush().await;
        let sent: Vec<(String, Option<f64>)> = inputs(&t.daemon)
            .iter()
            .map(|input| {
                (
                    format!(
                        "{}:{}",
                        input["kind"].as_str().unwrap_or(""),
                        input["action"].as_str().unwrap_or("")
                    ),
                    input["deltaY"].as_f64(),
                )
            })
            .collect();
        assert_eq!(
            sent,
            vec![
                ("key:down".to_owned(), None),
                ("wheel:".to_owned(), Some(12.0)),
                ("mouse:down".to_owned(), None),
                ("wheel:".to_owned(), Some(9.0)),
                ("mouse:up".to_owned(), None),
            ]
        );
        let widths: Vec<Value> = t
            .daemon
            .calls("browser.resize")
            .iter()
            .map(|p| p["width"].clone())
            .collect();
        assert_eq!(widths, vec![json!(700), json!(800)]);
        t.close().await;
    }

    #[tokio::test]
    async fn suspends_through_pending_takeover_and_resumes_behind_handback_without_losing_session_state()
     {
        let t = Rig::new(state(json!({ "control": "bot" })));
        t.open().await;
        t.flush().await;
        let gate = t.daemon.hold(&["browser.input", "browser.control"]);
        tokio::join!(
            t.session
                .set_control(&t.daemon, t.at(), BrowserControl::Human), // in flight
            async {
                tokio::task::yield_now().await;
                tokio::join!(t.session.suspend(&t.daemon, t.at()), async {
                    t.session.set_visible(&t.daemon, t.at(), true).await;
                    t.session
                        .set_control(&t.daemon, t.at(), BrowserControl::Human)
                        .await; // queues behind the hand-back
                    assert!(t.session.view().busy);
                    gate.send_replace(true);
                },);
            },
        );
        t.flush().await;
        let controls: Vec<Value> = t
            .daemon
            .calls("browser.control")
            .iter()
            .map(|p| p["control"].clone())
            .collect();
        assert_eq!(controls, vec![json!("human"), json!("bot"), json!("human")]);
        let view = t.session.view();
        assert_eq!(view.state.map(|s| s.control), Some(BrowserControl::Human));
        assert!(!view.busy);
        assert_eq!(t.daemon.count("browser.open"), 2); // the initial open, and the hand-back's state read
        t.close().await;
    }

    #[tokio::test]
    async fn a_second_hide_drops_a_takeover_queued_during_a_still_pending_handback() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.flush().await;
        t.daemon.clear();
        let gate = t.daemon.hold(&["browser.input", "browser.control"]);
        tokio::join!(
            t.input(key(KeyAction::Down, "Shift")), // in flight
            async {
                tokio::task::yield_now().await;
                tokio::join!(
                    t.session.suspend(&t.daemon, t.at()), // the first hide
                    async {
                        t.session.set_visible(&t.daemon, t.at(), true).await;
                        t.session
                            .set_control(&t.daemon, t.at(), BrowserControl::Human)
                            .await; // queued behind the hand-back
                        let second = t.session.suspend(&t.daemon, t.at()); // the second hide drops that takeover
                        gate.send_replace(true);
                        second.await;
                    },
                );
            },
        );
        let controls: Vec<Value> = t
            .daemon
            .calls("browser.control")
            .iter()
            .map(|p| p["control"].clone())
            .collect();
        assert_eq!(controls, vec![json!("bot")]);
        assert_eq!(
            t.session.view().state.map(|s| s.control),
            Some(BrowserControl::Bot)
        );
        let reads = t.daemon.log_len();
        t.advance(2000).await;
        assert_eq!(t.daemon.log_len(), reads);
        t.close().await;
    }

    #[tokio::test]
    async fn retains_the_last_image_and_avoids_view_notifications_for_an_unchanged_cursor_response()
    {
        let first = json!({
            "data": "AAAA", "mime": "image/jpeg", "width": 800, "height": 600, "sequence": 1,
            "streamId": "s1", "pageId": "p1", "viewportRevision": 1
        });
        let current = state(json!({ "revision": 1, "viewportRevision": 1 }));
        let mut polls = 0u64;
        let daemon = Daemon::scripted(move |name, _params| {
            if name != "browser.frame" {
                return Ok(json!(current));
            }
            polls += 1;
            let picture = if polls == 1 {
                first.clone()
            } else {
                Value::Null
            };
            Ok(json!({ "state": current, "frame": picture, "unchanged": polls > 1 }))
        });
        let t = Rig::of(daemon, false);
        t.open().await;
        t.flush().await;
        let picture = t.session.view().frame;
        let seen = t.session.changes();
        t.advance(750).await;
        assert_eq!(t.session.view().frame, picture);
        assert_eq!(t.session.changes(), seen);
        assert_eq!(
            t.daemon
                .calls("browser.frame")
                .last()
                .map(|p| p["after"].clone()),
            Some(json!({ "streamId": "s1", "sequence": 1 }))
        );
        t.close().await;
    }

    #[tokio::test]
    async fn hands_back_promptly_through_an_unresolved_image_request_and_discards_its_eventual_response()
     {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.flush().await;
        t.input(key(KeyAction::Down, "Shift")).await;
        let gate = t.daemon.hold(&["browser.frame"]);
        tokio::join!(
            advance_session(&t.session, &t.daemon, &t.clock, 0), // the second frame is held
            async {
                for _ in 0..4 {
                    tokio::task::yield_now().await;
                }
                t.session.suspend(&t.daemon, t.at()).await; // before the held frame is released
                assert_eq!(
                    t.session.view().state.map(|s| s.control),
                    Some(BrowserControl::Bot)
                );
                assert_eq!(actions(&t.daemon), vec!["down", "up"]);
                assert_eq!(t.daemon.count("browser.frame"), 2);
                assert_eq!(t.session.view().frame, None);
                gate.send_replace(true); // the held reply comes back, and is dropped
            },
        );
        assert_eq!(
            t.session.view().state.map(|s| s.control),
            Some(BrowserControl::Bot)
        );
        assert_eq!(t.session.view().frame, None);
        t.close().await;
    }

    #[tokio::test]
    async fn sends_drag_and_key_releases_to_their_original_tab_after_a_tab_selection() {
        let t = Rig::new(state(json!({})));
        t.open().await;
        t.flush().await;
        t.input(key(KeyAction::Down, "Shift")).await;
        t.input(mouse(
            MouseAction::Down,
            10.0,
            20.0,
            Some(MouseButton::Left),
        ))
        .await;
        t.flush().await;
        t.daemon.set_state(state(json!({ "activePageId": "p2" })));
        t.session
            .tab(&t.daemon, t.at(), TabAction::Select, Some("p2"))
            .await;
        t.flush().await;
        t.daemon.clear();
        t.session.release_all(&t.daemon, t.at()).await;
        t.flush().await;
        let releases: Vec<Value> = t
            .daemon
            .calls("browser.input")
            .into_iter()
            .filter(|p| p["input"]["action"] == "up")
            .collect();
        assert_eq!(releases.len(), 2);
        assert!(releases.iter().all(|p| p["pageId"] == "p1"));
        t.close().await;
    }

    #[tokio::test]
    async fn asks_for_the_stream_while_shown_and_stops_it_when_hidden_or_closed() {
        let t = Rig::streamed(state(json!({})));
        t.open().await;
        t.flush().await;
        assert_eq!(
            t.daemon.methods(),
            vec!["browser.open", "browser.frame", "browser.stream"]
        );
        assert_eq!(
            t.daemon.calls("browser.stream")[0],
            json!({ "botId": "b1", "on": true })
        );
        assert!(t.session.listening());
        t.session.set_visible(&t.daemon, t.at(), false).await;
        t.flush().await;
        let streams = |d: &Daemon| {
            d.calls("browser.stream")
                .iter()
                .map(|p| p["on"].clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(streams(&t.daemon), vec![json!(true), json!(false)]);
        assert!(!t.session.listening());
        t.session.set_visible(&t.daemon, t.at(), true).await;
        t.flush().await;
        assert_eq!(
            streams(&t.daemon),
            vec![json!(true), json!(false), json!(true)]
        );
        assert!(t.session.listening());
        t.close().await;
        assert_eq!(
            streams(&t.daemon),
            vec![json!(true), json!(false), json!(true), json!(false)]
        );
        assert!(!t.session.listening());
    }

    #[tokio::test]
    async fn acknowledges_each_streamed_frame_once_in_order_as_it_is_drawn_replaced_or_dropped() {
        let t = Rig::streamed(state(json!({})));
        t.open().await;
        t.flush().await;
        t.push(Some(live(1))).await;
        t.push(Some(live(2))).await; // the first is replaced before it is drawn
        assert_eq!(t.session.view().frame, Some(live(2)));
        let acks = |d: &Daemon| {
            d.calls("browser.streamAck")
                .iter()
                .map(|p| p["sequence"].clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(acks(&t.daemon), vec![json!(1)]);
        let shown = t.session.view().frame.expect("the streamed frame is shown");
        t.session.drawn(&t.daemon, &shown).await;
        t.session.drawn(&t.daemon, &shown).await; // Strict Mode runs the view's effect twice
        assert_eq!(acks(&t.daemon), vec![json!(1), json!(2)]);
        let mut other_tab = live(3);
        other_tab.page_id = Some("p9".into()); // another tab's, so never drawn
        t.push(Some(other_tab)).await;
        assert_eq!(t.session.view().frame, Some(shown));
        assert_eq!(acks(&t.daemon), vec![json!(1), json!(2), json!(3)]);
        assert_eq!(
            t.daemon.calls("browser.streamAck")[0],
            json!({ "botId": "b1", "sequence": 1 })
        );
        t.close().await;
    }

    #[tokio::test]
    async fn polls_browser_frame_once_the_stream_ends_and_asks_again_when_the_private_interaction_is_over()
     {
        let t = Rig::streamed(state(json!({})));
        t.open().await;
        t.flush().await;
        t.advance(600).await;
        assert_eq!(t.daemon.count("browser.frame"), 1); // while streaming, the poll reads only the state
        assert!(t.daemon.count("browser.open") > 1);
        t.daemon
            .set_state(state(json!({ "privateActive": true, "privateEpoch": 1 })));
        t.push(None).await;
        assert!(!t.session.listening());
        t.advance(600).await;
        assert!(t.daemon.count("browser.frame") > 1);
        assert_eq!(t.session.view().frame, None);
        let streams = |d: &Daemon| {
            d.calls("browser.stream")
                .iter()
                .map(|p| p["on"].clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(streams(&t.daemon), vec![json!(true)]); // not while it is private
        t.daemon.set_state(state(json!({ "privateEpoch": 1 })));
        t.advance(300).await;
        assert_eq!(streams(&t.daemon), vec![json!(true), json!(true)]);
        assert!(t.session.listening());
        t.close().await;
    }

    #[tokio::test]
    async fn keeps_polling_browser_frame_and_never_asks_again_when_an_older_daemon_answers_unknown_method()
     {
        let t = Rig::of(Daemon::streamed(state(json!({})), true), true);
        t.open().await;
        t.flush().await;
        let streams = |d: &Daemon| {
            d.calls("browser.stream")
                .iter()
                .map(|p| p["on"].clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(streams(&t.daemon), vec![json!(true)]);
        assert!(!t.session.listening());
        t.advance(600).await;
        assert!(t.daemon.count("browser.frame") > 1);
        t.session.set_visible(&t.daemon, t.at(), false).await;
        t.flush().await;
        t.session.set_visible(&t.daemon, t.at(), true).await;
        t.flush().await;
        assert_eq!(streams(&t.daemon), vec![json!(true)]);
        t.close().await;
    }

    #[tokio::test]
    async fn pulls_one_fresh_picture_when_a_command_leaves_the_streamed_one_out_of_date() {
        let t = Rig::of(
            Daemon::streamed(state(json!({ "controlEpoch": 1 })), false),
            true,
        );
        t.open().await;
        t.flush().await;
        t.session
            .set_control(&t.daemon, t.at(), BrowserControl::Bot)
            .await; // the picture shown was taken under the old controller
        t.flush().await;
        assert_eq!(t.daemon.count("browser.frame"), 2);
        assert_eq!(
            t.session.view().frame.and_then(|f| f.control_epoch),
            Some(2)
        );
        t.advance(600).await;
        assert_eq!(t.daemon.count("browser.frame"), 2); // then the stream keeps it current again
        t.close().await;
    }

    #[test]
    fn maps_a_pointer_onto_the_pages_own_pixels_inside_its_edges() {
        use super::super::browser_geometry::{Rect, Size};
        let boxed = Rect {
            left: 100.0,
            top: 50.0,
            width: 400.0,
            height: 300.0,
        };
        let page = Size::new(800.0, 600.0);
        assert_eq!(
            page_point(boxed, page, 300.0, 200.0),
            super::super::browser_geometry::Point { x: 400.0, y: 300.0 }
        );
        assert_eq!(
            page_point(boxed, page, 0.0, 9999.0),
            super::super::browser_geometry::Point { x: 0.0, y: 599.0 }
        );
        assert_eq!(
            page_point(
                Rect {
                    width: 0.0,
                    ..boxed
                },
                page,
                5.0,
                5.0
            )
            .x,
            0.0
        );
    }

    #[test]
    fn names_mouse_buttons_and_ignores_the_side_buttons() {
        assert_eq!(
            [0u8, 1, 2, 3].map(mouse_button),
            [
                Some(MouseButton::Left),
                Some(MouseButton::Middle),
                Some(MouseButton::Right),
                None
            ]
        );
    }

    #[test]
    fn turns_wheel_lines_and_pages_into_pixels_within_the_daemons_limit() {
        assert_eq!(wheel_pixels(3.0, 1), 48.0);
        assert_eq!(wheel_pixels(1.0, 2), 400.0);
        assert_eq!(wheel_pixels(-99999.0, 0), -10_000.0);
    }

    #[test]
    fn reads_the_address_bar_as_an_address_a_host_this_computer_or_a_search() {
        assert_eq!(address_to_url(" https://a.test/x "), "https://a.test/x");
        assert_eq!(
            address_to_url("example.com/path"),
            "https://example.com/path"
        );
        assert_eq!(
            address_to_url("localhost:3000/a"),
            "http://localhost:3000/a"
        );
        assert_eq!(
            address_to_url("cheap flights"),
            "https://duckduckgo.com/?q=cheap%20flights"
        );
    }

    #[test]
    fn lets_shifttab_leave_the_page_waits_on_paste_and_sends_other_keys() {
        use super::super::browser_geometry::{KeyEvent, KeyRoute};
        let plain = |key| KeyEvent {
            key,
            shift_key: false,
            ctrl_key: false,
            meta_key: false,
        };
        assert_eq!(
            key_route(&KeyEvent {
                shift_key: true,
                ..plain("Tab")
            }),
            KeyRoute::Leave
        );
        assert_eq!(key_route(&plain("Tab")), KeyRoute::Send);
        assert_eq!(
            key_route(&KeyEvent {
                ctrl_key: true,
                ..plain("V")
            }),
            KeyRoute::Paste
        );
        assert_eq!(key_route(&plain("a")), KeyRoute::Send);
    }

    #[test]
    fn keeps_the_requested_viewport_4_3_and_inside_the_daemons_limits() {
        use super::super::browser_geometry::Size;
        assert_eq!(viewport_size(400.0), Size::new(400.0, 300.0));
        assert_eq!(viewport_size(100.0), Size::new(240.0, 180.0));
        assert_eq!(viewport_size(9000.0), Size::new(2560.0, 1600.0));
    }
}
