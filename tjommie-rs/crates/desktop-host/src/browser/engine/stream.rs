//! A bot's live view (Phase 3 plan T3.4; `native-browser.ts`: `frame`, `#syncStream`, `#applyStream`, `#stopStream`,
//! `#forwardFrame`, `beginPrivate`, `endPrivate`). A screencast runs on the bot's active page for as long as any remote
//! controller watches, at the highest rate any of them asked. Each frame is acknowledged to Chromium at once, throttled,
//! validated and delivered to the daemon (newest frame wins while a delivery runs). A private interaction, a closing
//! browser or a daemon with nobody watching ends it.
//!
//! The screencast is not what makes a hidden page paint here (as it is in Electron): while anyone watches, the bot's
//! `streaming` need keeps its page awake and parked on-screen, so a capture needs no screencast of its own and the two
//! never take turns.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use futures::FutureExt as _;
use futures::future::{Either, select};
use protocol::browser::{BrowserFrame, BrowserFrameResult, BrowserState, FrameMime};
use protocol::native_browser::{NativeBrowserStreamMessage, StreamEnd};

use super::*;
use crate::browser::cdp::NoParams;
use crate::browser::stream::{
    AckParams, Frame, Newest, PageScreencastFrameAck, PageStartScreencast, PageStopScreencast,
    SCREENCAST, ScreencastFrame, Throttle, jpeg_size, validate,
};
use crate::browser::tools::protocol::{
    CaptureScreenshotParams, Clip, NetworkDisable, PageCaptureScreenshot, PageGetLayoutMetrics,
    RuntimeDisable, RuntimeTerminateExecution,
};

/// How long the screencast may take to start, and to stop.
const START_LIMIT: Duration = Duration::from_secs(5);
const STOP_LIMIT: Duration = Duration::from_millis(500);
/// How long one DevTools command of a capture, or of entering private mode, may take.
const CAPTURE_LIMIT: Duration = Duration::from_secs(10);
const PRIVATE_LIMIT: Duration = Duration::from_secs(1);
/// A thumbnail fits this box and this many bytes.
const THUMBNAIL: (f64, f64) = (320.0, 240.0);
const THUMBNAIL_BYTES: usize = 64 * 1024;
/// The size a frame, or a viewport, may have at most.
const MAX_SIDE: u32 = 4096;

/// The screencast of one page, while it runs.
pub(super) struct Source {
    pub page_id: String,
    pub tab_id: String,
    pub throttle: Throttle,
    pub newest: Newest,
    /// When the stream began: frame times are milliseconds since.
    pub started: Instant,
}

/// What a bot's stream is, and who wants it.
#[derive(Default)]
pub(super) struct StreamState {
    /// Each watching controller and the frame rate it asked for.
    pub controllers: HashMap<String, u32>,
    pub source: Option<Source>,
    /// Bumped each time the stream starts or stops: a frame, a delivery or a start belonging to an older run is ignored.
    pub run: u64,
}

impl Engine {
    /// Runs `future` for at most `limit`; `None` if it did not finish.
    async fn within<T>(&self, limit: Duration, future: impl Future<Output = T>) -> Option<T> {
        futures::pin_mut!(future);
        match select(future, self.sleep(limit)).await {
            Either::Left((value, _)) => Some(value),
            Either::Right(_) => None,
        }
    }

    /// Looks at the stream again where no `await` could carry the change (a state change): on the host's executor.
    pub(super) fn kick_stream(&self, bot: &Rc<Bot>) {
        {
            let state = bot.state.borrow();
            if state.stream.source.is_none() && state.stream.controllers.is_empty() {
                return;
            }
        }
        let (Some(spawn), Some(engine)) = (self.config.spawn.clone(), self.weak.upgrade()) else {
            return;
        };
        let bot = bot.clone();
        spawn(Box::pin(async move {
            let _ = engine.sync_stream(&bot).await;
        }));
    }

