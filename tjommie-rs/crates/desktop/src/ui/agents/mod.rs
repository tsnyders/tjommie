//! The Agents overview (Phase 3 plan T5.1; `app/agents/page.tsx`; desktop-agents.html): the title and summary, the
//! filter, search and New agent; then Needs you (the agents waiting for you, with Review), the agents' cards, the
//! rooms' cards and the New agent and New room tiles.
//!
//! Each card reads its agent's newest messages, and a room's card its newest posts: the screen loads their
//! conversations once (`ui::store::load_conversation`), and events keep them current, as the TypeScript's digests and
//! `useRoomPosts` did.

pub(crate) mod cards;

use std::collections::HashMap;

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Role, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div,
};
use gpui_kit::component::input::{InputEvent, InputState};
use protocol::model::{Bot, BotState};

use crate::guard::guarded;
use crate::model::overview::{OverviewFilter, matches, overview_summary};
use crate::model::state::{AppState, Connection, ConversationStatus};
use crate::motion::digits::Rolling;
use crate::semantic::SemanticExt as _;
use crate::ui::fields::{Look, RadioGroup, TextField};
use crate::ui::primitives::button::{IconButton, IconVariant, Pill, PillVariant};
use crate::ui::primitives::scroll::{follow_focus, scroll_handle};
use crate::ui::shell::route::{Navigate as _, Route};
use crate::ui::store::{self, History};
use crate::ui::style::{TypeExt as _, hsla, palette, r, theme};

/// The filters in the order they show (`FILTERS`).
const FILTERS: [(OverviewFilter, &str, &str); 4] = [
    (OverviewFilter::All, "all", "All"),
    (OverviewFilter::Working, "working", "Working"),
    (OverviewFilter::Waiting, "waiting", "Waiting"),
    (OverviewFilter::Paused, "paused", "Paused"),
];

/// The Agents overview, kept while the route shows it.
pub fn screen(window: &mut Window, cx: &mut App) -> AnyElement {
    window
        .use_keyed_state("screen.agents.view", cx, AgentsScreen::new)
        .into_any_element()
}

pub struct AgentsScreen {
    filter: OverviewFilter,
    search: Entity<InputState>,
    searching: bool,
    /// Why a card's last menu action failed, by agent.
    problems: HashMap<String, SharedString>,
    /// The conversations this screen asked for, and whether each has been in the state since. One that was and is gone
    /// (a reconnect forgets a chat still loading or failed) is asked for again (M5 review M3).
    asked: HashMap<String, bool>,
    /// The agents as last seen: when they change, a failed load is tried again, as the TypeScript fetched again on
    /// `[bots, ready]`.
    bots: Vec<Bot>,
    /// The Needs you count, rolling.
    needs: Rolling,
    _search: Subscription,
}

impl AgentsScreen {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search agents"));
        let subscription = cx.subscribe_in(&search, window, |this, search, event, _, cx| {
            guarded("search the agents", || {
                // The box closes when it's left empty (`onBlur`).
                if matches!(event, InputEvent::Blur) && search.read(cx).value().is_empty() {
                    this.searching = false;
                }
                cx.notify();
            });
        });
        let needs = Rolling::new("0", &theme(cx).motion.spring.playful);
        AgentsScreen {
            filter: OverviewFilter::All,
            search,
            searching: false,
            problems: HashMap::new(),
            asked: HashMap::new(),
            bots: Vec::new(),
            needs,
            _search: subscription,
        }
    }

    /// Asks for each agent's and room's newest page, when connected: once, again when a reconnect has forgotten it, and
    /// a failed one again when the agents change.
    fn load(&mut self, state: &AppState, cx: &mut Context<Self>) {
        if state.connection != Connection::Ready {
            return;
        }
        let retry = self.bots != state.bots;
        if retry {
            self.bots.clone_from(&state.bots);
        }
        let wanted = state
            .bots
            .iter()
            .map(|bot| (bot.id.clone(), History::Bot(bot.id.clone())))
            .chain(
                state
                    .rooms
                    .iter()
                    .map(|room| (room.id.clone(), History::Room(room.id.clone()))),
            );
        for (id, history) in wanted {
            let status = state.conversations.get(&id).map(|chat| chat.status);
            match (status, self.asked.get(&id).copied()) {
                // Never asked, or it was there and is gone.
                (None, None | Some(true)) => {
                    store::load_conversation(cx, &id, history).detach();
                    self.asked.insert(id, false);
                }
                (Some(ConversationStatus::Failed), _) if retry => {
                    store::retry_conversation(cx, &id, history).detach();
                    self.asked.insert(id, false);
                }
                (Some(_), None | Some(false)) => {
                    self.asked.insert(id, true);
                }
                // Still on its way, or there as it was.
                (None, Some(false)) | (Some(_), Some(true)) => {}
            }
        }
    }

    pub(super) fn set_problem(&mut self, bot_id: &str, problem: Option<SharedString>) {
        match problem {
            Some(problem) => self.problems.insert(bot_id.to_owned(), problem),
            None => self.problems.remove(bot_id),
        };
    }
}

