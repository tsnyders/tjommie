//! Audio and video in the viewer (`MediaPreview` in `file-viewer.tsx`; owner decision 5): WebView2's own player, in a
//! media viewer created when the preview first shows and closed when it goes (`desktop_host::media`). It sits over
//! the preview's place in the window, reads the file through the transfer, and reaches nothing else. The player is
//! Win32 work: it's created, placed and closed after a frame, never inside an entity update. Tab reaches it like any
//! control and leaves it at its ends; Escape in it closes the viewer.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use desktop_host::browser::keyboard::FocusDirection;
use desktop_host::media::{MediaSource, MediaViewer, Spawn};
use futures::future::LocalBoxFuture;
use gpui::{
    App, AppContext as _, Bounds, Context, EntityId, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, Render, Role, SharedString, StatefulInteractiveElement as _,
    Styled as _, Window, canvas, div, px,
};
use gpui_kit::component::WindowExt as _;
use windows::Win32::Foundation::RECT;

use super::source::Opened;
use crate::guard::{guarded, guarded_async};
use crate::semantic::SemanticExt as _;
use crate::ui::browser::local::{LocalBrowser, physical, window_hwnd};
use crate::ui::primitives::focus_handle;
use crate::ui::primitives::shield::BrowserShield;
use crate::ui::style::{TypeExt as _, hsla, palette, r, ring_or_reveal, theme};

const FAILED: &str = "This media is corrupt or its codec is unsupported on this device. Download is still available.";
/// The virtual-key code of Escape.
const ESCAPE: u32 = 0x1B;

/// The transfer as the player reads it, on a reading thread of its own.
struct Transfer(Arc<Opened>);

impl MediaSource for Transfer {
    fn len(&self) -> u64 {
        self.0.info().size
    }

    fn content_type(&self) -> &str {
        &self.0.info().content_type
    }

    // This runs on the player's own reading thread, outside any guard, where a panic would leave the request hanging:
    // it must stay panic-free, as it is (no index, no unwrap; a failed read is an `Err`) (M4 review L6).
    fn read(&self, offset: u64, len: usize) -> Result<Vec<u8>, String> {
        let end = offset.saturating_add(len as u64);
        futures::executor::block_on(self.0.read_span(offset, end))
    }
}

/// What the player's callbacks hand on to the app.
enum Keyboard {
    Leave(FocusDirection),
    Escape,
}

/// What the view shares with the work after each frame; the player is never held across an entity update.
#[derive(Clone)]
struct Shared {
    player: Rc<RefCell<Option<MediaViewer>>>,
    /// Creating the player was tried (once per viewer).
    tried: Rc<Cell<bool>>,
    /// The view is gone: a player that opens late is closed at once.
    gone: Rc<Cell<bool>>,
    /// Where the player goes, as of the last frame.
    at: Rc<Cell<Option<RECT>>>,
    problem: Rc<RefCell<Option<SharedString>>>,
    news: async_channel::Sender<Keyboard>,
    inbox: async_channel::Receiver<Keyboard>,
    opened: Arc<Opened>,
    video: bool,
    /// Another overlay is open over the viewer: the player's window is hidden, as GPUI can't draw over it.
    covered: Rc<Cell<bool>>,
}

pub struct MediaView {
    id: SharedString,
    shared: Shared,
    /// The player has the keyboard since the app's focus came to its place.
    handed: bool,
    /// The overlays open when the player came (its own viewer's dialog among them): more than these cover it.
    under: usize,
}

impl MediaView {
    pub fn new(id: SharedString, opened: Arc<Opened>, video: bool, cx: &mut Context<Self>) -> Self {
        let (news, inbox) = async_channel::unbounded();
        let shared = Shared {
            player: Rc::default(),
            tried: Rc::default(),
            gone: Rc::default(),
            at: Rc::default(),
            problem: Rc::default(),
            news,
            inbox,
            opened,
            video,
            covered: Rc::default(),
        };
        let closing = shared.clone();
        cx.on_release(move |_, cx| {
            guarded("let the media player go", || {
                closing.gone.set(true);
                closing.news.close();
                // Closed after the work in hand, not inside the release.
                let player = closing.player.clone();
                cx.defer(move |_| {
                    guarded("close the media player", || {
                        if let Some(viewer) = player.borrow_mut().take() {
                            viewer.close();
                        }
                    });
                });
            });
        })
        .detach();
        MediaView {
            id,
            shared,
            handed: false,
            under: BrowserShield::raised(cx),
        }
    }
}

