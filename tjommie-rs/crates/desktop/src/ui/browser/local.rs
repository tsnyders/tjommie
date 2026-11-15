//! This PC's bots' browsers, as the panel drives them (Phase 3 plan T3.2–T3.5 seen from T3.7). The engine lives on the UI
//! thread with the app's window as its parent. Its WebView2 callbacks only push typed messages, which a task here applies
//! in order (the bridge rule, GPUI limit 5); what it tells the app (`Notification`) reaches the panel through this global.
//! Win32 calls made from a draw or a handler are queued with `defer`.

use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use desktop_host::browser::BotIdentity;
use desktop_host::browser::engine::{
    Actor, Engine, EngineConfig, EngineError, HostEvent, LOCAL, Notification, Panel,
};
use desktop_host::browser::keyboard::{Chord, FocusDirection};
use desktop_host::identity::Identity;
use futures::future::LocalBoxFuture;
use gpui::{AnyWindowHandle, App, Bounds, Global, Keystroke, Pixels, Window};
use protocol::browser::{BrowserControl, BrowserState};
use protocol::model::Bot;
use protocol::native_browser::NativeBrowserUiCommand;
use raw_window_handle::RawWindowHandle;
use windows::Win32::Foundation::{HWND, RECT};

use super::{BrowserCommand, BrowserView};
use crate::guard::{guarded, guarded_async};
use crate::keymap;

/// How often the engine's timers are looked at (the 10 min idle close, the 30 s low-memory target).
// ponytail: a fixed 1 s tick; a timer per `next_deadline` if the wakeups ever show in an idle CPU profile.
const TICK: Duration = Duration::from_secs(1);

pub struct LocalBrowser {
    engine: Rc<Engine>,
    /// Each bot's newest browser state, from the engine's replies and notifications.
    states: HashMap<String, BrowserState>,
    /// The bots whose pages the panel has opened, as the engine knows them.
    identities: HashMap<String, BotIdentity>,
    /// What went wrong for a bot, or what the engine says to the person about it.
    errors: HashMap<String, String>,
    /// The app chords the pages give back, with the keystroke each dispatches.
    chords: Vec<(Chord, Keystroke)>,
    /// Where the keyboard goes when it leaves a page: set by the panel as it draws its controls.
    pub focus_return: Option<FocusReturn>,
}

/// Puts the keyboard where it goes when it leaves a page, by the direction it left in.
pub type FocusReturn = Rc<dyn Fn(FocusDirection, &mut Window, &mut App)>;

impl Global for LocalBrowser {}

/// A chord as the engine matches it, for an app keystroke (`ctrl-=` is `Ctrl+=`, `escape` is `Escape`).
pub fn chord(keystroke: &Keystroke) -> Option<Chord> {
    let mut text = String::new();
    for (held, name) in [
        (keystroke.modifiers.control, "Ctrl+"),
        (keystroke.modifiers.shift, "Shift+"),
        (keystroke.modifiers.alt, "Alt+"),
    ] {
        if held {
            text.push_str(name);
        }
    }
    text.push_str(&keystroke.key);
    Chord::parse(&text)
}

/// The engine's name for a bot, from the daemon's.
pub fn identity(bot: &Bot) -> BotIdentity {
    BotIdentity {
        id: bot.id.clone(),
        name: bot.name.clone(),
        home_folder: bot.home_folder.clone(),
        project_folder: bot.project_folder.clone(),
    }
}

/// Whether `state` is no older than `shown` (`native-browser-session.ts` `#accept`): another generation, or a revision
/// at least as new.
pub fn newer(shown: Option<&BrowserState>, state: &BrowserState) -> bool {
    match shown {
        Some(shown) if shown.generation == state.generation => {
            match (shown.revision, state.revision) {
                (Some(shown), Some(state)) => state >= shown,
                _ => true,
            }
        }
        _ => true,
    }
}

