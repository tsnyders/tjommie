//! The right panel beside a bot's chat (Phase 3 plan T3.7; `right-panel.tsx`, Aegis `browser-panel.html`): the Details |
//! Browser switch with the bot's settings, Expand and Hide, then the pane. Its width springs open and shut (`motion`) while
//! the chat column takes the rest; its content keeps its width and is revealed, not squashed. In the live variant the
//! page's window moves with the panel every frame, clipped to it (spike `place_clipped`); in the snapshot variant a still
//! picture of the page, taken at rest, slides instead and the live page comes back at rest.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use desktop_host::browser::engine::Panel;
use desktop_host::browser::keyboard::FocusDirection;
use desktop_host::perf::{qpc, qpc_frequency};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, AppContext as _, Bounds, Context, DispatchPhase, Entity, FocusHandle, Global,
    Image, ImageFormat, IntoElement, MouseDownEvent, ParentElement as _, Pixels, Render, Role,
    SharedString, StatefulInteractiveElement as _, Styled as _, Task, WeakEntity, Window, canvas,
    div, img, svg,
};
use gpui_kit::base::input::{InputEvent, InputState};
use protocol::model::{Bot, BotState};

use super::chrome::{self, Chrome};
use super::local::{self, LocalBrowser};
use super::motion::PanelMotion;
use super::panel_variant::{CpuMeter, PanelVariant};
use super::stream::RemotePicture;
use super::{BrowserCommand, BrowserData, BrowserView};
use crate::guard::{guarded, guarded_async};
use crate::model::labels::{autonomy_line, model_label, state_label};
use crate::model::panel_layout::{
    Pane, PanelView, choose_pane, clamp_width, pane_spec, panel_for, set_expanded, width_bounds,
};
use crate::motion::clock::{self, Live, LiveGuard};
use crate::motion::keyboard::Trigger;
use crate::semantic::SemanticExt as _;
use crate::ui::primitives::avatar::BotAvatar;
use crate::ui::primitives::button::IconButton;
use crate::ui::primitives::shield::BrowserShield;
use crate::ui::shell::data::ShellData;
use crate::ui::shell::route::{self, Navigate as _, Route};
use crate::ui::style::{TypeExt as _, hsla, palette, r, shadows, theme};

/// The panel, for the app-level keys (Ctrl+B and Escape) to reach.
struct PanelHandle(WeakEntity<BrowserPanel>);

impl Global for PanelHandle {}

/// The shield over the page while an overlay is open.
enum ShieldState {
    Down,
    /// The picture is being taken; the live page stays up until it is ready (spike: 64 to 102 ms).
    Raising,
    /// The page's window is out of the way; its picture stands in (none if the browser couldn't take one).
    Up(Option<Arc<Image>>),
}

pub struct BrowserPanel {
    motion: PanelMotion,
    cpu: CpuMeter,
    /// The bot the panel is for, the pane it shows, and whether the browser fills the content area.
    view: PanelView,
    /// The panel's bounds as last painted, for the page window's clip while it moves.
    panel_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The bot whose page is in the panel.
    mounted: Option<String>,
    /// The bot whose remote browser the panel shows (on another PC).
    remote_shown: Option<String>,
    /// The snapshot variant's picture of the page, taken at rest.
    snapshot: Option<Arc<Image>>,
    snapshot_task: Option<Task<Option<()>>>,
    shield: ShieldState,
    /// Holds `Live` while the live variant moves, so an inactive window keeps the display's rate.
    live: Option<LiveGuard>,
    /// Holds `Live` while another PC's live stream shows in the panel (the throttle rule, gate condition 5).
    stream_live: Option<LiveGuard>,
    address: Entity<InputState>,
    /// The person is typing an address: the bar shows what they typed, not the page's.
    address_draft: Rc<Cell<bool>>,
    prompt: Entity<InputState>,
    /// The page dialog the prompt's answer was set up for.
    prompt_for: Option<(String, String)>,
    /// Another PC's page as a picture.
    picture: RemotePicture,
    /// The control pill's focus, where the keyboard lands when it leaves the page.
    control_focus: Rc<RefCell<Option<FocusHandle>>>,
    scale: f32,
}