fn fail(shared: &Shared, view: EntityId, cx: &mut App) {
    *shared.problem.borrow_mut() = Some(FAILED.into());
    cx.notify(view);
}

/// The player's place: the video's, the preview's room, at most 65% of the window's height in pixels (the TypeScript's
/// 65vh; as rems, Larger text or the zoom made it taller than the viewer, release review m3); the audio player's, one
/// line.
fn place_for(video: bool, window: &Window) -> gpui::Div {
    let place = div().w_full();
    if video {
        place.h_full().max_h(px(
            (f32::from(window.viewport_size().height) * 0.65).max(120.)
        ))
    } else {
        place.h(r(54.))
    }
}

/// After the frame: the player is created where the preview is (the first time), or moved there.
fn place(
    shared: &Shared,
    view: EntityId,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let rect = physical(bounds, window.scale_factor());
    shared.at.set(Some(rect));
    let shared = shared.clone();
    window.defer(cx, move |window, cx| {
        guarded("place the media player", || {
            if let Some(viewer) = shared.player.borrow().as_ref() {
                viewer.set_bounds(rect);
                return;
            }
            if !shared.tried.replace(true) {
                open(shared.clone(), view, rect, window, cx);
            }
        });
    });
}

/// Opens the player in the bots' browser process (its own profile, `media`), then hands its keyboard news on.
fn open(shared: Shared, view: EntityId, rect: RECT, window: &mut Window, cx: &mut App) {
    let root = cx
        .try_global::<crate::live::DataRoot>()
        .map(|root| root.0.join("data").join("webview2"));
    let (Some(parent), Some(root)) = (window_hwnd(window), root) else {
        log::warn!("The media player has no window or data folder");
        fail(&shared, view, cx);
        return;
    };
    let executor = cx.foreground_executor().clone();
    let spawn: Spawn = Rc::new(move |task: LocalBoxFuture<'static, ()>| {
        let task = guarded_async("answer the media player", task);
        // unguarded: a future, not a closure; guarded_async wraps it on the line above
        executor.spawn(task).detach();
    });
    let source: Arc<dyn MediaSource> = Arc::new(Transfer(shared.opened.clone()));
    let handle = window.window_handle();
    cx.spawn(async move |cx| {
        guarded_async("open the media player", async move {
            let made = MediaViewer::open(parent, rect, source, shared.video, &root, spawn).await;
            let viewer = match made {
                Ok(viewer) => viewer,
                Err(error) => {
                    log::warn!("The media player didn't open: {error}");
                    cx.update(|cx| fail(&shared, view, cx));
                    return;
                }
            };
            if shared.gone.get() {
                viewer.close();
                return;
            }
            if let Some(rect) = shared.at.get() {
                viewer.set_bounds(rect);
            }
            // Inside WebView2's callbacks: the news only, each guarded.
            let (leaving, keys) = (shared.news.clone(), shared.news.clone());
            let leave = move |direction: FocusDirection| {
                guarded("hear the media player's Tab", || {
                    let _ = leaving.try_send(Keyboard::Leave(direction));
                });
            };
            let key = move |key: u32, ctrl: bool, shift: bool, alt: bool| {
                let escape = key == ESCAPE && !ctrl && !shift && !alt;
                if escape {
                    guarded("hear the media player's Escape", || {
                        let _ = keys.try_send(Keyboard::Escape);
                    });
                }
                escape
            };
            // unguarded: two callbacks, each guarded where it's made above
            let keyboard = viewer.on_keyboard(leave, key);
            if let Err(error) = keyboard {
                log::warn!("The media player keeps the keyboard: {error}");
            }
            // Opened under another overlay: hidden until it goes.
            if shared.covered.get() {
                viewer.set_visible(false);
            }
            *shared.player.borrow_mut() = Some(viewer);
            cx.update(|cx| cx.notify(view));
            while let Ok(news) = shared.inbox.recv().await {
                let _ = cx.update_window(handle, |_, window, cx| {
                    LocalBrowser::focus_app(window, cx);
                    match news {
                        Keyboard::Leave(FocusDirection::Next) => window.focus_next(cx),
                        Keyboard::Leave(FocusDirection::Previous) => window.focus_prev(cx),
                        Keyboard::Escape => window.close_dialog(cx),
                    }
                });
            }
        })
        .await;
    })
    .detach();
}