    /// Brings the screencast in line with what is wanted now. One at a time per bot; a failure to start reaches the caller.
    pub(super) async fn sync_stream(self: &Rc<Self>, bot: &Rc<Bot>) -> Result<(), EngineError> {
        let _turn = bot.stream_lock.lock().await;
        let (wanted, dead, private, page_id, max_fps) = {
            let state = bot.state.borrow();
            (
                !state.stream.controllers.is_empty(),
                state.dead,
                state.access.private_active(),
                state.active.clone(),
                state
                    .stream
                    .controllers
                    .values()
                    .copied()
                    .max()
                    .unwrap_or(60),
            )
        };
        let page_id = match page_id {
            Some(page_id) if wanted && !dead && !private => page_id,
            _ => {
                let reason = if dead {
                    StreamEnd::Closed
                } else if private {
                    StreamEnd::Private
                } else {
                    StreamEnd::Failed
                };
                self.stop_stream(bot, reason).await;
                return Ok(());
            }
        };
        {
            let mut state = bot.state.borrow_mut();
            if let Some(source) = state.stream.source.as_mut()
                && source.page_id == page_id
            {
                // Already running on this page: only the rate may change.
                source.throttle.set_max_fps(max_fps);
                return Ok(());
            }
        }
        self.stop_source(bot).await;
        let tab = self.tab(bot, Some(&page_id))?;
        // This page's frames reach the engine from now on (registered once per page).
        if !tab.screencast_wired.replace(true)
            && let Err(error) =
                events::watch_screencast(&self.events, &bot.identity.id, &tab.id, &tab.webview)
        {
            tab.screencast_wired.set(false);
            return Err(error.into());
        }
        let run = {
            let mut state = bot.state.borrow_mut();
            state.stream.run += 1;
            state.stream.run
        };
        // The page is awake, visible and painting before the screencast starts, whatever the window is doing.
        let _ = self.set_streaming(&bot.identity.id, true);
        let started = self
            .within(
                START_LIMIT,
                cdp::call::<PageStartScreencast>(&tab.webview, &SCREENCAST),
            )
            .await;
        match started {
            Some(Ok(_)) => {}
            Some(Err(error)) => {
                let _ = self.set_streaming(&bot.identity.id, false);
                return Err(error.into());
            }
            None => {
                self.stop_screencast(&tab).await;
                let _ = self.set_streaming(&bot.identity.id, false);
                return Err(EngineError::refused("The browser stream did not start."));
            }
        }
        // The page, the controllers or the browser may have changed while the screencast started.
        let (superseded, current) = {
            let state = bot.state.borrow();
            let superseded = state.stream.run != run;
            let current = !superseded
                && !state.dead
                && !state.access.private_active()
                && !state.stream.controllers.is_empty();
            (superseded, current)
        };
        if !current {
            self.stop_screencast(&tab).await;
            if !superseded {
                let _ = self.set_streaming(&bot.identity.id, false);
            }
            return Ok(());
        }
        bot.state.borrow_mut().stream.source = Some(Source {
            page_id,
            tab_id: tab.id.clone(),
            throttle: Throttle::new(max_fps),
            newest: Newest::default(),
            started: Instant::now(),
        });
        Ok(())
    }

    /// Stops the screencast that runs, if any, and forgets it. The controllers stay.
    async fn stop_source(&self, bot: &Rc<Bot>) {
        let tab_id = {
            let mut state = bot.state.borrow_mut();
            state.stream.run += 1;
            state.stream.source.take().map(|source| source.tab_id)
        };
        if let Some(tab_id) = tab_id {
            let tab = bot
                .state
                .borrow()
                .tabs
                .iter()
                .find(|tab| tab.id == tab_id)
                .cloned();
            if let Some(tab) = tab {
                self.stop_screencast(&tab).await;
            }
        }
    }

    async fn stop_screencast(&self, tab: &Rc<Tab>) {
        let _ = self
            .within(
                STOP_LIMIT,
                cdp::call::<PageStopScreencast>(&tab.webview, &NoParams {}),
            )
            .await;
    }