impl BrowserPanel {
    /// The panel for `window`, closed, its keys and the page's focus return wired to it.
    pub fn create(window: &mut Window, cx: &mut App) -> Entity<BrowserPanel> {
        let calm = theme(cx).motion.spring.calm.clone();
        let panel = cx.new(|cx| {
            let address =
                cx.new(|cx| InputState::new(window, cx).placeholder("Search or enter an address"));
            let prompt = cx.new(|cx| InputState::new(window, cx));
            let address_draft = Rc::new(Cell::new(false));
            BrowserPanel {
                motion: PanelMotion::new(pane_spec(Pane::Details).fallback as f32, &calm),
                cpu: CpuMeter::start(),
                view: PanelView {
                    bot: String::new(),
                    pane: Pane::Details,
                    expanded: false,
                },
                panel_bounds: Rc::default(),
                mounted: None,
                remote_shown: None,
                snapshot: None,
                snapshot_task: None,
                shield: ShieldState::Down,
                live: None,
                stream_live: None,
                address,
                address_draft,
                prompt,
                prompt_for: None,
                picture: RemotePicture::default(),
                control_focus: Rc::default(),
                scale: window.scale_factor(),
            }
        });
        panel.update(cx, |this, cx| {
            let (address, draft) = (this.address.clone(), this.address_draft.clone());
            cx.subscribe_in(&address, window, move |this, input, event, window, cx| {
                guarded("read the address bar", || {
                    this.address_event(input, event, &draft, window, cx)
                });
            })
            .detach();
        });
        cx.set_global(PanelHandle(panel.downgrade()));
        let focus = panel.read(cx).control_focus.clone();
        if cx.has_global::<LocalBrowser>() {
            cx.global_mut::<LocalBrowser>().focus_return = Some(Rc::new(
                move |direction: FocusDirection, window: &mut Window, cx: &mut App| {
                    // Shift+Tab out of the page lands on the control pill, where the keyboard would have gone; Tab past
                    // the page's end goes on from it.
                    if let Some(focus) = focus.borrow().as_ref() {
                        focus.focus(window, cx);
                        if direction == FocusDirection::Next {
                            window.focus_next(cx);
                        }
                    }
                },
            ));
        }
        panel
    }

    fn handle(cx: &App) -> Option<Entity<BrowserPanel>> {
        cx.try_global::<PanelHandle>()?.0.upgrade()
    }

    /// Opens the panel if it is closed, closes it if it is open (Ctrl+B, or a button).
    pub fn toggle(trigger: Trigger, cx: &mut App) {
        if let Some(panel) = Self::handle(cx) {
            panel.update(cx, |panel, cx| {
                let open = !panel.motion.is_open();
                panel.set_open(open, trigger, cx);
            });
        }
    }

    /// Escape with no overlay open: closes the panel, at once.
    pub fn dismiss(cx: &mut App) {
        if let Some(panel) = Self::handle(cx) {
            panel.update(cx, |panel, cx| {
                if panel.motion.is_open() {
                    panel.set_open(false, Trigger::Keyboard, cx);
                }
            });
        }
    }

    /// Whether the panel is open, or opening.
    pub fn is_open(&self) -> bool {
        self.motion.is_open()
    }

    /// The panel's visible width in design px.
    pub fn width(&self) -> f32 {
        self.motion.width.value
    }

    /// Opens or closes: at once after a key or under reduced motion, otherwise on the spring in the variant chosen now.
    /// The panel opens only beside a bot's chat.
    pub fn set_open(&mut self, open: bool, trigger: Trigger, cx: &mut Context<Self>) {
        if open && chat_bot(cx).is_none() {
            return;
        }
        let snaps = trigger.snaps(cx);
        let variant = if snaps {
            PanelVariant::Live
        } else {
            self.motion.choose(Instant::now(), self.cpu.last_second())
        };
        self.motion.set_open(open, snaps, variant, qpc());
        if self.motion.is_moving() && self.motion.variant() == PanelVariant::Live {
            self.live.get_or_insert_with(|| Live::hold(cx));
        }
        cx.notify();
    }

