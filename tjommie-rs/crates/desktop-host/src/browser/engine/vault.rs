//! The browser's side of the login vault (`native-browser.ts`: `vaultDestination`, `beginVault`, `writeVault`,
//! `finishVault`). These are capabilities of the app's own process, never browser tools or renderer commands: a fill goes
//! from the vault, through a private interaction, into the page's login form, and nothing the bot can see or do is in the way.

use protocol::vault::{LoginOrigin, VaultFillTarget};

use super::*;
use crate::vault::fill::{self, FillError, LoginFields};
use crate::vault::storage::VaultSecret;

/// What a private login interaction holds between its steps.
pub(super) struct LoginState {
    fields: LoginFields,
    private_epoch: u64,
    control_epoch: u64,
    page_id: String,
}

/// How a private login ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaultFinish {
    /// The fields are cleared and shown to be empty, and the person may resume.
    Clear,
    /// The browser is closed, which clears everything.
    Close,
}

fn changed(error: FillError) -> EngineError {
    EngineError::Refused(error.to_string())
}

fn refusal(error: EngineError) -> FillError {
    FillError::Changed(error.to_string())
}

impl Engine {
    fn tab_handle(tab: &Rc<Tab>) -> TabHandle {
        TabHandle {
            id: tab.id.clone(),
            webview: tab.webview.clone(),
            tab: tab.clone(),
        }
    }

    /// The HTTPS origin of the page a fill is aimed at, if the aim is still true: the target's generation and control epoch
    /// are current, no private interaction is on, the page is the active one, and its address is an ordinary HTTPS one.
    pub fn vault_destination(
        &self,
        bot: &BotIdentity,
        target: &VaultFillTarget,
        owner: &str,
    ) -> Result<LoginOrigin, EngineError> {
        let rc = self.bot(&bot.id)?;
        self.check_generation(&rc, &target.generation)?;
        let page = {
            let state = rc.state.borrow();
            state.access.require(owner, target.control_epoch)?;
            if state.access.private_active()
                || state.active.as_deref() != Some(target.page_id.as_str())
                || target.bot_id != bot.id
            {
                return Err(EngineError::refused(
                    "The login destination changed. Refresh the browser before filling.",
                ));
            }
            self.tab(&rc, Some(&target.page_id))?
        };
        let url = com::source(&page.webview)?;
        let origin = url::Url::parse(&url)
            .ok()
            .filter(|url| {
                url.scheme() == "https" && url.username().is_empty() && url.password().is_none()
            })
            .map(|url| url.origin().ascii_serialization())
            .unwrap_or_default();
        LoginOrigin::new(origin).map_err(|error| EngineError::Refused(error.to_string()))
    }

    /// Begins a private fill of the page `target` names, for a login that belongs to `origin`: the person at the host takes
    /// the browser (a confirmation at the host is the person's), the private interaction starts, and the form's fields are
    /// found. Returns the private epoch the following steps must name.
    pub async fn begin_vault(
        self: &Rc<Self>,
        bot: &BotIdentity,
        target: &VaultFillTarget,
        owner: &str,
        origin: &LoginOrigin,
        check: Check<'_>,
    ) -> Result<u64, EngineError> {
        check()?;
        if self.vault_destination(bot, target, owner)?.as_str() != origin.as_str() {
            return Err(EngineError::refused(
                "This login belongs to a different HTTPS origin.",
            ));
        }
        let rc = self.bot(&bot.id)?;
        if owner != LOCAL {
            // A host-native confirmation transfers remote control to the person at the host.
            let mut state = rc.state.borrow_mut();
            state.access.release(owner);
            state.access.take(LOCAL)?;
        }
        let control_epoch = rc.state.borrow().access.control_epoch();
        let epoch = self.begin_private(bot, LOCAL, control_epoch).await?;
        let control_epoch = rc.state.borrow().access.control_epoch();
        let page_id = target.page_id.clone();
        let current = || -> Result<(), EngineError> {
            check()?;
            let state = rc.state.borrow();
            if state.dead
                || state.access.private_epoch() != epoch
                || !state.access.private_active()
                || state.access.owner() != Some(LOCAL)
                || state.access.control_epoch() != control_epoch
                || state.active.as_deref() != Some(page_id.as_str())
            {
                return Err(EngineError::refused(
                    "The private login destination changed.",
                ));
            }
            Ok(())
        };
        let tab = Self::tab_handle(&self.tab(&rc, Some(&target.page_id))?);
        current()?;
        let fields = fill::prepare(&tab, origin.as_str(), &|| current().map_err(refusal))
            .await
            .map_err(changed)?;
        current()?;
        rc.state.borrow_mut().login = Some(LoginState {
            fields,
            private_epoch: epoch,
            control_epoch,
            page_id: target.page_id.clone(),
        });
        Ok(epoch)
    }