    /// Stops the screencast and forgets the controllers. Whoever was watching hears of it once, unless nobody was.
    pub(super) async fn stop_stream(&self, bot: &Rc<Bot>, reason: StreamEnd) {
        let watched = {
            let mut state = bot.state.borrow_mut();
            let watched = !state.stream.controllers.is_empty();
            state.stream.controllers.clear();
            watched
        };
        self.stop_source(bot).await;
        let _ = self.set_streaming(&bot.identity.id, false);
        if watched && let Some(on_stream) = self.config.on_stream.clone() {
            let _ = on_stream(NativeBrowserStreamMessage::StreamEnd {
                bot_id: bot.identity.id.clone(),
                reason,
            })
            .await;
        }
    }

    /// One screencast frame from a page: acknowledged at once (Chromium sends no further frame until it is), then
    /// throttled, validated and handed to the delivery, which runs beside this and keeps only the newest waiting.
    pub(super) fn on_screencast_frame(self: &Rc<Self>, bot_id: &str, tab_id: &str, json: &str) {
        let Some(bot) = self.live_bot(bot_id) else {
            return;
        };
        let tab = bot
            .state
            .borrow()
            .tabs
            .iter()
            .find(|tab| tab.id == tab_id)
            .cloned();
        let Some(tab) = tab else {
            return;
        };
        let Ok(frame) = serde_json::from_str::<ScreencastFrame>(json) else {
            return;
        };
        if let Some(session_id) = frame.session_id {
            // Fire and forget: the command is issued by this first poll, and its answer is not waited for.
            let _ = cdp::call::<PageScreencastFrameAck>(&tab.webview, &AckParams { session_id })
                .now_or_never();
        }
        let work = {
            let mut state = bot.state.borrow_mut();
            let run = state.stream.run;
            let Some(source) = state.stream.source.as_mut() else {
                return;
            };
            if source.tab_id != tab_id {
                return;
            }
            let at = source.started.elapsed().as_secs_f64() * 1000.0;
            if !source.throttle.ready(at) {
                return;
            }
            let Some(valid) = validate(&frame) else {
                return;
            };
            source.throttle.mark(at);
            source
                .newest
                .offer(valid)
                .map(|first| (run, source.page_id.clone(), first))
        };
        let Some((run, page_id, first)) = work else {
            return;
        };
        if let Some(spawn) = self.config.spawn.clone() {
            let (engine, bot) = (self.clone(), bot.clone());
            spawn(Box::pin(async move {
                engine.deliver(&bot, run, &page_id, first).await;
            }));
        }
    }

    /// Delivers `first`, then whichever frame arrived last while it was being delivered, until none waits.
    async fn deliver(&self, bot: &Rc<Bot>, run: u64, page_id: &str, first: Frame) {
        let mut frame = Some(first);
        while let Some(current) = frame.take() {
            if !self.stream_current(bot, run) {
                return;
            }
            self.forward_frame(bot, page_id, current).await;
            frame = {
                let mut state = bot.state.borrow_mut();
                if state.stream.run == run {
                    state
                        .stream
                        .source
                        .as_mut()
                        .and_then(|source| source.newest.delivered())
                } else {
                    None
                }
            };
        }
    }

    /// Whether the stream of `run` may still deliver: the browser is open, not private, and no newer run replaced it.
    fn stream_current(&self, bot: &Rc<Bot>, run: u64) -> bool {
        let state = bot.state.borrow();
        state.stream.run == run && !state.dead && !state.access.private_active()
    }