    fn address_event(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        draft: &Rc<Cell<bool>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => draft.set(true),
            InputEvent::PressEnter { .. } => {
                let typed = input.read(cx).value().to_string();
                draft.set(false);
                if !typed.trim().is_empty() {
                    let url = crate::model::browser_geometry::address_to_url(&typed);
                    self.command(BrowserCommand::Navigate(url), window, cx);
                }
            }
            _ => {}
        }
    }

    /// Sends the person's command to the bot's browser: this PC's engine, or the store's session for another PC.
    fn command(&mut self, command: BrowserCommand, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bot) = chat_bot(cx) else {
            return;
        };
        if is_remote(cx) {
            let on_remote = cx
                .try_global::<BrowserData>()
                .and_then(|data| data.on_remote.clone());
            match on_remote {
                Some(on_remote) => on_remote(&bot.id, command, window, cx),
                None => log::info!("No store yet for {command:?}"),
            }
        } else {
            LocalBrowser::command(&bot, command, cx);
        }
    }

    /// Follows the chat: another bot's panel starts at Details, and the page the panel had goes.
    fn follow(&mut self, bot: Option<&Bot>, window: &mut Window, cx: &mut Context<Self>) {
        let id = bot.map_or("", |bot| bot.id.as_str());
        if self.view.bot != id {
            self.view = panel_for(&self.view, id);
            self.address_draft.set(false);
            self.drop_image(ImageSlot::Snapshot, cx);
        }
        // No bot here (another screen, or the bot is gone): the panel closes at once.
        if bot.is_none() && (self.motion.is_open() || self.motion.is_moving()) {
            self.motion.set_open(false, true, PanelVariant::Live, qpc());
        }
        let wants_page = bot.is_some_and(|bot| {
            bot.browser
                && self.view.pane == Pane::Browser
                && (self.motion.is_open() || self.motion.is_moving())
                && !is_remote(cx)
        });
        // Another PC's bot's browser: its session opens while the Browser pane shows it, and closes when it doesn't.
        let on_remote = cx
            .try_global::<BrowserData>()
            .and_then(|data| data.on_remote.clone());
        let wants_remote = bot.is_some_and(|bot| {
            bot.browser
                && self.view.pane == Pane::Browser
                && (self.motion.is_open() || self.motion.is_moving())
                && is_remote(cx)
        });
        let remote_want = wants_remote.then(|| id.to_owned());
        if let Some(on_remote) = on_remote
            && self.remote_shown != remote_want
        {
            if let Some(gone) = self.remote_shown.take() {
                on_remote(&gone, BrowserCommand::Hide, window, cx);
            }
            if let Some(shown) = &remote_want {
                on_remote(shown, BrowserCommand::Show, window, cx);
            }
            self.remote_shown = remote_want;
        }
        let want = wants_page.then(|| id.to_owned());
        if self.mounted != want {
            if let Some(gone) = self.mounted.take() {
                // The page leaves with its shield: hiding it is all the engine needs.
                self.drop_image(ImageSlot::Shield, cx);
                self.shield = ShieldState::Down;
                LocalBrowser::hide(&gone, cx);
            }
            if let (Some(bot), Some(_)) = (bot, &want) {
                LocalBrowser::mount(bot, self.panel_rect(window).unwrap_or(HIDDEN_PANEL), cx);
            }
            self.mounted = want;
        }
    }

    /// The page area's last rectangle, for a mount before this frame has placed it.
    fn panel_rect(&self, window: &Window) -> Option<Panel> {
        let bounds = self.panel_bounds.get()?;
        Some(Panel {
            rect: local::physical(bounds, window.scale_factor()),
            occluded: true,
            clip: None,
        })
    }

    /// The motion came to rest: a live one leaves its p95 for the next choice, and a fresh picture is taken for the
    /// snapshot variant's next move.
    fn rested(&mut self, cx: &mut Context<Self>) {
        let frames = clock::frames(cx);
        let starts: Vec<i64> = frames.iter().map(|frame| frame.begin_qpc).collect();
        self.motion.finish(&starts, qpc_frequency(), Instant::now());
        self.live = None;
        if self.motion.is_open() {
            self.refresh_snapshot(cx);
        }
    }

    fn refresh_snapshot(&mut self, cx: &mut Context<Self>) {
        if self.private_page(cx) {
            return;
        }
        let Some(capture) = self
            .mounted
            .as_deref()
            .and_then(|bot| LocalBrowser::capture(bot, cx))
        else {
            return;
        };
        self.snapshot_task = Some(cx.spawn(async move |this, cx| {
            guarded_async("take the panel's picture", async move {
                let png = capture.await;
                let _ = this.update(cx, |this, cx| {
                    this.snapshot_task = None;
                    if let Ok(png) = png {
                        this.drop_image(ImageSlot::Snapshot, cx);
                        this.snapshot = Some(Arc::new(Image::from_bytes(ImageFormat::Png, png)));
                    }
                });
            })
            .await
        }));
    }

    /// The page in the panel is in a private interaction: the panel takes no picture of it and keeps none (review F6).
    fn private_page(&self, cx: &App) -> bool {
        self.mounted
            .as_deref()
            .is_some_and(|bot| chrome::private(LocalBrowser::view(bot, cx).state.as_ref()))
    }

    /// An overlay opened over the page (a menu, a dialog, a sheet): a picture of the page stands in while its window is out
    /// of the way. Closing the overlay brings the live page back.
    fn follow_shield(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let wanted = BrowserShield::is_raised(cx) && self.mounted.is_some();
        match (&self.shield, wanted) {
            // A private page's window steps aside with no picture taken: a plain cover stands in.
            (ShieldState::Down, true) if self.private_page(cx) => {
                self.shield = ShieldState::Up(None);
                cx.notify();
            }
            (ShieldState::Down, true) => {
                let Some(raise) = self
                    .mounted
                    .as_deref()
                    .and_then(|bot| LocalBrowser::raise_shield(bot, cx))
                else {
                    return;
                };
                self.shield = ShieldState::Raising;
                cx.spawn(async move |this, cx| {
                    guarded_async("raise the browser shield", async move {
                        let png = raise.await;
                        let _ = this.update(cx, |this, cx| {
                            // Taken as the page turned private: not kept.
                            let picture = (!png.is_empty() && !this.private_page(cx))
                                .then(|| Arc::new(Image::from_bytes(ImageFormat::Png, png)));
                            this.shield = ShieldState::Up(picture);
                            cx.notify();
                        });
                    })
                    .await
                })
                .detach();
            }
            (ShieldState::Raising | ShieldState::Up(_), false) => {
                if let Some(bot) = self.mounted.clone() {
                    self.lower_shield(&bot, window, cx);
                }
            }
            _ => {}
        }
    }

    fn lower_shield(&mut self, bot: &str, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.shield, ShieldState::Down) {
            return;
        }
        self.drop_image(ImageSlot::Shield, cx);
        self.shield = ShieldState::Down;
        LocalBrowser::lower_shield(bot, window, cx);
    }

    /// Lets GPUI forget a picture it no longer shows.
    fn drop_image(&mut self, slot: ImageSlot, cx: &mut Context<Self>) {
        let image = match slot {
            ImageSlot::Snapshot => self.snapshot.take(),
            ImageSlot::Shield => match std::mem::replace(&mut self.shield, ShieldState::Down) {
                ShieldState::Up(image) => image,
                other => {
                    self.shield = other;
                    None
                }
            },
        };
        if let Some(image) = image {
            image.remove_asset(cx);
        }
    }

    /// The room the panel has: the content area's width in design px, beside the sidebar.
    fn room(window: &Window, cx: &App) -> f64 {
        let theme = theme(cx);
        let zoom = f32::from(window.rem_size()) / 16.;
        let viewport = f32::from(window.viewport_size().width) / zoom.max(0.1);
        f64::from(viewport - theme.size.sidebar - theme.space.s12)
    }
}