    /// Types `secret` into the form that `begin_vault` found, if the interaction is still the one that began.
    pub async fn write_vault(
        self: &Rc<Self>,
        bot: &BotIdentity,
        epoch: u64,
        secret: &VaultSecret,
        check: Check<'_>,
    ) -> Result<(), EngineError> {
        let rc = self.bot(&bot.id)?;
        let (fields, control_epoch, page_id) = {
            let state = rc.state.borrow();
            match state
                .login
                .as_ref()
                .filter(|login| login.private_epoch == epoch)
            {
                Some(login) => (
                    login.fields.clone(),
                    login.control_epoch,
                    login.page_id.clone(),
                ),
                None => {
                    return Err(EngineError::refused(
                        "The private login interaction changed.",
                    ));
                }
            }
        };
        let current = || -> Result<(), EngineError> {
            check()?;
            let state = rc.state.borrow();
            if state.dead
                || !state.access.private_active()
                || state.access.private_epoch() != epoch
                || state.access.owner() != Some(LOCAL)
                || state.access.control_epoch() != control_epoch
                || state.active.as_deref() != Some(page_id.as_str())
            {
                return Err(EngineError::refused(
                    "The private login destination changed.",
                ));
            }
            Ok(())
        };
        current()?;
        let tab = Self::tab_handle(&self.tab(&rc, Some(&page_id))?);
        fill::fill(&tab, &fields, secret, &|| current().map_err(refusal))
            .await
            .map_err(changed)?;
        current()
    }

    /// Ends a private login: the fields are cleared and verified so the person may resume, or the browser is closed.
    pub async fn finish_vault(
        self: &Rc<Self>,
        bot: &BotIdentity,
        generation: &str,
        epoch: u64,
        mode: VaultFinish,
        check: Check<'_>,
    ) -> Result<(), EngineError> {
        let rc = self.bot(&bot.id)?;
        self.check_generation(&rc, generation)?;
        check()?;
        {
            let state = rc.state.borrow();
            if !state.access.private_active() || state.access.private_epoch() != epoch {
                return Err(EngineError::refused(
                    "The private login interaction changed.",
                ));
            }
        }
        if mode == VaultFinish::Close {
            return self.close_bot(&bot.id).await;
        }
        let (fields, page_id) = {
            let state = rc.state.borrow();
            match state
                .login
                .as_ref()
                .filter(|login| login.private_epoch == epoch)
            {
                Some(login) => (login.fields.clone(), login.page_id.clone()),
                None => {
                    return Err(EngineError::refused(
                        "Close this private browser before resuming; its fields cannot be verified.",
                    ));
                }
            }
        };
        rc.state.borrow_mut().access.take(LOCAL)?;
        let current = || -> Result<(), EngineError> {
            check()?;
            let state = rc.state.borrow();
            if state.dead || !state.access.private_active() || state.access.private_epoch() != epoch
            {
                return Err(EngineError::refused(
                    "The private login interaction changed.",
                ));
            }
            Ok(())
        };
        let tab = Self::tab_handle(&self.tab(&rc, Some(&page_id))?);
        self.end_private(bot, LOCAL, epoch, async {
            fill::clear(&tab, &fields, &|| current().map_err(refusal))
                .await
                .map_err(changed)
        })
        .await?;
        rc.state.borrow_mut().login = None;
        // The person still explicitly returns browser control after reviewing the cleared page.
        Ok(())
    }
}
