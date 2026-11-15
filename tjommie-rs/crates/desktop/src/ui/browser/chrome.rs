//! The browser's chrome (`BrowserChrome` in browser-panel.tsx; Aegis browser-panel.html): the tabs, the address bar with
//! back, forward, reload and the logins menu, who is in control with Take control or Return to agent, the Google notice,
//! the page's dialog, then the page's place and what went wrong. It draws a `BrowserView` and sends `BrowserCommand`s;
//! only the person in control may use the page's controls.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Entity, FocusHandle, InteractiveElement as _, IntoElement, ParentElement as _,
    Role, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div, svg,
};
use gpui_kit::base::input::InputState;
use protocol::browser::{BrowserControl, BrowserState, BrowserTab, HistoryAction, TabAction};
use protocol::model::{BotColor, BotShape};

use super::{BrowserCommand, BrowserView, logins};
use crate::guard::guarded;
use crate::keymap::Dismiss;
use crate::model::browser_geometry::google_sign_in_rejected;
use crate::semantic::SemanticExt as _;
use crate::ui::primitives::avatar::BotAvatar;
use crate::ui::primitives::button::{IconButton, Pill, PillVariant};
use crate::ui::primitives::focus_handle;
use crate::ui::style::{TypeExt as _, hsla, palette, r, shadows, theme};

/// Takes a command from the chrome to the bot's browser.
pub type OnCommand = Rc<dyn Fn(BrowserCommand, &mut Window, &mut App)>;

/// Whether the person here holds the page: in control, allowed to be, and not in a private interaction.
pub fn human(state: Option<&BrowserState>) -> bool {
    state.is_some_and(|state| {
        state.control == BrowserControl::Human
            && state.can_control != Some(false)
            && state.private_active != Some(true)
    })
}

/// Whether the page is in a private interaction: no picture of it is shown, taken or kept.
pub fn private(state: Option<&BrowserState>) -> bool {
    state.is_some_and(|state| state.private_active == Some(true))
}

/// Who is in control, in words (Aegis "Who is in control").
pub fn who(state: &BrowserState, bot_name: &str) -> String {
    if state.private_active == Some(true) {
        "Private interaction. Automation and pictures are paused.".into()
    } else if human(Some(state)) {
        "You're in control. Shift+Tab leaves the page.".into()
    } else if state.control == BrowserControl::Human {
        "Another device is controlling this browser. You can watch.".into()
    } else {
        format!("{bot_name} is using this browser")
    }
}

/// A tab's name: its title, else its address, else "New tab".
pub fn tab_title(tab: &BrowserTab) -> String {
    [&tab.title, &tab.url]
        .into_iter()
        .find(|text| !text.is_empty())
        .cloned()
        .unwrap_or_else(|| "New tab".into())
}

/// The active tab of a state.
pub fn active_tab(state: &BrowserState) -> Option<&BrowserTab> {
    state
        .tabs
        .iter()
        .find(|tab| Some(&tab.id) == state.active_page_id.as_ref())
}

pub struct Chrome<'a> {
    pub view: &'a BrowserView,
    pub bot_id: String,
    pub bot_name: SharedString,
    pub bot_color: BotColor,
    pub bot_shape: BotShape,
    pub address: Entity<InputState>,
    /// The person is typing an address; Escape in the bar puts the page's back.
    pub address_draft: Rc<Cell<bool>>,
    pub prompt: Entity<InputState>,
    /// Where the keyboard lands when it leaves the page: the control pill.
    pub control_focus: Rc<RefCell<Option<FocusHandle>>>,
    pub on_command: OnCommand,
    /// The page's place.
    pub viewport: AnyElement,
}