enum ImageSlot {
    Snapshot,
    Shield,
}

/// Where a page goes before its panel has been painted: nowhere visible, until the first frame places it.
const HIDDEN_PANEL: Panel = Panel {
    rect: windows::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: 1,
        bottom: 1,
    },
    occluded: true,
    clip: None,
};

/// The bot whose chat shows, if the store has it.
fn chat_bot(cx: &App) -> Option<Bot> {
    let Route::Chat(id) = route::current(cx) else {
        return None;
    };
    cx.try_global::<BrowserData>()?
        .bots
        .iter()
        .find(|bot| bot.id == id)
        .cloned()
}

/// The window is on another PC: the panel shows that PC's browser as a picture.
fn is_remote(cx: &App) -> bool {
    cx.try_global::<ShellData>()
        .is_some_and(|data| data.host.is_some())
}

impl Render for BrowserPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let bot = chat_bot(cx);
        if self.motion.is_moving() {
            if self.motion.step(clock::dt(cx)) {
                clock::keep_moving(window, cx);
            } else {
                self.rested(cx);
            }
        }
        // A motion a key or a closing chat cut short is over too.
        if !self.motion.is_moving() {
            self.live = None;
        }
        // The pane's own width, within the room the window has (the browser expanded takes all of it).
        let pane = if bot.as_ref().is_some_and(|bot| bot.browser) {
            self.view.pane
        } else {
            Pane::Details
        };
        let room = Self::room(window, cx);
        let width = if self.view.expanded && pane == Pane::Browser {
            room - f64::from(theme(cx).space.s12)
        } else {
            clamp_width(pane_spec(pane).fallback, width_bounds(pane, room))
        };
        if (self.motion.open_width() - width as f32).abs() > 0.5 {
            self.motion.set_open_width(width as f32);
        }
        self.follow(bot.as_ref(), window, cx);
        // A page turned private: no picture of it is kept, nor one being taken (review F6).
        if self.private_page(cx) {
            self.snapshot_task = None;
            self.drop_image(ImageSlot::Snapshot, cx);
        }
        self.follow_shield(window, cx);
        let streams = bot.as_ref().is_some_and(|bot| {
            self.motion.width.value > 0.
                && pane == Pane::Browser
                && is_remote(cx)
                && cx
                    .try_global::<BrowserData>()
                    .and_then(|data| data.remote.get(&bot.id))
                    .is_some_and(|view| {
                        view.frame.is_some() && !chrome::private(view.state.as_ref())
                    })
        });
        if streams {
            self.stream_live.get_or_insert_with(|| Live::hold(cx));
        } else {
            self.stream_live = None;
        }
        if window.scale_factor() != self.scale {
            self.scale = window.scale_factor();
            LocalBrowser::rescaled(window, cx);
        }
        let Some(bot) = bot.filter(|_| self.motion.width.value > 0.) else {
            // Closed: no room taken in the row, and nothing to draw.
            return div().absolute().size_0().into_any_element();
        };
        self.draw(&bot, pane, window, cx)
    }
}