/// Logical bounds to physical pixels in the window's client area, each edge rounded the way the compositor rounds it,
/// so neighbouring edges stay together (spike `physical`).
pub fn physical(bounds: Bounds<Pixels>, scale: f32) -> RECT {
    let edge = |value: Pixels| (f32::from(value) * scale).round() as i32;
    RECT {
        left: edge(bounds.origin.x),
        top: edge(bounds.origin.y),
        right: edge(bounds.origin.x + bounds.size.width),
        bottom: edge(bounds.origin.y + bounds.size.height),
    }
}

/// The part of the page window inside the panel, in the page window's own coordinates (spike `panel_bounds_clip`).
pub fn clip_to(page: &RECT, panel: &RECT) -> RECT {
    RECT {
        left: (panel.left - page.left).max(0),
        top: (panel.top - page.top).max(0),
        right: (panel.right - page.left).min(page.right - page.left),
        bottom: (panel.bottom - page.top).min(page.bottom - page.top),
    }
}

fn window_hwnd(window: &Window) -> Option<HWND> {
    match raw_window_handle::HasWindowHandle::window_handle(window)
        .ok()?
        .as_raw()
    {
        RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as *mut _)),
        _ => None,
    }
}

impl LocalBrowser {
    /// Starts the engine for `window`, with the browsers' data under `<root>/data/webview2`. Nothing of WebView2 starts
    /// until a bot's page is first shown. Without it (no window handle, a refused root) the panel says so.
    pub fn start(window: &mut Window, identity: Identity, cx: &mut App) {
        let started = Self::create(window, identity, cx);
        if let Err(error) = started {
            log::error!("The bots' browsers are off: {error}");
        }
    }

