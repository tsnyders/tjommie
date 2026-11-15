//! Where the keyboard and the pixels go between the page and the app (Phase 3 plan T3.5; GPUI limits 8, 9, 10 and 16):
//! the shield that stands in for the page under a menu, focus leaving the page by the keyboard or a click, the app's
//! chords taken before the page sees them, and the page placed again when the display scale changes.

use std::time::Duration;

use super::*;
use crate::browser::cdp::NoParams;
use crate::browser::keyboard::{
    AddBindingParams, AddScriptParams, BINDING, BindingCalled, Chord, FocusDirection,
    HANDOFF_SCRIPT, PageAddScriptToEvaluateOnNewDocument, RuntimeAddBinding, WORLD,
};
use crate::browser::tools::protocol::{PageEnable, RuntimeEnable};

/// How long a preview may take.
const PREVIEW_LIMIT: Duration = Duration::from_secs(3);

/// What stands in for a page while a menu or dialog is over its area: a picture of it, taken while it was still visible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shield {
    /// A PNG of the page as it was. Empty if the browser could not take one: the app then draws a plain cover.
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// The pixel size of a PNG, from its header.
pub fn png_size(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    if bytes.get(..8)? != SIGNATURE {
        return None;
    }
    let number = |at: usize| -> Option<u32> {
        let four: [u8; 4] = bytes.get(at..at + 4)?.try_into().ok()?;
        Some(u32::from_be_bytes(four))
    };
    Some((number(16)?, number(20)?))
}

impl Engine {
    /// Raises the shield over a bot's page: a picture of the page is taken now, while it is visible, and then its window
    /// is hidden, so the app can draw the picture in its place and a menu over it. Whatever happens the page is hidden.
    pub async fn raise_shield(self: &Rc<Self>, id: &str) -> Shield {
        let png = match self.capture_preview(id).await {
            Ok(png) => png,
            Err(error) => {
                log::warn!("browser shield without a picture: {error}");
                Vec::new()
            }
        };
        self.occlude_bot(id);
        let (width, height) = png_size(&png).unwrap_or((0, 0));
        Shield { png, width, height }
    }

    /// Lowers the shield: the live page comes back.
    pub fn lower_shield(&self, id: &str) {
        let Ok(bot) = self.bot(id) else {
            return;
        };
        if let Some(panel) = bot.state.borrow_mut().panel.as_mut() {
            panel.occluded = false;
        }
        self.apply(&bot);
    }

    /// Has every document of a page run the hand-off script in an isolated world, with a binding only that world can call.
    /// Without it Shift+Tab would stay in the page (a keyboard trap), so a failure is logged, not hidden.
    pub(super) async fn install_handoff(&self, webview: &ICoreWebView2) {
        // Chromium runs a new-document script only while the page domain is enabled.
        let enabled = cdp::call::<PageEnable>(webview, &NoParams {}).await;
        let script = cdp::call::<PageAddScriptToEvaluateOnNewDocument>(
            webview,
            &AddScriptParams {
                source: HANDOFF_SCRIPT.to_string(),
                world_name: WORLD.to_string(),
            },
        )
        .await;
        let binding = self.enable_handoff_binding(webview).await;
        if let Err(error) = enabled.map(|_| ()).and(script.map(|_| ())).and(binding) {
            log::warn!("browser keyboard hand-off is not installed: {error}");
        }
    }

    /// Puts the hand-off binding into the isolated world's contexts, present and to come. Chromium installs a binding into
    /// the contexts created after it is added only while the runtime domain is enabled, and drops it when the domain is
    /// disabled (a private interaction does), so this is also what ends a private interaction with.
    pub(super) async fn enable_handoff_binding(
        &self,
        webview: &ICoreWebView2,
    ) -> Result<(), CdpError> {
        cdp::call::<RuntimeEnable>(webview, &NoParams {}).await?;
        cdp::call::<RuntimeAddBinding>(
            webview,
            &AddBindingParams {
                name: BINDING.to_string(),
                execution_context_name: WORLD.to_string(),
            },
        )
        .await?;
        Ok(())
    }