impl Render for AgentsScreen {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = store::snapshot(cx);
        self.load(&state, cx);
        let query = self.search.read(cx).value().to_string();
        let everything = self.filter == OverviewFilter::All && query.trim().is_empty();
        let shown: Vec<&Bot> = state
            .bots
            .iter()
            .filter(|bot| matches(bot, self.filter, &query))
            .collect();
        let waiting: Vec<&Bot> = state
            .bots
            .iter()
            .filter(|bot| bot.state == BotState::Waiting)
            .collect();
        self.needs
            .set(waiting.len().to_string(), crate::motion::reduced::is_on(cx));
        if self.needs.step(crate::motion::clock::dt(cx)) {
            crate::motion::clock::keep_moving(window, cx);
        }
        let now = jiff::Zoned::now();
        let messages = |id: &str| {
            state
                .conversations
                .get(id)
                .filter(|chat| chat.status == ConversationStatus::Ready)
                .map(|chat| chat.messages.as_slice())
        };
        let screen = cx.entity();
        // What needs the app mutably (focus handles) comes first; then the theme is read.
        let header = self.header(&state, window, cx);
        // No agents yet and nobody signed in: the first-run setup in place of the overview (M5 review M6).
        let first_run =
            crate::ui::setup::wanted(&state).then(|| crate::ui::setup::first_run(window, cx));
        let overview = first_run.is_none();
        let mut grid: Vec<AnyElement> = shown
            .iter()
            .map(|bot| {
                let problem = self.problems.get(&bot.id).cloned();
                cards::bot_card(
                    bot,
                    &state.bots,
                    messages(&bot.id).unwrap_or(&[]),
                    &state.routines,
                    problem,
                    &now,
                    screen.clone(),
                    window,
                    cx,
                )
            })
            .collect();
        if everything {
            grid.extend(state.rooms.iter().map(|room| {
                cards::room_card(
                    "agents.room",
                    room,
                    messages(&room.id),
                    &state.bots,
                    None,
                    &now,
                    cx,
                )
            }));
            grid.push(cards::new_bot_tile(window, cx));
            grid.push(cards::new_room_tile(
                "agents.new_room",
                |window, cx| crate::ui::rooms::open_room_form(None, window, cx),
                window,
                cx,
            ));
        }
        let scroll = scroll_handle("agents.scroll", window, cx);
        let theme = theme(cx);
        let colors = palette(cx);
        let columns = columns(window, cx);
        let needs_you = (everything && !waiting.is_empty()).then(|| {
            let cards: Vec<AnyElement> = waiting
                .iter()
                .map(|bot| cards::needs_you(bot, messages(&bot.id).unwrap_or(&[]), &now, cx))
                .collect();
            div()
                .flex()
                .flex_col()
                .gap(r(theme.space.s12))
                .child(
                    section_label("Needs you", cx)
                        .child(
                            div()
                                .flex_none()
                                .size(gpui::px(7.))
                                .rounded_full()
                                .bg(hsla(colors.accent.marker)),
                        )
                        .child(
                            div()
                                .semantic("agents.needs.count", Role::Status)
                                .aria_label(format!("{} need you", waiting.len()))
                                .font_family(crate::assets::MONO_FAMILY)
                                .child(
                                    self.needs
                                        .render(theme.typography.desktop.label.line_height),
                                ),
                        ),
                )
                .child(
                    div()
                        .grid()
                        .grid_cols(columns)
                        .gap(crate::ui::style::card_gap(cx))
                        .children(cards),
                )
        });
        let empty = (!everything && shown.is_empty()).then(|| {
            div()
                .semantic("agents.empty", Role::Paragraph)
                .aria_label("No agents match.")
                .mt(r(theme.space.s40))
                .text_center()
                .type_style(&theme.typography.desktop.ui)
                .text_color(hsla(colors.text.secondary))
                .child("No agents match.")
        });
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(r(theme.space.s24))
            .pt(r(theme.space.s32))
            .px(r(theme.space.s32))
            .child(header)
            .child(follow_focus(
                &scroll,
                div()
                    .id("agents.scroll")
                    .track_scroll(&scroll)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .pb(r(theme.space.s28))
                    .flex()
                    .flex_col()
                    .gap(r(theme.space.s24))
                    .children(first_run)
                    .children(needs_you)
                    .children(overview.then(|| {
                        div()
                            .flex()
                            .flex_col()
                            .gap(r(theme.space.s12))
                            .child(section_label("Agents", cx))
                            .child(
                                div()
                                    .grid()
                                    .grid_cols(columns)
                                    .gap(crate::ui::style::card_gap(cx))
                                    .children(grid),
                            )
                    }))
                    .children(empty.filter(|_| overview)),
            ))
    }
}