    fn create(window: &mut Window, identity: Identity, cx: &mut App) -> Result<(), String> {
        let parent = window_hwnd(window).ok_or("the window has no Win32 handle")?;
        let root = identity.data_root()?;
        let (events, inbox) = async_channel::unbounded::<HostEvent>();
        let (notify, notices) = async_channel::unbounded::<Notification>();
        let mut config = EngineConfig::new(parent, root.join("data").join("webview2"), identity);
        let executor = cx.foreground_executor().clone();
        config.spawn = Some(Rc::new(move |task: LocalBoxFuture<'static, ()>| {
            let task = guarded_async("a browser task", task);
            // unguarded: a future, not a closure; guarded_async wraps it on the line above
            executor.spawn(task).detach();
        }));
        let engine = Engine::new(config, events, notify).map_err(|error| error.to_string())?;
        let chords = keymap::page_forwarded()
            .into_iter()
            .filter_map(|keystroke| Some((chord(&keystroke)?, keystroke)))
            .collect::<Vec<_>>();
        engine.set_forwarded_chords(chords.iter().map(|(chord, _)| *chord).collect());
        cx.set_global(LocalBrowser {
            engine: engine.clone(),
            states: HashMap::new(),
            identities: HashMap::new(),
            errors: HashMap::new(),
            chords,
            focus_return: None,
        });
        let applying = engine.clone();
        cx.spawn(async move |_| {
            guarded_async("apply the browsers' events", async move {
                while let Ok(event) = inbox.recv().await {
                    applying.apply_event(event).await;
                }
            })
            .await
        })
        .detach();
        let handle = window.window_handle();
        cx.spawn(async move |cx| {
            guarded_async("hear the browsers", async move {
                while let Ok(notification) = notices.recv().await {
                    cx.update(|cx| hear(notification, handle, cx));
                }
            })
            .await
        })
        .detach();
        let ticking = engine.clone();
        cx.spawn(async move |cx| {
            guarded_async("tick the browsers", async move {
                loop {
                    cx.background_executor().timer(TICK).await;
                    ticking.tick(Instant::now()).await;
                }
            })
            .await
        })
        .detach();
        // A hidden or minimized window shows no page; a stream or a tool still keeps its page awake.
        let visible = engine.clone();
        window
            .observe_window_visibility(move |visibility, _, _| {
                guarded("follow the window's visibility", || {
                    visible.set_window_visible(visibility.is_visible())
                });
            })
            .detach();
        Ok(())
    }

    /// What the panel shows of a bot's browser: its state and what went wrong; the page itself is WebView2's.
    pub fn view(bot_id: &str, cx: &App) -> BrowserView {
        let Some(local) = cx.try_global::<LocalBrowser>() else {
            return BrowserView {
                error: Some(
                    "The browser host is offline. Reconnect to watch or take control.".into(),
                ),
                ..BrowserView::default()
            };
        };
        BrowserView {
            state: local.states.get(bot_id).cloned(),
            frame: None,
            error: local.errors.get(bot_id).cloned(),
            busy: false,
        }
    }

    /// Opens the bot's browser into the panel at `panel` (opening it first if needed), taking the panel from any other bot.
    pub fn mount(bot: &Bot, panel: Panel, cx: &mut App) {
        let Some(local) = local_mut(cx) else {
            return;
        };
        let identity = identity(bot);
        local.identities.insert(bot.id.clone(), identity.clone());
        let engine = local.engine.clone();
        cx.spawn(async move |cx| {
            guarded_async("open a bot's browser", async move {
                let result = engine.mount(&identity, Some(panel)).await;
                cx.update(|cx| accept(&identity.id, result, cx));
            })
            .await
        })
        .detach();
    }

    /// Puts the open page where the panel's page area is this frame. Win32 work, so after the draw.
    pub fn place(bot_id: &str, panel: Option<Panel>, window: &Window, cx: &mut App) {
        let Some(engine) = cx
            .try_global::<LocalBrowser>()
            .map(|local| local.engine.clone())
        else {
            return;
        };
        let bot_id = bot_id.to_owned();
        window.defer(cx, move |_, _| {
            guarded("place a bot's page", || {
                // Not open yet (its mount is on its way): nothing to place.
                let _ = engine.set_panel(&bot_id, panel);
            });
        });
    }

    /// The panel left the bot's page: a page dialog the person left open is dismissed, the page goes, and the browser
    /// goes back to the bot (`native-browser-session.ts` `#handBack`).
    pub fn hide(bot_id: &str, cx: &mut App) {
        let Some(local) = cx.try_global::<LocalBrowser>() else {
            return;
        };
        let engine = local.engine.clone();
        let dismiss = local.states.get(bot_id).and_then(|state| {
            let dialog = state
                .dialog
                .as_ref()
                .filter(|_| state.control == BrowserControl::Human)?;
            let identity = local.identities.get(bot_id)?.clone();
            Some((
                identity,
                NativeBrowserUiCommand::Dialog {
                    generation: state.generation.clone(),
                    page_id: dialog.page_id.clone(),
                    accept: false,
                    prompt_text: None,
                },
            ))
        });
        let bot_id = bot_id.to_owned();
        cx.spawn(async move |_| {
            guarded_async("hand a bot's browser back", async move {
                if let Some((identity, dialog)) = dismiss {
                    let _ = engine
                        .act(&identity, &dialog, Actor::Human, None, LOCAL)
                        .await;
                }
                engine.hide_bot(&bot_id).await;
            })
            .await
        })
        .detach();
    }

    /// Runs the person's command on a bot's browser, on the page and generation they saw.
    pub fn command(bot: &Bot, command: BrowserCommand, cx: &mut App) {
        let Some(local) = cx.try_global::<LocalBrowser>() else {
            return;
        };
        let state = local.states.get(&bot.id);
        let generation = state.map(|state| state.generation.clone());
        let page = state.and_then(|state| state.active_page_id.clone());
        let ui = match command {
            BrowserCommand::SetControl(control) => {
                Some(NativeBrowserUiCommand::Control { control })
            }
            BrowserCommand::Retry => Some(NativeBrowserUiCommand::Open),
            BrowserCommand::Navigate(url) => {
                generation
                    .zip(page)
                    .map(|(generation, page_id)| NativeBrowserUiCommand::Navigate {
                        generation,
                        page_id,
                        url,
                    })
            }
            BrowserCommand::History(action) => {
                generation
                    .zip(page)
                    .map(|(generation, page_id)| NativeBrowserUiCommand::History {
                        generation,
                        page_id,
                        action,
                    })
            }
            BrowserCommand::Tab { action, page_id } => {
                generation.map(|generation| NativeBrowserUiCommand::Tab {
                    generation,
                    action,
                    page_id,
                })
            }
            // The dialog open, on the page it belongs to (which may not be the page in front).
            BrowserCommand::Dialog {
                accept,
                prompt_text,
            } => state.and_then(|state| {
                let dialog = state.dialog.as_ref()?;
                Some(NativeBrowserUiCommand::Dialog {
                    generation: state.generation.clone(),
                    page_id: dialog.page_id.clone(),
                    accept,
                    prompt_text,
                })
            }),
            // This PC's page is WebView2's own: nothing is drawn by the app.
            BrowserCommand::Drawn(_) => None,
        };
        let Some(ui) = ui else {
            return;
        };
        let engine = local.engine.clone();
        let identity = identity(bot);
        cx.spawn(async move |cx| {
            guarded_async("run a browser command", async move {
                let result = engine.act(&identity, &ui, Actor::Human, None, LOCAL).await;
                cx.update(|cx| accept(&identity.id, result, cx));
            })
            .await
        })
        .detach();
    }

    /// A PNG of what the bot's page shows now, for the snapshot variant and the shield; the page must be visible.
    pub fn capture(
        bot_id: &str,
        cx: &App,
    ) -> Option<impl Future<Output = Result<Vec<u8>, EngineError>> + use<>> {
        let engine = cx.try_global::<LocalBrowser>()?.engine.clone();
        let bot_id = bot_id.to_owned();
        Some(async move { engine.capture_preview(&bot_id).await })
    }

    /// Raises the shield over the bot's page: a picture of it, then its window out of the way.
    pub fn raise_shield(bot_id: &str, cx: &App) -> Option<impl Future<Output = Vec<u8>> + use<>> {
        let engine = cx.try_global::<LocalBrowser>()?.engine.clone();
        let bot_id = bot_id.to_owned();
        Some(async move { engine.raise_shield(&bot_id).await.png })
    }

    /// Lowers the shield: the live page comes back.
    pub fn lower_shield(bot_id: &str, window: &Window, cx: &mut App) {
        let Some(engine) = cx
            .try_global::<LocalBrowser>()
            .map(|local| local.engine.clone())
        else {
            return;
        };
        let bot_id = bot_id.to_owned();
        window.defer(cx, move |_, _| {
            guarded("lower the browser shield", || engine.lower_shield(&bot_id));
        });
    }

    /// The keyboard belongs to the app: a click landed outside the page.
    pub fn focus_app(window: &Window, cx: &mut App) {
        let Some(engine) = cx
            .try_global::<LocalBrowser>()
            .map(|local| local.engine.clone())
        else {
            return;
        };
        window.defer(cx, move |_, _| {
            guarded("take the keyboard back", || engine.focus_app());
        });
    }

    /// The display scale or the app zoom changed: every page is placed again.
    pub fn rescaled(window: &Window, cx: &mut App) {
        let Some(engine) = cx
            .try_global::<LocalBrowser>()
            .map(|local| local.engine.clone())
        else {
            return;
        };
        window.defer(cx, move |_, _| {
            guarded("place the pages again", || engine.dpi_changed());
        });
    }
}

