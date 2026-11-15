//! One tool call's context (`Context` in `native-browser-automation.ts`): its 22-second deadline, its cancellation, the
//! epochs it started under, and the DevTools call that is bounded, interruptible by a page dialog and checked before and
//! after. Time here is the real clock: the engine's clock is a seam that tests move, and a page does not wait on it.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::future::{Either, select};
use protocol::browser::BrowserControl;

use super::protocol::RuntimeTerminateExecution;
use super::reply::ToolError;
use crate::browser::BotIdentity;
use crate::browser::cdp::{Method, NoParams};
use crate::browser::engine::{Engine, TabHandle};

/// How long one DevTools command may take.
pub const ACTION: Duration = Duration::from_secs(10);
/// The whole call: past it, the tool says so and stops.
pub const DEADLINE: Duration = Duration::from_secs(22);
/// How often a waiting command looks at the guard and at the page's dialogs.
const POLL: Duration = Duration::from_millis(50);
/// How long cleanup commands may take; they never hold the tool.
const CLEANUP: Duration = Duration::from_millis(500);

pub struct Context {
    pub engine: Rc<Engine>,
    pub bot: BotIdentity,
    pub deadline: Instant,
    /// Set when the call is cancelled (`cancel_tools`, or the person took control).
    pub cancelled: Rc<Cell<bool>>,
    pub control_epoch: Cell<Option<u64>>,
    pub private_epoch: Cell<Option<u64>>,
    pub viewport_revision: Cell<Option<u64>>,
}

impl Context {
    pub fn new(engine: Rc<Engine>, bot: BotIdentity, cancelled: Rc<Cell<bool>>) -> Self {
        Self {
            engine,
            bot,
            deadline: Instant::now() + DEADLINE,
            cancelled,
            control_epoch: Cell::new(None),
            private_epoch: Cell::new(None),
            viewport_revision: Cell::new(None),
        }
    }

    /// Fails if this call may no longer act: cancelled, a private interaction began or ended, the person took or returned
    /// the browser, the panel resized, or the deadline passed. Run before and after every step.
    pub fn guard(&self) -> Result<(), ToolError> {
        if self.cancelled.get() {
            return Err(ToolError::new(
                "The browser tool was cancelled. Take a fresh snapshot before continuing.",
            ));
        }
        let view = self.engine.tool_view(&self.bot.id)?;
        if view.private_active
            || self
                .private_epoch
                .get()
                .is_some_and(|epoch| epoch != view.private_epoch)
            || self
                .control_epoch
                .get()
                .is_some_and(|epoch| epoch != view.control_epoch)
        {
            return Err(ToolError::new(
                "The browser interaction changed; this tool result was discarded.",
            ));
        }
        if view.control != BrowserControl::Bot {
            return Err(ToolError::new(
                "The person has taken browser control. Return control to the agent before continuing.",
            ));
        }
        if self
            .viewport_revision
            .get()
            .is_some_and(|revision| revision != view.viewport_revision)
        {
            return Err(ToolError::new(
                "The browser panel resized during this action. Take a fresh snapshot before retrying.",
            ));
        }
        if Instant::now() >= self.deadline {
            return Err(ToolError::new(
                "The browser tool exceeded its 22-second deadline. Take a fresh snapshot before retrying.",
            ));
        }
        Ok(())
    }

    /// Whether the page is blocked on a JavaScript dialog (also true if the bot's browser is gone).
    pub fn dialog_open(&self) -> bool {
        self.engine
            .tool_view(&self.bot.id)
            .map_or(true, |view| view.dialog.is_some())
    }

    /// Waits `ms` milliseconds, checking the guard every 50.
    pub async fn delay(&self, ms: u64) -> Result<(), ToolError> {
        let end = Instant::now() + Duration::from_millis(ms);
        loop {
            self.guard()?;
            let left = end.saturating_duration_since(Instant::now());
            self.engine.sleep(left.min(POLL)).await;
            if Instant::now() >= end {
                break;
            }
        }
        self.guard()
    }

    /// One typed DevTools command, bounded to 10 s, ended early by a page dialog or a failed guard, and checked before
    /// and after. A page-script command that did not finish is terminated, so it cannot run on after the tool.
    pub async fn cdp<M: Method>(
        &self,
        tab: &TabHandle,
        params: &M::Params,
    ) -> Result<M::Reply, ToolError> {
        self.guard()?;
        let deadline = Instant::now() + ACTION;
        let call = tab.cdp::<M>(params);
        futures::pin_mut!(call);
        let mut dialog = false;
        let outcome = loop {
            match select(call.as_mut(), self.engine.sleep(POLL)).await {
                // WebView2 words every protocol error "The parameter is incorrect": the method says which call it was.
                Either::Left((reply, _)) => {
                    break reply.map_err(|error| ToolError(format!("{}: {error}", M::NAME)));
                }
                Either::Right(((), _)) => {
                    if let Err(error) = self.guard() {
                        break Err(error);
                    }
                    if self.dialog_open() {
                        dialog = true;
                        break Err(ToolError::new(
                            "The page opened a dialog. Use browser_handle_dialog before continuing.",
                        ));
                    }
                    if Instant::now() >= deadline {
                        break Err(ToolError::new(
                            "The browser operation timed out. Check for a page dialog, then take a fresh snapshot.",
                        ));
                    }
                }
            }
        };
        match outcome {
            Ok(reply) => {
                self.guard()?;
                Ok(reply)
            }
            Err(error) => {
                // End a running page script before the tool lets go of the page (a dialog needs the page as it is).
                let script = matches!(M::NAME, "Runtime.evaluate" | "Runtime.callFunctionOn");
                if script && !dialog && !self.dialog_open() {
                    self.cleanup::<RuntimeTerminateExecution>(tab, &NoParams {})
                        .await;
                }
                Err(error)
            }
        }
    }

    /// A command whose failure and delay do not matter: it is given half a second and its answer is dropped.
    pub async fn cleanup<M: Method>(&self, tab: &TabHandle, params: &M::Params) {
        let call = tab.cdp::<M>(params);
        futures::pin_mut!(call);
        let _ = select(call.as_mut(), self.engine.sleep(CLEANUP)).await;
    }
}