impl AgentsScreen {
    fn header(
        &mut self,
        state: &AppState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let colors = palette(cx);
        let screen = cx.entity();
        let filter = FILTERS.iter().fold(
            RadioGroup::new("agents.filter", "Show", self.filter).look(Look::Segmented),
            |group, (value, key, label)| group.choice(*value, *key, *label),
        );
        let on_filter = screen.clone();
        let filter = filter.on_choose(move |value, _, cx| {
            on_filter.update(cx, |this, cx| {
                this.filter = value;
                cx.notify();
            });
        });
        let search: AnyElement = if self.searching {
            div()
                .w(r(200.))
                .child(TextField::new(
                    "agents.search",
                    "Search agents",
                    &self.search,
                ))
                .into_any_element()
        } else {
            let open = screen.clone();
            let field = self.search.clone();
            IconButton::new("agents.search.open", "search", "Search agents")
                .variant(IconVariant::Grey)
                .on_press(move |window, cx| {
                    open.update(cx, |this, cx| {
                        this.searching = true;
                        cx.notify();
                    });
                    field.update(cx, |field, cx| field.focus(window, cx));
                })
                .into_any_element()
        };
        let _ = window;
        div()
            .flex()
            .items_end()
            .gap(r(theme.space.s16))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .semantic("screen.agents", Role::Heading)
                            .aria_label("Agents")
                            .type_style(&theme.typography.desktop.display_l)
                            .text_color(hsla(colors.text.primary))
                            .child("Agents"),
                    )
                    .child(
                        div()
                            .semantic("agents.summary", Role::Paragraph)
                            .aria_label(overview_summary(&state.bots))
                            .mt(r(theme.space.s10))
                            .type_style(&theme.typography.desktop.ui)
                            .text_color(hsla(colors.text.secondary))
                            .child(overview_summary(&state.bots)),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(r(theme.space.s8))
                    .child(filter)
                    .child(search)
                    .child(
                        Pill::new("agents.new", "New agent")
                            .variant(PillVariant::Primary)
                            .small()
                            .icon("plus")
                            .on_press(|_, cx| cx.navigate(Route::NewBot)),
                    ),
            )
            .into_any_element()
    }
}

/// A section's label (`.section-label`).
fn section_label(text: &'static str, cx: &App) -> gpui::Div {
    let theme = theme(cx);
    div()
        .flex()
        .items_center()
        .gap(r(theme.space.s8))
        .type_style(&theme.typography.desktop.label)
        .text_color(hsla(palette(cx).text.tertiary))
        .child(text)
}

/// The grid's columns: as many 280 px cards as the main column holds (`minmax(280px, 1fr)`), one to four.
pub(crate) fn columns(window: &Window, cx: &App) -> u16 {
    let theme = theme(cx);
    let rem = f32::from(window.rem_size()) / 16.0;
    let chrome = (theme.size.sidebar + theme.space.s12 + 2.0 * theme.space.s32) * rem;
    let width = f32::from(window.viewport_size().width) - chrome;
    let card = (280.0 + theme.space.s16) * rem;
    ((width + theme.space.s16 * rem) / card)
        .floor()
        .clamp(1.0, 4.0) as u16
}

#[cfg(test)]
mod tests;