fn local_mut(cx: &mut App) -> Option<&mut LocalBrowser> {
    if cx.has_global::<LocalBrowser>() {
        Some(cx.global_mut::<LocalBrowser>())
    } else {
        None
    }
}

/// A command's answer: its state shown unless something newer was, or what went wrong.
fn accept(bot_id: &str, result: Result<BrowserState, EngineError>, cx: &mut App) {
    let Some(local) = local_mut(cx) else {
        return;
    };
    match result {
        Ok(state) => {
            local.errors.remove(bot_id);
            if newer(local.states.get(bot_id), &state) {
                local.states.insert(bot_id.to_owned(), state);
            }
        }
        Err(error) => {
            local.errors.insert(bot_id.to_owned(), error.to_string());
        }
    }
    cx.refresh_windows();
}

/// What the engine says: a state to draw, words for the person, the keyboard leaving a page, or an app chord.
fn hear(notification: Notification, handle: AnyWindowHandle, cx: &mut App) {
    match notification {
        Notification::State(state) => {
            if let Some(local) = local_mut(cx)
                && newer(local.states.get(&state.bot_id), &state)
            {
                local.states.insert(state.bot_id.clone(), *state);
                cx.refresh_windows();
            }
        }
        Notification::Notice { bot_id, message } => {
            if let Some(local) = local_mut(cx) {
                local.errors.insert(bot_id, message);
                cx.refresh_windows();
            }
        }
        Notification::FocusReturn { direction, .. } => {
            let Some(focus_return) = cx
                .try_global::<LocalBrowser>()
                .and_then(|local| local.focus_return.clone())
            else {
                return;
            };
            let _ = handle.update(cx, |_, window, cx| focus_return(direction, window, cx));
        }
        Notification::Shortcut { chord, .. } => {
            let keystroke = cx.try_global::<LocalBrowser>().and_then(|local| {
                local
                    .chords
                    .iter()
                    .find(|(known, _)| *known == chord)
                    .map(|(_, keystroke)| keystroke.clone())
            });
            if let Some(keystroke) = keystroke {
                let _ = handle.update(cx, |_, window, cx| window.dispatch_keystroke(keystroke, cx));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, px, size};

    #[test]
    fn every_forwarded_app_chord_has_an_engine_chord() {
        let forwarded = keymap::page_forwarded();
        let chords: Vec<Chord> = forwarded.iter().filter_map(chord).collect();
        assert_eq!(chords.len(), forwarded.len(), "{forwarded:?}");
        assert!(chords.contains(&Chord::parse("Ctrl+B").unwrap()));
        assert!(chords.contains(&Chord::parse("Escape").unwrap()));
        assert!(chords.contains(&Chord::parse("Ctrl+=").unwrap()));
    }

    #[test]
    fn the_page_rect_rounds_each_edge_and_the_clip_keeps_the_part_inside_the_panel() {
        let bounds = Bounds::new(point(px(100.25), px(50.)), size(px(400.), px(300.5)));
        assert_eq!(
            physical(bounds, 1.5),
            RECT {
                left: 150,
                top: 75,
                right: 750,
                bottom: 526
            }
        );
        // The page is laid out at its open width; the panel shows only its left 200 px so far.
        let page = RECT {
            left: 1000,
            top: 100,
            right: 1600,
            bottom: 900,
        };
        let panel = RECT {
            left: 990,
            top: 60,
            right: 1200,
            bottom: 920,
        };
        assert_eq!(
            clip_to(&page, &panel),
            RECT {
                left: 0,
                top: 0,
                right: 200,
                bottom: 800
            }
        );
    }

    #[test]
    fn a_reply_older_than_the_shown_state_is_dropped() {
        let state = |generation: &str, revision: Option<u64>| BrowserState {
            bot_id: "b1".into(),
            generation: generation.into(),
            control: BrowserControl::Bot,
            control_epoch: None,
            private_epoch: None,
            private_active: None,
            can_control: None,
            revision,
            viewport_revision: None,
            busy: None,
            active_page_id: None,
            tabs: Vec::new(),
            dialog: None,
        };
        let shown = state("g1", Some(5));
        assert!(!newer(Some(&shown), &state("g1", Some(4))));
        assert!(newer(Some(&shown), &state("g1", Some(5))));
        assert!(
            newer(Some(&shown), &state("g2", Some(1))),
            "a new generation"
        );
        assert!(newer(None, &shown));
    }
}