    /// A PNG of what a bot's active page shows now (`CapturePreview`). The page must be visible to have anything to show.
    pub async fn capture_preview(&self, id: &str) -> Result<Vec<u8>, EngineError> {
        let bot = self.bot(id)?;
        let tab = self.tab(&bot, None)?;
        let preview = com::capture_preview(&tab.webview);
        futures::pin_mut!(preview);
        match futures::future::select(preview, self.sleep(PREVIEW_LIMIT)).await {
            futures::future::Either::Left((png, _)) => Ok(png?),
            futures::future::Either::Right(_) => {
                Err(EngineError::refused("The browser preview timed out."))
            }
        }
    }

    /// The keyboard belongs to the app: a click landed outside the page, or the page gave it up.
    pub fn focus_app(&self) {
        // SAFETY: a plain Win32 call on the app's own window, on the UI thread.
        unsafe {
            let _ = SetFocus(Some(self.config.parent));
        }
    }

    /// The chords the app keeps for itself while a page has the keyboard (`keymap::page_forwarded()`): each is taken
    /// before the page sees it and reported as a [`Notification::Shortcut`]. Every other key goes to the page.
    pub fn set_forwarded_chords(&self, chords: Vec<Chord>) {
        *self.forwarded.borrow_mut() = chords;
    }

    /// The window moved to a display with another scale, or the app zoom changed: every page is placed again at its
    /// panel's rectangle. The app first gives each panel its new physical rectangle (`set_panel`), and WebView2 follows
    /// the display's scale on its own.
    pub fn dpi_changed(&self) {
        let bots: Vec<Rc<Bot>> = self.bots.borrow().values().cloned().collect();
        for bot in bots {
            bot.state.borrow_mut().applied = None;
            self.apply(&bot);
        }
    }

    /// Focus leaves a bot's page by the keyboard (`direction`), if the person is the one typing into it: it goes to the
    /// app, which moves it to the next or previous control. Input a phone sends through the browser never steals it.
    pub(super) fn focus_leaves(&self, bot: &Rc<Bot>, direction: FocusDirection) {
        if !person_is_typing(bot) {
            return;
        }
        self.focus_app();
        let _ = self.notify.try_send(Notification::FocusReturn {
            bot_id: bot.identity.id.clone(),
            direction,
        });
    }

    /// The isolated world called the hand-off binding.
    pub(super) fn on_binding(&self, bot: &Rc<Bot>, json: &str) {
        if let Ok(call) = serde_json::from_str::<BindingCalled>(json)
            && call.name == BINDING
            && call.payload == "previous"
        {
            self.focus_leaves(bot, FocusDirection::Previous);
        }
    }

    /// An app chord was pressed with a page in focus. It is the app's command only if the person is the one typing:
    /// a key that reaches a page the person is not holding must not open or close a tab for them.
    pub(super) fn on_shortcut(&self, bot: &Rc<Bot>, chord: Chord) {
        if !person_is_typing(bot) {
            return;
        }
        let _ = self.notify.try_send(Notification::Shortcut {
            bot_id: bot.identity.id.clone(),
            chord,
        });
    }
}

/// Whether the bot's page is placed for the person to type into (they hold it, and it is on screen): only then is a key
/// that reaches it the person's.
fn person_is_typing(bot: &Rc<Bot>) -> bool {
    bot.state
        .borrow()
        .applied
        .as_ref()
        .is_some_and(|applied| applied.interactive)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_png_header_gives_its_size() {
        let mut bytes = vec![
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R',
        ];
        bytes.extend(1280u32.to_be_bytes());
        bytes.extend(800u32.to_be_bytes());
        assert_eq!(png_size(&bytes), Some((1280, 800)));
        assert_eq!(png_size(&bytes[..20]), None, "cut off inside the header");
        assert_eq!(png_size(b"GIF89a not a png at all"), None);
        assert_eq!(png_size(&[]), None);
    }
}