    async fn forward_frame(&self, bot: &Rc<Bot>, page_id: &str, frame: Frame) {
        let message = {
            let mut state = bot.state.borrow_mut();
            // A frame of a page that is no longer active, or one taken as privacy began, is dropped here.
            if state.dead
                || state.active.as_deref() != Some(page_id)
                || state.access.private_active()
            {
                return;
            }
            let ticket = state.access.ticket();
            state.frame_sequence += 1;
            NativeBrowserStreamMessage::StreamFrame {
                bot_id: bot.identity.id.clone(),
                frame: BrowserFrame {
                    data: frame.data,
                    mime: FrameMime::Jpeg,
                    width: frame.width,
                    height: frame.height,
                    sequence: state.frame_sequence,
                    viewport_width: Some(frame.device_width),
                    viewport_height: Some(frame.device_height),
                    page_id: Some(page_id.to_string()),
                    viewport_revision: Some(state.viewport_revision),
                    stream_id: Some(format!(
                        "{}:{}:{}",
                        state.generation, page_id, ticket.private_epoch
                    )),
                    control_epoch: Some(ticket.control_epoch),
                    private_epoch: Some(ticket.private_epoch),
                },
            }
        };
        let Some(on_stream) = self.config.on_stream.clone() else {
            return;
        };
        let accepted = on_stream(message).await;
        // Nobody is watching this bot at the daemon (it restarted, or the viewers left): stop sending.
        if !accepted && !bot.state.borrow().dead {
            bot.state.borrow_mut().stream.controllers.clear();
            self.kick_stream(bot);
        }
    }

    // ---- remote actions --------------------------------------------------------------------------------------

    /// `stream`: a controller turns its live view on (at `max_fps`) or off.
    pub(super) async fn remote_stream(
        self: &Rc<Self>,
        bot: &Rc<Bot>,
        owner: &str,
        on: bool,
        max_fps: Option<u32>,
        connection: Check<'_>,
    ) -> Result<BrowserState, EngineError> {
        {
            let mut state = bot.state.borrow_mut();
            if !on {
                state.stream.controllers.remove(owner);
            } else if !state.access.private_active() && state.active.is_some() {
                // While a private interaction is on there is nothing to show: no controller is registered, and the
                // state says why.
                state
                    .stream
                    .controllers
                    .insert(owner.to_string(), max_fps.unwrap_or(60).clamp(1, 60));
            }
        }
        let started = self.sync_stream(bot).await.and_then(|()| connection());
        if let Err(error) = started {
            // A stream that could not start must not keep this controller registered.
            bot.state.borrow_mut().stream.controllers.remove(owner);
            self.kick_stream(bot);
            return Err(error);
        }
        Ok(self.browser_state(bot, owner))
    }

    /// A controller leaves: its stream is over, and the handback that follows must not wait for the screencast to stop.
    pub(super) fn drop_controller_stream(&self, bot: &Rc<Bot>, owner: &str) {
        bot.state.borrow_mut().stream.controllers.remove(owner);
        self.kick_stream(bot);
    }

    /// `frame`: one picture of the active page, as a JPEG with the CSS viewport it was taken at, or a thumbnail of it.
    pub(super) async fn remote_frame(
        self: &Rc<Self>,
        bot: &BotIdentity,
        owner: &str,
        connection: Check<'_>,
        thumbnail: bool,
    ) -> Result<BrowserFrameResult, EngineError> {
        connection()?;
        self.open(bot).await?;
        let rc = self.bot(&bot.id)?;
        let (ticket, page_id, revision) = {
            let state = rc.state.borrow();
            (
                state.access.ticket(),
                state.active.clone(),
                state.viewport_revision,
            )
        };
        let page_id = match page_id {
            Some(page_id) if !rc.state.borrow().access.private_active() => page_id,
            _ => {
                return Ok(BrowserFrameResult {
                    state: self.browser_state(&rc, owner),
                    frame: None,
                    unchanged: None,
                });
            }
        };
        let check = || -> Result<(), EngineError> {
            connection()?;
            let state = rc.state.borrow();
            state.access.check(ticket, false)?;
            if state.dead
                || state.active.as_deref() != Some(page_id.as_str())
                || state.viewport_revision != revision
            {
                return Err(EngineError::refused(
                    "The browser page or viewport changed; this frame was discarded.",
                ));
            }
            Ok(())
        };
        // A capture is a command like any other: it takes its turn and has the page woken for it.
        let guard = self.begin_tool(bot).await?;
        let result = self
            .enqueue(&rc, async {
                check()?;
                let tab = self.tab(&rc, Some(&page_id))?;
                let picture = self.capture(&tab, thumbnail, &check).await?;
                check()?;
                let mut state = self.browser_state(&rc, owner);
                let busy = {
                    let inner = rc.state.borrow();
                    inner.busy > 1 || inner.handover > 0
                };
                state.busy = Some(busy);
                let mut inner = rc.state.borrow_mut();
                inner.frame_sequence += 1;
                let frame = BrowserFrame {
                    data: picture.data,
                    mime: FrameMime::Jpeg,
                    width: picture.width,
                    height: picture.height,
                    sequence: inner.frame_sequence,
                    viewport_width: Some(picture.viewport.0),
                    viewport_height: Some(picture.viewport.1),
                    page_id: Some(page_id.clone()),
                    viewport_revision: Some(revision),
                    stream_id: Some(format!(
                        "{}:{}:{}",
                        inner.generation, page_id, ticket.private_epoch
                    )),
                    control_epoch: Some(ticket.control_epoch),
                    private_epoch: Some(ticket.private_epoch),
                };
                Ok(BrowserFrameResult {
                    state,
                    frame: Some(frame),
                    unchanged: None,
                })
            })
            .await;
        guard.end();
        result
    }

