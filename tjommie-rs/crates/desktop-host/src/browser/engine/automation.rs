//! What the browser tools need from the engine (`withAutomation` in `native-browser.ts`): a session on a bot's page that
//! holds the bot's command turn, the awake-and-parked guarantee and an access ticket for as long as the tool runs, and a
//! cheap look at who controls the browser.

use futures::future::LocalBoxFuture;
use protocol::browser::{BrowserControl, BrowserDialog};
use tokio::sync::OwnedMutexGuard;

use super::*;
use crate::browser::access::Ticket;

/// Who controls a bot's browser, without touching the page: what a tool checks before and after every step.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolView {
    pub control: BrowserControl,
    pub control_epoch: u64,
    pub private_epoch: u64,
    pub private_active: bool,
    pub viewport_revision: u64,
    pub dialog: Option<BrowserDialog>,
}

/// A tool at work on a bot's active page. While it lives the page is awake and parked (or in the panel), no other
/// command of the bot runs, and the engine reports the bot busy. End it with [`finish`](Self::finish) to check that
/// nothing changed under it; dropping it only lets go.
pub struct AutomationSession {
    engine: Rc<Engine>,
    bot: Rc<Bot>,
    guard: Option<ToolGuard>,
    turn: Option<OwnedMutexGuard<()>>,
    ticket: Ticket,
    tab: TabHandle,
}

impl AutomationSession {
    pub fn tab(&self) -> &TabHandle {
        &self.tab
    }

    /// A tab of the bot by id (a tool's target), for the rest of this session.
    pub fn tab_by_id(&self, id: &str) -> Result<TabHandle, EngineError> {
        self.guard
            .as_ref()
            .ok_or_else(|| EngineError::refused("The agent browser is closing."))?
            .tab_by_id(id)
    }

    /// Ends the session: refused if control or privacy changed while it ran, so its result is discarded.
    pub fn finish(mut self) -> Result<(), EngineError> {
        let checked = self.bot.state.borrow().access.check(self.ticket, true);
        self.release();
        checked.map_err(EngineError::from)
    }

    fn release(&mut self) {
        if let Some(turn) = self.turn.take() {
            let engine = self.engine.clone();
            {
                let mut state = self.bot.state.borrow_mut();
                state.busy = state.busy.saturating_sub(1);
            }
            if !self.bot.state.borrow().dead {
                engine.changed(&self.bot);
            }
            drop(turn);
        }
        if let Some(guard) = self.guard.take() {
            guard.end();
        }
    }
}

impl Drop for AutomationSession {
    fn drop(&mut self) {
        if self.turn.is_none() {
            return;
        }
        // Dropped without `finish` (a cancelled task): let go without touching COM from here; the guard's own drop asks
        // the host to look at the bot again.
        if let Ok(mut state) = self.bot.state.try_borrow_mut() {
            state.busy = state.busy.saturating_sub(1);
        }
        let _ = self.engine.events.try_send(HostEvent::Reapply {
            bot_id: self.bot.identity.id.clone(),
        });
    }
}

impl Engine {
    /// A bot's control state, read without touching the page.
    pub fn tool_view(&self, bot_id: &str) -> Result<ToolView, EngineError> {
        let bot = self.bot(bot_id)?;
        let state = bot.state.borrow();
        Ok(ToolView {
            control: state.access.control(),
            control_epoch: state.access.control_epoch(),
            private_epoch: state.access.private_epoch(),
            private_active: state.access.private_active(),
            viewport_revision: state.viewport_revision,
            dialog: state.dialog.clone(),
        })
    }

    /// The ids of a bot's tabs, in order: a tool's per-tab memory is dropped for the ones that are gone.
    pub fn tab_ids(&self, bot_id: &str) -> Vec<String> {
        self.bot(bot_id)
            .map(|bot| {
                bot.state
                    .borrow()
                    .tabs
                    .iter()
                    .map(|tab| tab.id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Waits without blocking (the host's timer).
    pub fn sleep(&self, duration: Duration) -> LocalBoxFuture<'static, ()> {
        (self.config.sleep)(duration)
    }

    /// The engine's clock.
    pub fn clock(&self) -> Instant {
        self.now()
    }

    /// Starts a tool on a bot's page: the bot must be under its own control, the tool takes the bot's turn (one command
    /// at a time), and the page is woken and parked first. The first of the bot's tabs, or `page_id`, is the target.
    pub async fn automation(
        self: &Rc<Self>,
        bot: &BotIdentity,
    ) -> Result<AutomationSession, EngineError> {
        let guard = self.begin_tool(bot).await?;
        let rc = guard.bot.clone();
        self.require_control(&rc, Actor::Bot)?;
        let ticket = rc.state.borrow().access.ticket();
        let turn = rc.queue.clone().lock_owned().await;
        if rc.state.borrow().dead {
            return Err(EngineError::refused("The agent browser is closing."));
        }
        rc.state.borrow().access.check(ticket, true)?;
        self.require_control(&rc, Actor::Bot)?;
        rc.state.borrow_mut().busy += 1;
        self.changed(&rc);
        let tab = guard.tab()?;
        Ok(AutomationSession {
            engine: self.clone(),
            bot: rc,
            guard: Some(guard),
            turn: Some(turn),
            ticket,
            tab,
        })
    }
}