impl Chrome<'_> {
    pub fn render(self, window: &mut Window, cx: &mut App) -> AnyElement {
        let theme = theme(cx).clone();
        let colors = palette(cx).clone();
        let state = self.view.state.as_ref();
        let human = human(state);
        let on_command = self.on_command.clone();
        let send = move |command: BrowserCommand| -> SendFn {
            let on_command = on_command.clone();
            Box::new(move |window: &mut Window, cx: &mut App| {
                on_command(command.clone(), window, cx)
            })
        };
        let mut chrome = div().flex_1().min_h_0().flex().flex_col();
        if let Some(state) = state {
            chrome = chrome
                .child(self.tabs(state, human, &send, &theme, &colors))
                .child(self.address_row(state, human, &send, window, cx))
                .child(self.control_bar(state, human, &send, window, cx));
            if active_tab(state).is_some_and(|tab| google_sign_in_rejected(&tab.url)) {
                chrome = chrome.child(
                    div()
                        .semantic("browser.google", Role::Paragraph)
                        .aria_label("Google may block sign-in in automated browsers.")
                        .mt(r(theme.space.s8))
                        .mx(r(theme.space.s12))
                        .type_style(&theme.typography.desktop.small)
                        .text_color(hsla(colors.text.secondary))
                        .child("Google may block sign-in in automated browsers."),
                );
            }
            if let Some(dialog) = &state.dialog {
                chrome = chrome.child(page_dialog(
                    dialog,
                    human,
                    &self.prompt,
                    &self.on_command,
                    &send,
                    &theme,
                    &colors,
                ));
            }
        }
        let retry = state.is_none();
        chrome
            .child(self.viewport)
            .when_some(self.view.error.clone(), |chrome, error| {
                chrome.child(
                    div()
                        .semantic("browser.error", Role::Alert)
                        .aria_label(error.clone())
                        .flex()
                        .items_center()
                        .gap(r(theme.space.s8))
                        .mx(r(theme.space.s12))
                        .mb(r(theme.space.s12))
                        .type_style(&theme.typography.desktop.small)
                        .text_color(hsla(theme.color.bot.red))
                        .child(div().flex_1().min_w_0().child(error))
                        .when(retry, |row| {
                            row.child(
                                Pill::new("browser.retry", "Try again")
                                    .small()
                                    .on_press(send(BrowserCommand::Retry)),
                            )
                        }),
                )
            })
            .into_any_element()
    }

    /// The tabs: each selects or closes, and New tab, for the person in control only.
    fn tabs(
        &self,
        state: &BrowserState,
        human: bool,
        send: &impl Fn(BrowserCommand) -> SendFn,
        theme: &crate::theme::Theme,
        colors: &crate::theme::Palette,
    ) -> impl IntoElement {
        let tabs = state.tabs.iter().map(|tab| {
            let title = tab_title(tab);
            let on = Some(&tab.id) == state.active_page_id.as_ref();
            let select = send(BrowserCommand::Tab {
                action: TabAction::Select,
                page_id: Some(tab.id.clone()),
            });
            let close = send(BrowserCommand::Tab {
                action: TabAction::Close,
                page_id: Some(tab.id.clone()),
            });
            div()
                .semantic(format!("browser.tab.{}", tab.id), Role::Tab)
                .aria_label(format!("Browser tab {title}"))
                .aria_selected(on)
                .flex()
                .items_center()
                .gap(r(theme.space.s8))
                .h(r(32.))
                .min_w_0()
                .max_w(r(270.))
                .pl(r(theme.space.s12))
                .pr(r(theme.space.s8))
                .rounded(r(theme.radius.field))
                .type_style(&theme.typography.desktop.small)
                .map(|tab| {
                    if on {
                        tab.bg(hsla(colors.surface.raised))
                            .text_color(hsla(colors.text.primary))
                            .shadow(shadows(&theme.shadow.light.card))
                    } else {
                        tab.text_color(hsla(colors.text.secondary))
                    }
                })
                .when(human, |tab| {
                    tab.cursor_pointer().on_click(move |_, window, cx| {
                        guarded("select a browser tab", || select(window, cx));
                    })
                })
                // The design's tab icon is the site's favicon, which the engine doesn't report: the title stands alone.
                .child(div().flex_1().min_w_0().truncate().child(title.clone()))
                .child(
                    div()
                        .semantic(format!("browser.tab.{}.close", tab.id), Role::Button)
                        .aria_label(format!("Close tab {title}"))
                        .flex_none()
                        .when(human, |x| {
                            x.cursor_pointer().on_click(move |_, window, cx| {
                                guarded("close a browser tab", || {
                                    cx.stop_propagation();
                                    close(window, cx)
                                });
                            })
                        })
                        .child(
                            svg()
                                .path("icons/i/175/x.svg")
                                .size(r(14.))
                                .text_color(hsla(colors.icon.muted)),
                        ),
                )
        });
        let new_tab = IconButton::new("browser.new_tab", "plus", "New tab").when(human, |button| {
            button.on_press(send(BrowserCommand::Tab {
                action: TabAction::New,
                page_id: None,
            }))
        });
        div()
            .semantic("browser.tabs", Role::TabList)
            .aria_label("Browser tabs")
            .flex()
            .flex_none()
            .items_center()
            .gap(r(theme.space.s4))
            .h(r(40.))
            .px(r(theme.space.s12))
            .overflow_hidden()
            .children(tabs)
            .child(new_tab)
    }

    /// Back, forward, reload, the address and the logins menu.
    fn address_row(
        &self,
        state: &BrowserState,
        human: bool,
        send: &impl Fn(BrowserCommand) -> SendFn,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        let theme = theme(cx).clone();
        let colors = palette(cx).clone();
        let button =
            |id: &'static str, icon: &'static str, label: &'static str, action: HistoryAction| {
                IconButton::new(id, icon, label).when(human, |button| {
                    button.on_press(send(BrowserCommand::History(action)))
                })
            };
        let (draft, address, url) = (
            self.address_draft.clone(),
            self.address.clone(),
            active_tab(state)
                .map(|tab| tab.url.clone())
                .unwrap_or_default(),
        );
        let field = div()
            .semantic("browser.address", Role::TextInput)
            .aria_label("Browser address")
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .gap(r(theme.space.s8))
            .h(r(34.))
            .px(r(theme.space.s12))
            .rounded_full()
            .bg(hsla(colors.surface.raised))
            .type_style(&theme.typography.desktop.small)
            .text_color(hsla(colors.text.primary))
            // Escape while typing an address puts the page's back and keeps the panel open.
            .on_action(move |_: &Dismiss, window, cx| {
                guarded("cancel the typed address", || {
                    if draft.replace(false) {
                        address.update(cx, |input, cx| input.set_value(url.clone(), window, cx));
                    } else {
                        cx.propagate();
                    }
                });
            })
            .child(
                svg()
                    .path("icons/i/175/lock.svg")
                    .size(r(14.))
                    .flex_none()
                    .text_color(hsla(colors.icon.muted)),
            )
            .child(div().flex_1().min_w_0().child(self.address.clone()));
        let menu = logins::menu(state, window, cx);
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(r(theme.space.s4))
            .h(r(46.))
            .pt(r(theme.space.s6))
            .px(r(theme.space.s12))
            .child(button(
                "browser.back",
                "arrow-left",
                "Back",
                HistoryAction::Back,
            ))
            .child(button(
                "browser.forward",
                "arrow-right",
                "Forward",
                HistoryAction::Forward,
            ))
            .child(button(
                "browser.reload",
                "rotate-cw",
                "Reload",
                HistoryAction::Reload,
            ))
            .child(field)
            .child(menu)
    }

    /// Who is in control, with Take control or Return to agent (raised white pills: Approve stays the one near-black
    /// action beside the chat).
    fn control_bar(
        &self,
        state: &BrowserState,
        human: bool,
        send: &impl Fn(BrowserCommand) -> SendFn,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        let theme = theme(cx).clone();
        let colors = palette(cx).clone();
        let private = state.private_active == Some(true);
        let pill = (!private).then(|| {
            *self.control_focus.borrow_mut() = Some(focus_handle("browser.control", window, cx));
            let (label, control) = if human {
                ("Return to agent", BrowserControl::Bot)
            } else {
                ("Take control", BrowserControl::Human)
            };
            Pill::new("browser.control", label)
                .variant(PillVariant::Raised)
                .small()
                .on_press(send(BrowserCommand::SetControl(control)))
        });
        let who = who(state, &self.bot_name);
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap(r(theme.space.s10))
            .mt(r(theme.space.s10))
            .mx(r(theme.space.s12))
            .py(r(theme.space.s8))
            .pl(r(theme.space.s12))
            .pr(r(theme.space.s8))
            .rounded(r(theme.radius.md))
            .bg(hsla(colors.surface.sunk))
            .child(BotAvatar::new(
                self.bot_color,
                self.bot_shape,
                theme.size.avatar20,
            ))
            .child(
                div()
                    .semantic("browser.who", Role::Status)
                    .aria_label(who.clone())
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .type_style(&theme.typography.desktop.ui)
                    .text_color(hsla(colors.text.primary))
                    .child(who),
            )
            .children(pill)
    }
}