    /// A JPEG of the page (or a thumbnail of it) and the CSS viewport it shows.
    async fn capture(
        &self,
        tab: &Rc<Tab>,
        thumbnail: bool,
        check: Check<'_>,
    ) -> Result<Picture, EngineError> {
        let bad = |message: &str| EngineError::refused(message);
        let timed_out = || EngineError::refused("The browser frame timed out.");
        check()?;
        let metrics = self
            .within(
                CAPTURE_LIMIT,
                cdp::call::<PageGetLayoutMetrics>(&tab.webview, &NoParams {}),
            )
            .await
            .ok_or_else(timed_out)??
            .css_visual_viewport;
        check()?;
        let (width, height) = (metrics.client_width.round(), metrics.client_height.round());
        if !(1.0..=f64::from(MAX_SIDE)).contains(&width)
            || !(1.0..=f64::from(MAX_SIDE)).contains(&height)
        {
            return Err(bad("The browser CSS viewport exceeds its limit."));
        }
        let viewport = (width as u32, height as u32);
        let shoot = |scale: Option<f64>| {
            let clip = scale.map(|scale| Clip {
                x: metrics.page_x,
                y: metrics.page_y,
                width,
                height,
                scale,
            });
            CaptureScreenshotParams {
                format: "jpeg".to_string(),
                quality: Some(if thumbnail { 50 } else { 60 }),
                clip,
                capture_beyond_viewport: false,
            }
        };
        let take = |params: CaptureScreenshotParams| async move {
            let reply = self
                .within(
                    CAPTURE_LIMIT,
                    cdp::call::<PageCaptureScreenshot>(&tab.webview, &params),
                )
                .await
                .ok_or_else(timed_out)??;
            check()?;
            let head: String = reply.data.chars().take(65536).collect();
            let bytes =
                base64::Engine::decode(&base64::engine::general_purpose::STANDARD, head.as_bytes())
                    .map_err(|_| bad("The browser frame was invalid or exceeded 2 MiB."))?;
            let (w, h) = jpeg_size(&bytes)
                .ok_or_else(|| bad("The browser returned an invalid frame size."))?;
            if reply.data.len() > crate::browser::stream::IMAGE_LIMIT.div_ceil(3) * 4
                || w > MAX_SIDE
                || h > MAX_SIDE
            {
                return Err(bad("The browser returned an invalid frame size."));
            }
            Ok::<_, EngineError>((reply.data, w, h))
        };
        if !thumbnail {
            let (data, w, h) = take(shoot(None)).await?;
            return Ok(Picture {
                data,
                width: w,
                height: h,
                viewport,
            });
        }
        // A thumbnail fits 320 by 240: ask for the scale that does, and once more if the picture came out larger
        // (the scale is of CSS pixels, and a screen's own scaling is its business).
        let mut scale = (THUMBNAIL.0 / width).min(THUMBNAIL.1 / height).min(1.0);
        let mut taken = take(shoot(Some(scale))).await?;
        if f64::from(taken.1) > THUMBNAIL.0 || f64::from(taken.2) > THUMBNAIL.1 {
            let fit = (THUMBNAIL.0 / f64::from(taken.1)).min(THUMBNAIL.1 / f64::from(taken.2));
            scale *= fit * 0.98;
            taken = take(shoot(Some(scale))).await?;
        }
        let (data, w, h) = taken;
        if data.len() / 4 * 3 > THUMBNAIL_BYTES {
            return Err(bad("The browser thumbnail exceeded 64 KiB."));
        }
        Ok(Picture {
            data,
            width: w,
            height: h,
            viewport,
        })
    }