impl BrowserPanel {
    fn draw(
        &mut self,
        bot: &Bot,
        pane: Pane,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let content = match pane {
            Pane::Details => self.details(bot, cx).into_any_element(),
            Pane::Browser => self.browser(bot, window, cx),
        };
        let head = self.head(bot, pane, cx);
        let theme = theme(cx);
        let colors = palette(cx);
        let open_width = self.motion.open_width();
        let width = self.motion.width.value;
        let progress = if open_width > 0. {
            (width / open_width).clamp(0., 1.)
        } else {
            0.
        };
        let gap = theme.space.s12;
        let panel_bounds = self.panel_bounds.clone();
        div()
            .semantic("browser.panel", Role::Complementary)
            .aria_label(format!("About {}", bot.name))
            .relative()
            .flex_none()
            .h_full()
            .w(r(width))
            // The row's gap grows with the panel, so the chat column doesn't jump at the start of an opening.
            .ml(r(-gap * (1. - progress)))
            .overflow_hidden()
            .bg(hsla(colors.surface.panel))
            .rounded(r(theme.radius.panel))
            .border_t_1()
            .border_color(hsla(colors.overlay.highlight))
            .shadow(shadows(&theme.shadow.light.glass))
            .child(
                canvas(
                    |_, _, _| {},
                    move |bounds, _, _, _| panel_bounds.set(Some(bounds)),
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .w(r(open_width))
                    .h_full()
                    .flex()
                    .flex_col()
                    .opacity(self.motion.content_opacity())
                    .child(head)
                    .child(content),
            )
            .into_any_element()
    }

    /// The head: the Details | Browser switch, then Expand (in the browser), the bot's settings and Hide.
    fn head(&self, bot: &Bot, pane: Pane, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = theme(cx);
        let entity = cx.entity();
        let switch = bot.browser.then(|| {
            let choose = |key: Pane| {
                let entity = entity.clone();
                move |_: &mut Window, cx: &mut App| {
                    let bot = entity.read(cx).view.bot.clone();
                    entity.update(cx, |this, cx| {
                        this.view = choose_pane(&this.view, &bot, key);
                        cx.notify();
                    });
                }
            };
            div()
                .semantic("browser.panes", Role::TabList)
                .aria_label("Agent panel views")
                .flex()
                .p(r(3.))
                .gap(r(theme.space.s2))
                .rounded_full()
                .bg(hsla(palette(cx).surface.sunk))
                .child(segment(
                    "browser.pane.details",
                    "Details",
                    pane == Pane::Details,
                    choose(Pane::Details),
                    cx,
                ))
                .child(segment(
                    "browser.pane.browser",
                    "Browser",
                    pane == Pane::Browser,
                    choose(Pane::Browser),
                    cx,
                ))
        });
        let expanded = self.view.expanded;
        let expand = (pane == Pane::Browser).then(|| {
            let entity = entity.clone();
            let label = if expanded {
                "Restore browser"
            } else {
                "Expand browser"
            };
            IconButton::new("browser.expand", "maximize-2", label).on_press(move |_, cx| {
                entity.update(cx, |this, cx| {
                    let bot = this.view.bot.clone();
                    this.view = set_expanded(&this.view, &bot, !expanded);
                    cx.notify();
                })
            })
        });
        let settings = {
            let id = bot.id.clone();
            IconButton::new("browser.settings", "settings", "Agent settings")
                .on_press(move |_, cx| cx.navigate(Route::BotSettings(id.clone())))
        };
        let hide = IconButton::new("browser.hide", "panel-right", "Hide panel").on_press(
            move |window, cx| {
                let trigger = Trigger::of(window);
                entity.update(cx, |this, cx| this.set_open(false, trigger, cx));
            },
        );
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(r(theme.space.s8))
            .h(r(60.))
            .pl(r(theme.space.s16))
            .pr(r(theme.space.s12))
            .children(switch)
            .child(div().flex_1())
            .children(expand)
            .child(settings)
            .child(hide)
    }

    /// The Details pane: the bot, and its browser's card. The rest of `bot-panel.tsx` (usage, limits, routines, recent
    /// activity) reads the store, which T1.4 wires.
    fn details(&self, bot: &Bot, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = theme(cx);
        let colors = palette(cx);
        let entity = cx.entity();
        let open = move |_: &mut Window, cx: &mut App| {
            entity.update(cx, |this, cx| {
                let bot = this.view.bot.clone();
                this.view = choose_pane(&this.view, &bot, Pane::Browser);
                cx.notify();
            })
        };
        // bot-panel.tsx `Summary`: what the bot alone says. Its usage and limits read the store, which T1.4 wires.
        let model = format!(
            "{}{}",
            model_label(bot.provider, &bot.model),
            if bot.using_fallback == Some(true) {
                " (fallback)"
            } else {
                ""
            }
        );
        let state = (bot.state != BotState::Idle).then(|| {
            state_label(
                bot.state,
                bot.limited_until.as_deref(),
                &jiff::tz::TimeZone::system(),
            )
        });
        let row = |id: &'static str, text: String| {
            div()
                .semantic(id, Role::Paragraph)
                .aria_label(text.clone())
                .min_w_0()
                .truncate()
                .type_style(&theme.typography.desktop.small)
                .text_color(hsla(colors.text.secondary))
                .child(text)
        };
        let card = |child: gpui::Div| {
            child
                .flex()
                .items_center()
                .gap(r(theme.space.s12))
                .p(r(theme.space.s16))
                .rounded(r(theme.radius.md))
                .bg(hsla(colors.surface.raised))
                .shadow(shadows(&theme.shadow.light.card))
        };
        div()
            .flex()
            .flex_col()
            .gap(r(theme.space.s16))
            .px(r(theme.space.s16))
            .pb(r(theme.space.s16))
            .child(
                card(div())
                    .items_start()
                    .child(BotAvatar::new(bot.color, bot.shape, theme.size.avatar40))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(r(theme.space.s6))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(r(theme.space.s12))
                                    .child(
                                        div()
                                            .semantic("browser.details.name", Role::Heading)
                                            .aria_label(bot.name.clone())
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .type_style(&theme.typography.desktop.display_s)
                                            .text_color(hsla(colors.text.primary))
                                            .child(bot.name.clone()),
                                    )
                                    .children(state.map(|state| {
                                        div()
                                            .semantic("browser.details.state", Role::Status)
                                            .aria_label(state.clone())
                                            .flex_none()
                                            .h(r(22.))
                                            .px(r(theme.space.s8))
                                            .flex()
                                            .items_center()
                                            .rounded_full()
                                            .bg(hsla(colors.surface.sunk))
                                            .type_style(&theme.typography.desktop.meta)
                                            .text_color(hsla(colors.text.primary))
                                            .child(state)
                                    })),
                            )
                            .child(row("browser.details.model", model))
                            .child(row("browser.details.autonomy", autonomy_line(bot.autonomy).to_owned()))
                            .child(row("browser.details.home", "Home folder".to_owned()))
                            .children(bot.project_folder.as_ref().map(|folder| {
                                row("browser.details.project", format!("Project folder: {folder}"))
                            })),
                    ),
            )
            .when(bot.browser, |pane| {
                pane.child(
                    card(div())
                        .child(svg().path("icons/i/175/monitor.svg").size(r(20.)).text_color(hsla(colors.icon.base)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(r(theme.space.s4))
                                .child(
                                    div()
                                        .type_style(&theme.typography.desktop.ui_strong)
                                        .text_color(hsla(colors.text.primary))
                                        .child("Computer"),
                                )
                                .child(
                                    div()
                                        .type_style(&theme.typography.desktop.small)
                                        .text_color(hsla(colors.text.secondary))
                                        .child("Built into Tjommie, with its own sign-ins. Watch the agent use it, or take over."),
                                ),
                        )
                        .child(
                            crate::ui::primitives::button::Pill::new("browser.open", "Open")
                                .small()
                                .on_press(open),
                        ),
                )
            })
    }

    /// The Browser pane: the chrome over the page's place.
    fn browser(&mut self, bot: &Bot, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let remote = is_remote(cx);
        let view = if remote {
            cx.try_global::<BrowserData>()
                .and_then(|data| data.remote.get(&bot.id).cloned())
                .unwrap_or_else(|| BrowserView {
                    error: Some(
                        "The browser host is offline. Reconnect to watch or take control.".into(),
                    ),
                    ..BrowserView::default()
                })
        } else {
            LocalBrowser::view(&bot.id, cx)
        };
        // The address bar shows the page's address unless the person is typing one.
        let url = view
            .state
            .as_ref()
            .and_then(|state| {
                state
                    .tabs
                    .iter()
                    .find(|tab| Some(&tab.id) == state.active_page_id.as_ref())
            })
            .map(|tab| tab.url.clone())
            .unwrap_or_default();
        if !self.address_draft.get() && self.address.read(cx).value().as_ref() != url {
            self.address
                .update(cx, |input, cx| input.set_value(url.clone(), window, cx));
        }
        let human = chrome::human(view.state.as_ref());
        self.address
            .update(cx, |input, cx| input.set_readonly(!human, cx));
        // A new page dialog starts its answer at the page's default.
        let dialog = view.state.as_ref().and_then(|state| state.dialog.as_ref());
        let dialog_key = dialog.map(|dialog| (dialog.page_id.clone(), dialog.message.clone()));
        if dialog_key != self.prompt_for {
            let default = dialog.map_or(String::new(), |dialog| dialog.default_value.clone());
            self.prompt
                .update(cx, |input, cx| input.set_value(default, window, cx));
            self.prompt_for = dialog_key;
        }
        let entity = cx.entity();
        let on_command: chrome::OnCommand = Rc::new(move |command, window, cx| {
            entity.update(cx, |this, cx| this.command(command, window, cx));
        });
        let viewport = self.viewport(bot, &view, remote, window, cx);
        Chrome {
            view: &view,
            bot_id: bot.id.clone(),
            bot_name: SharedString::from(bot.name.clone()),
            bot_color: bot.color,
            bot_shape: bot.shape,
            address: self.address.clone(),
            address_draft: self.address_draft.clone(),
            prompt: self.prompt.clone(),
            control_focus: self.control_focus.clone(),
            on_command,
            viewport,
        }
        .render(window, cx)
    }

    /// Where the page shows: this PC's page window is placed over it every frame (clipped while the panel moves, out of
    /// the way under an overlay or in the snapshot variant's move, with a picture in its place); another PC's page is
    /// drawn as its picture.
    fn viewport(
        &mut self,
        bot: &Bot,
        view: &BrowserView,
        remote: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let moving = self.motion.is_moving();
        let snapshot_move = moving && self.motion.variant() == PanelVariant::Snapshot;
        let shielded = matches!(self.shield, ShieldState::Up(_));
        let picture = match &self.shield {
            ShieldState::Up(picture) => picture.clone(),
            _ if snapshot_move => self.snapshot.clone(),
            _ => None,
        };
        let placing = (!remote && self.mounted.as_deref() == Some(bot.id.as_str())).then(|| {
            let (bot_id, panel_bounds) = (bot.id.clone(), self.panel_bounds.clone());
            let live_move = moving && !snapshot_move;
            canvas(
                |_, _, _| {},
                move |bounds, _, window, cx| {
                    let scale = window.scale_factor();
                    let rect = local::physical(bounds, scale);
                    let clip = live_move
                        .then(|| panel_bounds.get())
                        .flatten()
                        .map(|panel| local::clip_to(&rect, &local::physical(panel, scale)));
                    let panel = Panel {
                        rect,
                        occluded: shielded || snapshot_move,
                        clip,
                    };
                    LocalBrowser::place(&bot_id, Some(panel), window, cx);
                    // A click anywhere in the app's own window takes the keyboard back from the page (GPUI limit 9).
                    window.on_mouse_event(move |_: &MouseDownEvent, phase, window, cx| {
                        guarded("take the keyboard back", || {
                            if phase == DispatchPhase::Capture {
                                LocalBrowser::focus_app(window, cx)
                            }
                        });
                    });
                },
            )
            .absolute()
            .size_full()
        });
        let waiting = view.state.is_none() && view.error.is_none();
        let private = chrome::private(view.state.as_ref());
        let remote_picture = remote
            .then(|| {
                self.picture.take(view.frame.as_ref(), private, window, cx);
                let entity = cx.entity();
                self.picture.element(move |frame, window, cx| {
                    entity.update(cx, |this, cx| {
                        this.command(BrowserCommand::Drawn(frame), window, cx)
                    });
                })
            })
            .flatten();
        let theme = theme(cx);
        let colors = palette(cx);
        div()
            .semantic("browser.viewport", Role::Application)
            .aria_label("Browser viewport")
            .relative()
            .flex_1()
            .min_h(r(240.))
            .mt(r(theme.space.s10))
            .mx(r(theme.space.s12))
            .mb(r(theme.space.s12))
            .rounded(r(theme.radius.md))
            .overflow_hidden()
            .bg(hsla(colors.surface.raised))
            .children(placing)
            .when_some(picture, |frame, picture| {
                frame.child(img(picture).absolute().size_full())
            })
            .children(remote_picture)
            .when(waiting, |frame| {
                frame.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .type_style(&theme.typography.desktop.small)
                        .text_color(hsla(colors.text.secondary))
                        .child("Opening the browser…"),
                )
            })
            .into_any_element()
    }
}

/// One choice of the Details | Browser switch: the chosen one raised.
fn segment(
    id: &'static str,
    label: &'static str,
    on: bool,
    choose: impl Fn(&mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let theme = theme(cx);
    let colors = palette(cx);
    div()
        .semantic(id, Role::Tab)
        .aria_label(label)
        .aria_selected(on)
        .flex()
        .items_center()
        .h(r(30.))
        .px(r(theme.space.s14))
        .rounded_full()
        .cursor_pointer()
        .type_style(&theme.typography.desktop.button)
        .map(|segment| {
            if on {
                segment
                    .bg(hsla(colors.surface.raised))
                    .text_color(hsla(colors.text.primary))
                    .shadow(shadows(&theme.shadow.light.card))
            } else {
                segment.text_color(hsla(colors.text.secondary))
            }
        })
        .on_click(move |_, window, cx| {
            guarded("switch the panel's view", || choose(window, cx));
        })
        .child(label)
        .into_any_element()
}

#[cfg(test)]
#[path = "panel_tests.rs"]
mod tests;