impl Render for MediaView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The shield, as the browser panel's (M4 review P3): GPUI can't draw over the player's window, so while another
        // overlay is open over the viewer (a confirm, a menu, the connection sheet) the player hides; its sound goes on.
        let covered = BrowserShield::raised(cx) > self.under;
        if covered != self.shared.covered.replace(covered) {
            let player = self.shared.player.clone();
            window.defer(cx, move |_, _| {
                guarded("shield the media player", || {
                    if let Some(viewer) = player.borrow().as_ref() {
                        viewer.set_visible(!covered);
                    }
                });
            });
        }
        let id = self.id.clone();
        let focus = focus_handle(&format!("{id}.player"), window, cx);
        // The app's focus came to the player's place: the player takes the keyboard, once it's open.
        let focused = focus.is_focused(window);
        if focused && !self.handed && self.shared.player.borrow().is_some() {
            self.handed = true;
            let player = self.shared.player.clone();
            window.defer(cx, move |_, _| {
                guarded("give the media player the keyboard", || {
                    if let Some(viewer) = player.borrow().as_ref() {
                        viewer.focus();
                    }
                });
            });
        }
        self.handed &= focused;
        let theme = theme(cx);
        let colors = palette(cx);
        let problem = self.shared.problem.borrow().clone();
        if let Some(problem) = problem {
            return div()
                .semantic(format!("{id}.problem"), Role::Alert)
                .aria_label(problem.clone())
                .type_style(&theme.typography.desktop.small)
                .text_color(hsla(colors.danger.text))
                .child(problem);
        }
        let video = self.shared.video;
        let (shared, view) = (self.shared.clone(), cx.entity_id());
        place_for(video, window)
            .semantic(
                format!("{id}.player"),
                if video {
                    Role::Application
                } else {
                    Role::Group
                },
            )
            .aria_label(if video {
                "Video player"
            } else {
                "Audio player"
            })
            .track_focus(&focus)
            .relative()
            .rounded(r(theme.radius.sm))
            .bg(hsla(colors.surface.sunk))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, window, cx| place(&shared, view, bounds, window, cx),
                )
                .size_full(),
            )
            .children(ring_or_reveal(&focus, theme.radius.sm, window, cx))
    }
}

#[cfg(test)]
mod tests {
    use gpui::{
        InteractiveElement as _, IntoElement as _, ParentElement as _, Styled as _, div, px, size,
    };

    /// The video's place fills the room the viewer gives it and no more, with Larger text and the zoom at 125 % too
    /// (release review m3: as rems, 65% of a 600 px window made it 609 px tall, past the dialog). A test window has no
    /// HWND for the player itself, so this lays out its place alone, in a 300 px room.
    #[gpui::test]
    fn the_video_fits_its_room_at_larger_text(cx: &mut gpui::TestAppContext) {
        let cx = crate::ui::primitives::test_window(cx, |window, _| {
            div()
                .h(px(300.))
                .child(super::place_for(true, window).debug_selector(|| "place".into()))
                .into_any_element()
        });
        crate::ui::testing::larger_text_and_zoom(cx, true);
        cx.simulate_resize(size(px(1000.), px(600.)));
        crate::ui::testing::draw(cx);
        let place = cx.debug_bounds("place").expect("the place");
        assert!(
            place.size.height > px(0.) && place.size.height <= px(300.),
            "the video fits its room: {place:?}"
        );
        crate::ui::testing::larger_text_and_zoom(cx, false);
    }
}