    // ---- private interaction ---------------------------------------------------------------------------------

    /// A private interaction begins (the vault fills a login): the owner must hold the browser at `control_epoch`. What
    /// the bot could see or do is cut off first: held input is released, the page's script is stopped, and the
    /// network and runtime domains are switched off, so nothing typed or filled can reach a tool or a log. The stream
    /// ends (its watchers are told why) and snapshots and frames are refused until it ends.
    pub async fn begin_private(
        self: &Rc<Self>,
        bot: &BotIdentity,
        owner: &str,
        control_epoch: u64,
    ) -> Result<u64, EngineError> {
        let guard = self.begin_tool(bot).await?;
        let rc = guard.bot.clone();
        let result = self.begin_private_open(&rc, owner, control_epoch).await;
        guard.end();
        result
    }

    async fn begin_private_open(
        self: &Rc<Self>,
        bot: &Rc<Bot>,
        owner: &str,
        control_epoch: u64,
    ) -> Result<u64, EngineError> {
        let epoch = bot
            .state
            .borrow_mut()
            .access
            .begin_private(owner, control_epoch)?;
        for tab in self.tabs_of(bot) {
            self.release(bot, &tab).await;
            let unsafe_to_enter =
                || EngineError::refused("The browser could not enter private mode safely.");
            self.within(
                PRIVATE_LIMIT,
                cdp::call::<PageStopScreencast>(&tab.webview, &NoParams {}),
            )
            .await
            .ok_or_else(unsafe_to_enter)??;
            self.within(
                PRIVATE_LIMIT,
                cdp::call::<RuntimeTerminateExecution>(&tab.webview, &NoParams {}),
            )
            .await
            .ok_or_else(unsafe_to_enter)??;
            self.within(
                PRIVATE_LIMIT,
                cdp::call::<NetworkDisable>(&tab.webview, &NoParams {}),
            )
            .await
            .ok_or_else(unsafe_to_enter)??;
            self.within(
                PRIVATE_LIMIT,
                cdp::call::<RuntimeDisable>(&tab.webview, &NoParams {}),
            )
            .await
            .ok_or_else(unsafe_to_enter)??;
        }
        self.changed(bot);
        // The stream ends now, not at the next event: its watchers hear that a private interaction began.
        self.sync_stream(bot).await?;
        Ok(epoch)
    }

    /// A private interaction ends. `safe_to_resume` first verifies, in the page, that the fields were cleared or
    /// submitted; the bot gets the browser back only if it says so.
    pub async fn end_private(
        self: &Rc<Self>,
        bot: &BotIdentity,
        owner: &str,
        private_epoch: u64,
        safe_to_resume: impl Future<Output = Result<(), EngineError>>,
    ) -> Result<BrowserState, EngineError> {
        let rc = self.bot(&bot.id)?;
        safe_to_resume.await?;
        rc.state
            .borrow_mut()
            .access
            .end_private(owner, private_epoch)?;
        self.changed(&rc);
        Ok(self.browser_state(&rc, owner))
    }

    /// The daemon restarted: every remote controller's stream went with its connection.
    pub(super) fn drop_remote_streams(&self, bot: &Rc<Bot>) {
        bot.state
            .borrow_mut()
            .stream
            .controllers
            .retain(|owner, _| owner == LOCAL);
        self.kick_stream(bot);
    }
}

/// One picture of the page.
struct Picture {
    data: String,
    width: u32,
    height: u32,
    viewport: (u32, u32),
}