type SendFn = Box<dyn Fn(&mut Window, &mut App)>;

/// The page's JavaScript dialog: answerable only by the person in control.
fn page_dialog(
    dialog: &protocol::browser::BrowserDialog,
    human: bool,
    prompt: &Entity<InputState>,
    on_command: &OnCommand,
    send: &impl Fn(BrowserCommand) -> SendFn,
    theme: &crate::theme::Theme,
    colors: &crate::theme::Palette,
) -> impl IntoElement {
    let is_prompt = dialog.kind == "prompt";
    let (answer, on_command) = (prompt.clone(), on_command.clone());
    let ok = move |window: &mut Window, cx: &mut App| {
        let prompt_text = is_prompt.then(|| answer.read(cx).value().to_string());
        on_command(
            BrowserCommand::Dialog {
                accept: true,
                prompt_text,
            },
            window,
            cx,
        );
    };
    let cancel = send(BrowserCommand::Dialog {
        accept: false,
        prompt_text: None,
    });
    div()
        .semantic("browser.dialog", Role::AlertDialog)
        .aria_label("The page asks")
        // Escape in the dialog (its answer, its buttons) cancels it and keeps the panel, as `right-panel.tsx` kept
        // Escape for an alertdialog (review F7). The bot's dialog isn't the person's to answer: Escape goes on.
        .on_action(move |_: &Dismiss, window, cx| {
            guarded("cancel the page's dialog by key", || {
                if human {
                    cancel(window, cx);
                } else {
                    cx.propagate();
                }
            });
        })
        .flex()
        .flex_col()
        .gap(r(theme.space.s8))
        .mt(r(theme.space.s10))
        .mx(r(theme.space.s12))
        .p(r(theme.space.s12))
        .rounded(r(theme.radius.md))
        .bg(hsla(colors.surface.raised))
        .shadow(shadows(&theme.shadow.light.card))
        .type_style(&theme.typography.desktop.small)
        .text_color(hsla(colors.text.primary))
        .child(div().child(dialog.message.clone()))
        .when(is_prompt, |bar| {
            bar.child(
                div()
                    .semantic("browser.dialog.answer", Role::TextInput)
                    .aria_label("Your answer")
                    .h(r(32.))
                    .px(r(theme.space.s12))
                    .flex()
                    .items_center()
                    .rounded_full()
                    .bg(hsla(colors.surface.sunk))
                    .child(prompt.clone()),
            )
        })
        .map(|bar| {
            if human {
                bar.child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(r(theme.space.s8))
                        .when(dialog.kind != "alert", |buttons| {
                            buttons.child(
                                Pill::new("browser.dialog.cancel", "Cancel")
                                    .small()
                                    .on_press(send(BrowserCommand::Dialog {
                                        accept: false,
                                        prompt_text: None,
                                    })),
                            )
                        })
                        .child(Pill::new("browser.dialog.ok", "OK").small().on_press(ok)),
                )
            } else {
                bar.child(
                    div()
                        .semantic("browser.dialog.take", Role::Paragraph)
                        .aria_label("Take control to answer.")
                        .text_color(hsla(colors.text.secondary))
                        .child("Take control to answer."),
                )
            }
        })
}
