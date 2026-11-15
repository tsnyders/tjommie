//! Port of `packages/ui/lib/state.ts`: the app's state and how each event changes it. The TypeScript returned a new state
//! for each change. Here a function changes the state in place and returns whether its content changed, which is what
//! the store tells its subscribers. Where the TypeScript compared objects by identity, this compares content.

use std::collections::{BTreeMap, HashSet};

use protocol::Event;
use protocol::accounts::AccountInfo;
use protocol::model::{Bot, Message, ProviderSetup, Room, Routine, RoutineRun, Settings};
use protocol::wire::{Id, MessageDelta, PendingNotification};

/// The connection's timing, in milliseconds (`LINK`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    pub ping_foreground_ms: u64,
    pub ping_background_ms: u64,
    pub pong_timeout_ms: u64,
    pub lan_window_ms: u64,
    pub backoff_ms: [u64; 5],
}

pub const LINK: Link = Link {
    ping_foreground_ms: 20_000,
    ping_background_ms: 60_000,
    pong_timeout_ms: 10_000,
    lan_window_ms: 1_000,
    backoff_ms: [0, 1_000, 2_000, 5_000, 10_000],
};

/// How many sign-in attempts the state remembers.
const SIGN_INS_KEPT: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfflineReason {
    HostOffline,
    Unreachable,
}

/// Where the connection stands. A saved remote PC that can't be reached keeps its cached screens and is `Offline`;
/// `Failed` is only for the desktop's own daemon and a first connect with nothing cached. `Removed` is the PC refusing
/// this device, which is final.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Connection {
    Connecting,
    Ready,
    Reconnecting,
    Offline { reason: OfflineReason },
    Removed,
    Update,
    Failed { reason: String },
    Outside,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConversationStatus {
    Loading,
    Ready,
    Failed,
}

/// A message event that changes a chat: the three message events of the protocol.
#[derive(Debug, Clone, PartialEq)]
pub enum MessageEvent {
    Added(Message),
    Updated(Message),
    Delta(MessageDelta),
}

impl MessageEvent {
    fn conversation_id(&self) -> &str {
        match self {
            Self::Added(message) | Self::Updated(message) => &message.conversation_id,
            Self::Delta(delta) => &delta.conversation_id,
        }
    }
}

impl From<MessageEvent> for Event {
    fn from(event: MessageEvent) -> Self {
        match event {
            MessageEvent::Added(message) => Event::MessageAdded(message),
            MessageEvent::Updated(message) => Event::MessageUpdated(message),
            MessageEvent::Delta(delta) => Event::MessageDelta(delta),
        }
    }
}

/// A chat's messages. `pending` holds the message events that arrived while the first page was loading.
#[derive(Debug, Clone, PartialEq)]
pub struct Conversation {
    pub status: ConversationStatus,
    pub messages: Vec<Message>,
    pub has_more: bool,
    pub pending: Vec<MessageEvent>,
}

/// One page of a chat's history, newest last.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub messages: Vec<Message>,
    pub has_more: bool,
}

/// What `connected` is given: the lists fetched again on a new connection. `accounts` and `notifications` are `None` when
/// the daemon didn't send them (an older daemon has no accounts; notifications default to none).
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub bots: Vec<Bot>,
    pub settings: Settings,
    pub routines: Vec<Routine>,
    pub rooms: Vec<Room>,
    pub accounts: Option<Vec<AccountInfo>>,
    pub notifications: Option<Vec<PendingNotification>>,
}

/// The app's state. `setup`, `accounts` and `settings` are `None` until the first list or change arrives.
#[derive(Debug, Clone, PartialEq)]
pub struct AppState {
    pub connection: Connection,
    /// The saved PC this device is on (its id), or `None` for the desktop's own Tjommie and before a phone picks a PC.
    pub host: Option<String>,
    pub bots: Vec<Bot>,
    pub setup: Option<Vec<ProviderSetup>>,
    /// `None` until the first list arrives.
    pub accounts: Option<Vec<AccountInfo>>,
    /// Attempt ids of the latest `setup.signInEnded` events; a new connection keeps them.
    pub sign_ins_ended: Vec<String>,
    pub settings: Option<Settings>,
    pub routines: Vec<Routine>,
    /// Each routine's newest `routine.run` event, by routine id.
    pub latest_runs: BTreeMap<String, RoutineRun>,
    /// Everyone first, then the others oldest first.
    pub rooms: Vec<Room>,
    /// Routine failures not shown yet, listed again on each connection.
    pub notifications: Vec<PendingNotification>,
    pub conversations: BTreeMap<String, Conversation>,
}

/// `initialState`.
impl Default for AppState {
    fn default() -> Self {
        Self {
            connection: Connection::Connecting,
            host: None,
            bots: Vec::new(),
            setup: None,
            accounts: None,
            sign_ins_ended: Vec::new(),
            settings: None,
            routines: Vec::new(),
            latest_runs: BTreeMap::new(),
            rooms: Vec::new(),
            notifications: Vec::new(),
            conversations: BTreeMap::new(),
        }
    }
}

/// What the lists of bots, routines and rooms are keyed and ordered by.
trait Listed: Clone {
    fn id(&self) -> &str;
    /// `None` for Everyone, which has no creation time and so comes first.
    fn created_at(&self) -> Option<&str>;
}

impl Listed for Bot {
    fn id(&self) -> &str {
        &self.id
    }

    fn created_at(&self) -> Option<&str> {
        Some(&self.created_at)
    }
}

impl Listed for Routine {
    fn id(&self) -> &str {
        &self.id
    }

    fn created_at(&self) -> Option<&str> {
        Some(&self.created_at)
    }
}

impl Listed for Room {
    fn id(&self) -> &str {
        &self.id
    }

    fn created_at(&self) -> Option<&str> {
        self.created_at.as_deref()
    }
}

/// Stores `value` when it differs from what's there, and says whether it did.
fn set_changed<T: PartialEq>(slot: &mut T, value: T) -> bool {
    if *slot == value {
        false
    } else {
        *slot = value;
        true
    }
}

/// Drops the items `keep` refuses, and says whether any went.
fn retain_changed<T>(items: &mut Vec<T>, keep: impl FnMut(&T) -> bool) -> bool {
    let before = items.len();
    items.retain(keep);
    items.len() != before
}

/// Oldest first. The sort is stable, as JavaScript's is, so items created at the same time keep their order.
fn by_created<T: Listed>(mut items: Vec<T>) -> Vec<T> {
    items.sort_by(|a, b| {
        a.created_at()
            .unwrap_or("")
            .cmp(b.created_at().unwrap_or(""))
    });
    items
}

/// The list with `item` replacing the one with its id, placed by its creation time.
fn upsert<T: Listed>(list: &[T], item: T) -> Vec<T> {
    let mut next: Vec<T> = list
        .iter()
        .filter(|existing| existing.id() != item.id())
        .cloned()
        .collect();
    next.push(item);
    by_created(next)
}

/// Applies one message event to a chat's messages and says whether they changed. An update or delta for a message outside
/// the loaded pages (an old card being answered) changes nothing.
pub fn apply_message_event(messages: &mut Vec<Message>, event: &MessageEvent) -> bool {
    match event {
        MessageEvent::Added(message) => {
            if messages.iter().any(|shown| shown.id == message.id) {
                false
            } else {
                messages.push(message.clone());
                true
            }
        }
        MessageEvent::Updated(message) => {
            let mut changed = false;
            for shown in messages.iter_mut().filter(|shown| shown.id == message.id) {
                if *shown != *message {
                    *shown = message.clone();
                    changed = true;
                }
            }
            changed
        }
        MessageEvent::Delta(delta) => {
            // An empty delta adds no text, so nothing changes.
            if delta.delta.is_empty() {
                return false;
            }
            let mut changed = false;
            for shown in messages.iter_mut().filter(|shown| shown.id == delta.id) {
                shown.body.push_str(&delta.delta);
                changed = true;
            }
            changed
        }
    }
}

/// Applies one message event to its chat: queued while the chat is loading, applied to its messages otherwise. A chat
/// that isn't open ignores it: its history loads when it opens.
fn apply_message(state: &mut AppState, event: MessageEvent) -> bool {
    let Some(conversation) = state.conversations.get_mut(event.conversation_id()) else {
        return false;
    };
    if conversation.status == ConversationStatus::Loading {
        conversation.pending.push(event);
        true
    } else {
        apply_message_event(&mut conversation.messages, &event)
    }
}

/// Applies one event to the state (`applyEvent`) and says whether the state's content changed.
pub fn apply_event(state: &mut AppState, event: &Event) -> bool {
    match event {
        // ponytail: Re-sorting each state tick is fine below 100 bots; above that, preserve positions for unchanged createdAt.
        Event::BotUpdated(bot) => {
            let next = upsert(&state.bots, bot.clone());
            set_changed(&mut state.bots, next)
        }
        Event::BotDeleted(Id { id }) => {
            let mut changed = retain_changed(&mut state.bots, |bot| bot.id != *id);
            changed |= retain_changed(&mut state.routines, |routine| routine.bot_id != *id);
            for room in &mut state.rooms {
                changed |= retain_changed(&mut room.bot_ids, |bot_id| bot_id != id);
            }
            changed | state.conversations.remove(id).is_some()
        }
        Event::SetupChanged(setup) => set_changed(&mut state.setup, Some(setup.clone())),
        Event::AccountsChanged(accounts) => {
            set_changed(&mut state.accounts, Some(accounts.clone()))
        }
        Event::SetupSignInEnded(ended) => {
            state.sign_ins_ended.push(ended.attempt.clone());
            let excess = state.sign_ins_ended.len().saturating_sub(SIGN_INS_KEPT);
            state.sign_ins_ended.drain(..excess);
            true
        }
        Event::SettingsChanged(settings) => {
            set_changed(&mut state.settings, Some(settings.clone()))
        }
        Event::RoutineUpdated(routine) => {
            let next = upsert(&state.routines, routine.clone());
            set_changed(&mut state.routines, next)
        }
        Event::RoutineDeleted(Id { id }) => {
            retain_changed(&mut state.routines, |routine| routine.id != *id)
        }
        Event::RoomUpdated(room) => {
            let next = upsert(&state.rooms, room.clone());
            set_changed(&mut state.rooms, next)
        }
        Event::RoomDeleted(Id { id }) => {
            let changed = retain_changed(&mut state.rooms, |room| room.id != *id);
            changed | state.conversations.remove(id).is_some()
        }
        Event::RoutineRun(run) => {
            if state.latest_runs.get(&run.routine_id) == Some(run) {
                false
            } else {
                state
                    .latest_runs
                    .insert(run.routine_id.clone(), run.clone());
                true
            }
        }
        Event::MessageAdded(message) => apply_message(state, MessageEvent::Added(message.clone())),
        Event::MessageUpdated(message) => {
            apply_message(state, MessageEvent::Updated(message.clone()))
        }
        Event::MessageDelta(delta) => apply_message(state, MessageEvent::Delta(delta.clone())),
        // Approvals arrive as messages, and the desktop app shows notifications: nothing else changes the state here.
        _ => false,
    }
}

/// A chat is loading its first page (`startLoading`).
pub fn start_loading(state: &mut AppState, conversation_id: &str) -> bool {
    let next = Conversation {
        status: ConversationStatus::Loading,
        messages: Vec::new(),
        has_more: false,
        pending: Vec::new(),
    };
    if state.conversations.get(conversation_id) == Some(&next) {
        return false;
    }
    state.conversations.insert(conversation_id.to_owned(), next);
    true
}

/// The first page arrived. Events that came in meanwhile are replayed on top: the page already holds added and updated
/// messages, but not text that is still streaming, which the daemon only saves when it ends.
pub fn finish_loading(state: &mut AppState, conversation_id: &str, page: Page) -> bool {
    let Some(conversation) = state.conversations.get_mut(conversation_id) else {
        return false;
    };
    if conversation.status != ConversationStatus::Loading {
        return false;
    }
    let mut messages = page.messages;
    for event in &conversation.pending {
        apply_message_event(&mut messages, event);
    }
    *conversation = Conversation {
        status: ConversationStatus::Ready,
        messages,
        has_more: page.has_more,
        pending: Vec::new(),
    };
    true
}

/// An older page of a loaded chat goes above what's shown, without duplicates (`addOlder`).
pub fn add_older(state: &mut AppState, conversation_id: &str, page: Page) -> bool {
    let Some(conversation) = state.conversations.get_mut(conversation_id) else {
        return false;
    };
    if conversation.status != ConversationStatus::Ready {
        return false;
    }
    let older: Vec<Message> = {
        let known: HashSet<&str> = conversation
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect();
        page.messages
            .into_iter()
            .filter(|message| !known.contains(message.id.as_str()))
            .collect()
    };
    let changed = !older.is_empty() || conversation.has_more != page.has_more;
    let mut merged = older;
    merged.append(&mut conversation.messages);
    conversation.messages = merged;
    conversation.has_more = page.has_more;
    changed
}

/// A chat is dropped from the state (`forgetConversation`).
pub fn forget_conversation(state: &mut AppState, conversation_id: &str) -> bool {
    state.conversations.remove(conversation_id).is_some()
}

/// A new connection: the lists are fetched again. Loaded chats stay on screen and catch up (`catch_up`); one still loading
/// or failed is forgotten, so its screen loads it again.
pub fn connected(state: &mut AppState, snapshot: Snapshot) -> bool {
    let mut changed = set_changed(&mut state.connection, Connection::Ready);
    changed |= set_changed(&mut state.bots, by_created(snapshot.bots));
    changed |= set_changed(&mut state.settings, Some(snapshot.settings));
    changed |= set_changed(&mut state.routines, by_created(snapshot.routines));
    changed |= set_changed(&mut state.rooms, by_created(snapshot.rooms));
    changed |= set_changed(&mut state.accounts, snapshot.accounts);
    changed |= set_changed(
        &mut state.notifications,
        snapshot.notifications.unwrap_or_default(),
    );
    changed |= set_changed(&mut state.latest_runs, BTreeMap::new());
    let before = state.conversations.len();
    state
        .conversations
        .retain(|_, conversation| conversation.status == ConversationStatus::Ready);
    changed | (state.conversations.len() != before)
}

/// A loaded chat's newest page, fetched again after a reconnect (`catchUp`). When the page's oldest message is loaded,
/// the chat is replaced from there on; otherwise more than a page arrived while away, so the page replaces the chat and
/// older pages load again. Message events that arrived while the page was fetched are replayed on it first.
pub fn catch_up(
    state: &mut AppState,
    conversation_id: &str,
    page: Page,
    arrived: &[MessageEvent],
) -> bool {
    let Some(conversation) = state.conversations.get_mut(conversation_id) else {
        return false;
    };
    if conversation.status != ConversationStatus::Ready {
        return false;
    }
    // Where the page's oldest message sits in the chat, when it is loaded.
    let at = page.messages.first().and_then(|first| {
        conversation
            .messages
            .iter()
            .position(|message| message.id == first.id)
    });
    let mut newest = page.messages;
    for event in arrived {
        apply_message_event(&mut newest, event);
    }
    let (messages, has_more) = match at {
        Some(at) => {
            let mut kept: Vec<Message> = conversation.messages.iter().take(at).cloned().collect();
            kept.append(&mut newest);
            (kept, conversation.has_more)
        }
        None => {
            // More than a page arrived while away. The chat had messages before this, so more may load.
            let had_messages = !conversation.messages.is_empty();
            (newest, page.has_more || had_messages)
        }
    };
    let changed = messages != conversation.messages || has_more != conversation.has_more;
    conversation.messages = messages;
    conversation.has_more = has_more;
    changed
}

/// A room's bots, in the bots' order. Everyone always holds every bot: the daemon sends no `room.updated` for it when a
/// bot is made, so its `botIds` can be out of date.
pub fn room_bots<'a>(room: &Room, bots: &'a [Bot]) -> Vec<&'a Bot> {
    if room.everyone {
        bots.iter().collect()
    } else {
        bots.iter()
            .filter(|bot| room.bot_ids.contains(&bot.id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::model::{
        Autonomy, BotColor, BotShape, BotState, MessageKind, MessageStatus, Provider,
        RoutineRunStatus, TEAM_ROOM_ID, TimeZone,
    };
    use protocol::wire::{Empty, SetupSignInEnded};

    fn settings() -> Settings {
        Settings {
            chain_limit: 5,
            relay_url: None,
        }
    }

    fn snapshot(bots: Vec<Bot>, routines: Vec<Routine>, rooms: Vec<Room>) -> Snapshot {
        Snapshot {
            bots,
            settings: settings(),
            routines,
            rooms,
            accounts: None,
            notifications: None,
        }
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    fn bot(id: &str, created_at: &str) -> Bot {
        Bot {
            id: id.to_owned(),
            name: id.to_owned(),
            color: BotColor::Blue,
            shape: BotShape::Circle,
            instructions: String::new(),
            provider: Provider::Claude,
            model: "sonnet".to_owned(),
            account_id: None,
            effective_account_id: None,
            using_fallback: None,
            autonomy: Autonomy::Folder,
            home_folder: format!("C:/bots/{id}"),
            project_folder: None,
            browser: false,
            paused: false,
            can_message: Vec::new(),
            state: BotState::Idle,
            setup_reason: None,
            limited_until: None,
            created_at: created_at.to_owned(),
        }
    }

    fn bot_ids(bots: &[Bot]) -> Vec<&str> {
        bots.iter().map(|bot| bot.id.as_str()).collect()
    }

    fn room_ids(rooms: &[Room]) -> Vec<&str> {
        rooms.iter().map(|room| room.id.as_str()).collect()
    }

    fn room_names(rooms: &[Room]) -> Vec<&str> {
        rooms.iter().map(|room| room.name.as_str()).collect()
    }

    /// A done text message in chat `b1`, with an empty body.
    fn message(id: &str) -> Message {
        Message {
            id: id.to_owned(),
            conversation_id: "b1".to_owned(),
            author: "b1".to_owned(),
            kind: MessageKind::Text,
            body: String::new(),
            data: None,
            status: MessageStatus::Done,
            created_at: "2026-09-27T09:00:00.000Z".to_owned(),
        }
    }

    fn routine(id: &str, bot_id: &str) -> Routine {
        Routine {
            id: id.to_owned(),
            bot_id: bot_id.to_owned(),
            name: "Morning check".to_owned(),
            prompt: "Check pnet.co.za".to_owned(),
            days: vec![1, 2, 3, 4, 5],
            times: strings(&["08:48"]),
            time_zone: TimeZone::from_stored("Africa/Johannesburg"),
            time_limit_minutes: None,
            enabled: true,
            next_run_at: None,
            created_at: "2026-09-27T08:00:00.000Z".to_owned(),
        }
    }

    fn run(id: &str, routine_id: &str, status: RoutineRunStatus) -> Event {
        Event::RoutineRun(RoutineRun {
            id: id.to_owned(),
            bot_id: "b1".to_owned(),
            routine_id: routine_id.to_owned(),
            due_at: "2026-09-27T08:48:00.000Z".to_owned(),
            started_at: None,
            ended_at: None,
            status,
            note: None,
        })
    }

    fn everyone(bot_ids: &[&str]) -> Room {
        Room {
            id: TEAM_ROOM_ID.to_owned(),
            name: "Everyone".to_owned(),
            bot_ids: strings(bot_ids),
            everyone: true,
            created_at: None,
        }
    }

    fn room(id: &str, bot_ids: &[&str], created_at: &str) -> Room {
        Room {
            id: id.to_owned(),
            name: id.to_owned(),
            bot_ids: strings(bot_ids),
            everyone: false,
            created_at: Some(created_at.to_owned()),
        }
    }

    /// An account as the daemon lists it: only the fields with no default are sent.
    fn account(id: &str, label: &str) -> AccountInfo {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "provider": "claude",
            "label": label,
            "kind": "subscription",
            "isDefault": false,
            "isFallback": false,
            "createdAt": "2026-09-27T00:00:00Z",
            "hasCredential": true,
            "limits": [],
            "spending": { "knownUsd": 0.0, "unknownRows": 0, "capUsd": null, "blocked": false }
        }))
        .unwrap_or_else(|error| panic!("the account fixture is valid: {error}"))
    }

    fn added(data: Message) -> MessageEvent {
        MessageEvent::Added(data)
    }

    fn updated(data: Message) -> MessageEvent {
        MessageEvent::Updated(data)
    }

    fn delta(id: &str, text: &str) -> MessageEvent {
        MessageEvent::Delta(MessageDelta {
            id: id.to_owned(),
            conversation_id: "b1".to_owned(),
            delta: text.to_owned(),
        })
    }

    fn ended(provider: Provider, attempt: &str) -> Event {
        Event::SetupSignInEnded(SetupSignInEnded {
            provider,
            attempt: attempt.to_owned(),
        })
    }

    /// `ready` in the TypeScript tests: chat `b1` loaded with these messages.
    fn ready(messages: Vec<Message>) -> AppState {
        let mut state = AppState::default();
        start_loading(&mut state, "b1");
        finish_loading(
            &mut state,
            "b1",
            Page {
                messages,
                has_more: false,
            },
        );
        state
    }

    fn chat<'a>(state: &'a AppState, id: &str) -> &'a Conversation {
        state
            .conversations
            .get(id)
            .unwrap_or_else(|| panic!("chat {id} is loaded"))
    }

    fn message_ids<'a>(state: &'a AppState, id: &str) -> Vec<&'a str> {
        chat(state, id)
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect()
    }

    fn bodies<'a>(state: &'a AppState, id: &str) -> Vec<&'a str> {
        chat(state, id)
            .messages
            .iter()
            .map(|message| message.body.as_str())
            .collect()
    }

    fn id_body<'a>(state: &'a AppState, id: &str) -> Vec<(&'a str, &'a str)> {
        chat(state, id)
            .messages
            .iter()
            .map(|message| (message.id.as_str(), message.body.as_str()))
            .collect()
    }

    #[test]
    fn freezes_the_reconnect_timing_contract() {
        assert_eq!(
            LINK,
            Link {
                ping_foreground_ms: 20_000,
                ping_background_ms: 60_000,
                pong_timeout_ms: 10_000,
                lan_window_ms: 1_000,
                backoff_ms: [0, 1_000, 2_000, 5_000, 10_000],
            }
        );
    }

    #[test]
    fn holds_the_accounts_unknown_until_listed_replaced_whole_by_each_change_and_listed_again_on_a_new_connection()
     {
        let list = vec![account("a1", "Claude (Personal)")];
        let mut state = AppState::default();
        assert_eq!(state.accounts, None);
        connected(
            &mut state,
            Snapshot {
                accounts: Some(list.clone()),
                ..snapshot(vec![], vec![], vec![])
            },
        );
        assert_eq!(state.accounts, Some(list));
        let next = vec![account("a2", "Work")];
        assert!(apply_event(
            &mut state,
            &Event::AccountsChanged(next.clone())
        ));
        assert_eq!(state.accounts, Some(next));
        // An older daemon sends no accounts.
        connected(&mut state, snapshot(vec![], vec![], vec![]));
        assert_eq!(state.accounts, None);
    }

    #[test]
    fn keeps_bots_in_creation_order_and_forgets_a_deleted_bots_chat_and_routines() {
        let mut state = AppState::default();
        connected(
            &mut state,
            snapshot(
                vec![
                    bot("b2", "2026-09-27T10:00:00.000Z"),
                    bot("b1", "2026-09-27T09:00:00.000Z"),
                ],
                vec![routine("r1", "b1")],
                vec![],
            ),
        );
        assert_eq!(bot_ids(&state.bots), vec!["b1", "b2"]);
        assert!(apply_event(
            &mut state,
            &Event::BotUpdated(Bot {
                state: BotState::Working,
                ..bot("b1", "2026-09-27T09:00:00.000Z")
            })
        ));
        assert_eq!(state.bots[0].state, BotState::Working);
        start_loading(&mut state, "b1");
        assert!(apply_event(
            &mut state,
            &Event::BotDeleted(Id {
                id: "b1".to_owned()
            })
        ));
        assert_eq!(bot_ids(&state.bots), vec!["b2"]);
        assert!(state.routines.is_empty());
        assert!(!state.conversations.contains_key("b1"));
    }

    #[test]
    fn keeps_everyone_first_and_the_other_rooms_oldest_first_updates_and_removes_them_and_lists_them_again_on_a_new_connection()
     {
        let growth = room("r2", &["b1"], "2026-10-25T10:00:00.000Z");
        let mut state = AppState::default();
        connected(
            &mut state,
            snapshot(vec![], vec![], vec![growth.clone(), everyone(&["b1"])]),
        );
        assert_eq!(room_ids(&state.rooms), vec![TEAM_ROOM_ID, "r2"]);
        assert!(apply_event(
            &mut state,
            &Event::RoomUpdated(room("r1", &["b1"], "2026-10-25T09:00:00.000Z"))
        ));
        assert!(apply_event(
            &mut state,
            &Event::RoomUpdated(Room {
                name: "Growth".to_owned(),
                ..growth
            })
        ));
        assert_eq!(room_names(&state.rooms), vec!["Everyone", "r1", "Growth"]);
        start_loading(&mut state, "r1");
        assert!(apply_event(
            &mut state,
            &Event::RoomDeleted(Id {
                id: "r1".to_owned()
            })
        ));
        assert_eq!(room_ids(&state.rooms), vec![TEAM_ROOM_ID, "r2"]);
        assert!(!state.conversations.contains_key("r1"));
        connected(&mut state, snapshot(vec![], vec![], vec![everyone(&[])]));
        assert_eq!(room_ids(&state.rooms), vec![TEAM_ROOM_ID]);
    }

    #[test]
    fn takes_a_deleted_bot_out_of_every_room() {
        let bots = vec![
            bot("b1", "2026-09-27T09:00:00.000Z"),
            bot("b2", "2026-09-27T10:00:00.000Z"),
        ];
        let rooms = vec![
            everyone(&["b1", "b2"]),
            room("r1", &["b1", "b2"], "2026-10-25T09:00:00.000Z"),
            room("r2", &["b2"], "2026-10-25T10:00:00.000Z"),
        ];
        let mut state = AppState::default();
        connected(&mut state, snapshot(bots, vec![], rooms));
        apply_event(
            &mut state,
            &Event::BotDeleted(Id {
                id: "b1".to_owned(),
            }),
        );
        let members: Vec<Vec<String>> = state
            .rooms
            .iter()
            .map(|room| room.bot_ids.clone())
            .collect();
        assert_eq!(
            members,
            vec![strings(&["b2"]), strings(&["b2"]), strings(&["b2"])]
        );
    }

    #[test]
    fn has_every_bot_in_everyone_even_one_made_after_the_rooms_were_listed_and_only_its_members_in_another_room()
     {
        let mut state = AppState::default();
        connected(
            &mut state,
            snapshot(
                vec![bot("b1", "2026-09-27T09:00:00.000Z")],
                vec![],
                vec![
                    everyone(&["b1"]),
                    room("r1", &["b1"], "2026-10-25T09:00:00.000Z"),
                ],
            ),
        );
        // The new bot comes with no room.updated: the daemon sends those for the other rooms only.
        apply_event(
            &mut state,
            &Event::BotUpdated(bot("b2", "2026-09-27T10:00:00.000Z")),
        );
        let (all, growth) = (&state.rooms[0], &state.rooms[1]);
        let in_all: Vec<&str> = room_bots(all, &state.bots)
            .into_iter()
            .map(|bot| bot.id.as_str())
            .collect();
        let in_growth: Vec<&str> = room_bots(growth, &state.bots)
            .into_iter()
            .map(|bot| bot.id.as_str())
            .collect();
        assert_eq!(in_all, vec!["b1", "b2"]);
        assert_eq!(in_growth, vec!["b1"]);
    }

    #[test]
    fn applies_new_updated_and_streaming_messages_to_an_open_chat_and_ignores_chats_that_arent_open()
     {
        let mut state = ready(vec![Message {
            body: "Hi".to_owned(),
            ..message("m1")
        }]);
        apply_event(
            &mut state,
            &Event::from(added(Message {
                status: MessageStatus::Streaming,
                ..message("m2")
            })),
        );
        apply_event(&mut state, &Event::from(delta("m2", "Wor")));
        apply_event(&mut state, &Event::from(delta("m2", "king")));
        assert_eq!(bodies(&state, "b1"), vec!["Hi", "Working"]);
        apply_event(
            &mut state,
            &Event::from(updated(Message {
                body: "Working on it.".to_owned(),
                ..message("m2")
            })),
        );
        let reply = &chat(&state, "b1").messages[1];
        assert_eq!(
            (reply.body.as_str(), reply.status),
            ("Working on it.", MessageStatus::Done)
        );
        let before = state.clone();
        let other = apply_event(
            &mut state,
            &Event::from(added(Message {
                conversation_id: "b9".to_owned(),
                ..message("x1")
            })),
        );
        assert!(!other);
        assert_eq!(state, before);
    }

    #[test]
    fn rebuilds_a_reply_that_was_streaming_while_its_chat_opened_opened_mid_stream() {
        let mut state = AppState::default();
        start_loading(&mut state, "b1");
        apply_event(&mut state, &Event::from(delta("m1", "Hel")));
        apply_event(&mut state, &Event::from(delta("m1", "lo")));
        finish_loading(
            &mut state,
            "b1",
            Page {
                messages: vec![Message {
                    status: MessageStatus::Streaming,
                    ..message("m1")
                }],
                has_more: false,
            },
        );
        assert_eq!(bodies(&state, "b1"), vec!["Hello"]);
        apply_event(&mut state, &Event::from(delta("m1", " there")));
        assert_eq!(bodies(&state, "b1"), vec!["Hello there"]);
        apply_event(
            &mut state,
            &Event::from(updated(Message {
                status: MessageStatus::Done,
                body: "Hello there!".to_owned(),
                ..message("m1")
            })),
        );
        let reply = &chat(&state, "b1").messages[0];
        assert_eq!(
            (reply.status, reply.body.as_str()),
            (MessageStatus::Done, "Hello there!")
        );
    }

    #[test]
    fn shows_a_message_that_arrived_while_its_chat_was_loading_exactly_once() {
        let mut state = AppState::default();
        start_loading(&mut state, "b1");
        apply_event(
            &mut state,
            &Event::from(added(Message {
                body: "New".to_owned(),
                ..message("m2")
            })),
        );
        finish_loading(
            &mut state,
            "b1",
            Page {
                messages: vec![
                    message("m1"),
                    Message {
                        body: "New".to_owned(),
                        ..message("m2")
                    },
                ],
                has_more: true,
            },
        );
        assert_eq!(message_ids(&state, "b1"), vec!["m1", "m2"]);
        assert!(chat(&state, "b1").has_more);
    }

    #[test]
    fn adds_older_pages_above_without_duplicates() {
        let mut state = ready(vec![message("m3"), message("m4")]);
        add_older(
            &mut state,
            "b1",
            Page {
                messages: vec![message("m1"), message("m2"), message("m3")],
                has_more: false,
            },
        );
        assert_eq!(message_ids(&state, "b1"), vec!["m1", "m2", "m3", "m4"]);
    }

    #[test]
    fn keeps_loaded_chats_on_a_new_connection_so_nothing_blanks_and_forgets_ones_still_loading_so_their_screens_load_them_again()
     {
        let mut state = ready(vec![message("m1"), message("m2")]);
        start_loading(&mut state, "b2");
        state.connection = Connection::Reconnecting;
        connected(&mut state, snapshot(vec![], vec![], vec![]));
        assert_eq!(state.connection, Connection::Ready);
        assert_eq!(message_ids(&state, "b1"), vec!["m1", "m2"]);
        assert!(!state.conversations.contains_key("b2"));
    }

    #[test]
    fn lists_the_routine_failures_waiting_to_be_shown_on_each_connection() {
        let failure = PendingNotification::RoutineFailed {
            bot_id: "b1".to_owned(),
            routine_id: "r1".to_owned(),
            run_id: "u1".to_owned(),
            title: "Morning check failed".to_owned(),
            body: "It ran out of time.".to_owned(),
        };
        assert!(AppState::default().notifications.is_empty());
        let mut state = AppState::default();
        connected(
            &mut state,
            Snapshot {
                notifications: Some(vec![failure.clone()]),
                ..snapshot(vec![], vec![], vec![])
            },
        );
        assert_eq!(state.notifications, vec![failure]);
        connected(&mut state, snapshot(vec![], vec![], vec![]));
        assert!(state.notifications.is_empty());
    }

    #[test]
    fn keeps_each_routines_newest_run_event_so_an_open_routine_editor_can_fetch_its_runs_again() {
        let mut state = AppState::default();
        apply_event(&mut state, &run("u1", "r1", RoutineRunStatus::Running));
        let running = state.latest_runs["r1"].clone();
        assert_eq!(
            (running.id.as_str(), running.status),
            ("u1", RoutineRunStatus::Running)
        );
        apply_event(&mut state, &run("u2", "r2", RoutineRunStatus::Queued));
        // Another routine's run leaves this one alone.
        assert_eq!(state.latest_runs.get("r1"), Some(&running));
        apply_event(&mut state, &run("u1", "r1", RoutineRunStatus::Done));
        let newest = &state.latest_runs["r1"];
        assert_eq!(
            (newest.id.as_str(), newest.status),
            ("u1", RoutineRunStatus::Done)
        );
        connected(&mut state, snapshot(vec![], vec![], vec![]));
        assert!(state.latest_runs.is_empty());
    }

    #[test]
    fn records_ended_sign_in_attempts_so_an_old_attempt_ending_never_reads_as_the_new_one_ending() {
        let mut state = AppState::default();
        assert!(state.sign_ins_ended.is_empty());
        apply_event(&mut state, &ended(Provider::Codex, "a"));
        // A card signing in as "b" keeps waiting.
        assert!(!state.sign_ins_ended.iter().any(|attempt| attempt == "b"));
        apply_event(&mut state, &ended(Provider::Codex, "b"));
        assert_eq!(state.sign_ins_ended, vec!["a", "b"]);
        // A new connection keeps them, so an attempt that ended before it still reads as ended.
        connected(&mut state, snapshot(vec![], vec![], vec![]));
        assert_eq!(state.sign_ins_ended, vec!["a", "b"]);
        for i in 0..30 {
            apply_event(&mut state, &ended(Provider::Claude, &format!("n{i}")));
        }
        assert_eq!(state.sign_ins_ended.len(), 20);
        assert_eq!(state.sign_ins_ended.last().map(String::as_str), Some("n29"));
    }

    // describe("catching up after a reconnect")

    #[test]
    fn replaces_from_the_newest_pages_oldest_message_onward_when_that_message_is_loaded_keeping_the_older_pages()
     {
        let mut state = ready(vec![
            message("m1"),
            message("m2"),
            Message {
                body: "Working".to_owned(),
                ..message("m3")
            },
        ]);
        catch_up(
            &mut state,
            "b1",
            Page {
                messages: vec![
                    message("m2"),
                    Message {
                        body: "Done.".to_owned(),
                        ..message("m3")
                    },
                    message("m4"),
                ],
                has_more: true,
            },
            &[],
        );
        assert_eq!(
            id_body(&state, "b1"),
            vec![("m1", ""), ("m2", ""), ("m3", "Done."), ("m4", "")]
        );
        // The older pages were already loaded.
        assert!(!chat(&state, "b1").has_more);
    }

    #[test]
    fn replaces_the_chat_with_the_newest_page_with_more_to_load_when_more_than_a_page_arrived_while_away()
     {
        let mut state = ready(vec![message("m1"), message("m2")]);
        catch_up(
            &mut state,
            "b1",
            Page {
                messages: vec![message("m5"), message("m6")],
                has_more: true,
            },
            &[],
        );
        assert_eq!(message_ids(&state, "b1"), vec!["m5", "m6"]);
        assert!(chat(&state, "b1").has_more);
    }

    #[test]
    fn replays_what_arrived_while_the_page_was_being_fetched_so_a_new_message_and_its_streamed_text_survive()
     {
        let during = vec![
            added(Message {
                status: MessageStatus::Streaming,
                ..message("m2")
            }),
            delta("m2", "Hel"),
            delta("m2", "lo"),
        ];
        // Shown live meanwhile.
        let mut state = ready(vec![message("m1")]);
        for event in &during {
            apply_event(&mut state, &Event::from(event.clone()));
        }
        // The page was read before m2 started.
        catch_up(
            &mut state,
            "b1",
            Page {
                messages: vec![message("m1")],
                has_more: false,
            },
            &during,
        );
        assert_eq!(id_body(&state, "b1"), vec![("m1", ""), ("m2", "Hello")]);
    }

    #[test]
    fn leaves_a_chat_that_isnt_loaded_alone() {
        let mut state = AppState::default();
        start_loading(&mut state, "b1");
        let before = state.clone();
        assert!(!catch_up(
            &mut state,
            "b1",
            Page {
                messages: vec![message("m1")],
                has_more: false,
            },
            &[],
        ));
        assert_eq!(state, before);
        let mut initial = AppState::default();
        assert!(!catch_up(
            &mut initial,
            "b9",
            Page {
                messages: vec![],
                has_more: false,
            },
            &[],
        ));
        assert_eq!(initial, AppState::default());
    }

    /// The state half of the TypeScript test. The other half calls `loadConversation` twice (connection.ts, ported by the
    /// connection task): a failed load is kept until a new connection, and only then is the chat's history fetched again.
    #[test]
    fn keeps_a_failed_history_load_until_a_new_connection_clears_it() {
        let mut state = AppState::default();
        state.conversations.insert(
            "b1".to_owned(),
            Conversation {
                status: ConversationStatus::Failed,
                messages: vec![],
                has_more: false,
                pending: vec![],
            },
        );
        connected(&mut state, snapshot(vec![], vec![], vec![]));
        assert!(!state.conversations.contains_key("b1"));
    }

    // Decisions the TypeScript doesn't test.

    #[test]
    fn the_flag_says_whether_the_content_changed_for_every_event() {
        let mut state = ready(vec![message("m1")]);
        connected(
            &mut state,
            snapshot(vec![bot("b1", "2026-09-27T09:00:00.000Z")], vec![], vec![]),
        );
        let cases: Vec<(Event, bool)> = vec![
            (
                Event::BotUpdated(bot("b1", "2026-09-27T09:00:00.000Z")),
                false,
            ),
            (
                Event::BotUpdated(Bot {
                    state: BotState::Working,
                    ..bot("b1", "2026-09-27T09:00:00.000Z")
                }),
                true,
            ),
            (
                Event::BotDeleted(Id {
                    id: "nobody".to_owned(),
                }),
                false,
            ),
            (Event::MessageAdded(message("m1")), false),
            (Event::from(delta("m1", "")), false),
            (Event::from(delta("m1", "x")), true),
            (Event::from(updated(message("m9"))), false),
            (
                Event::from(added(Message {
                    conversation_id: "b9".to_owned(),
                    ..message("x")
                })),
                false,
            ),
            (Event::SettingsChanged(settings()), false),
            (Event::DevicesChanged(Empty::default()), false),
        ];
        for (event, expected) in &cases {
            let before = state.clone();
            let changed = apply_event(&mut state, event);
            assert_eq!(changed, *expected, "{event:?}");
            assert_eq!(state != before, *expected, "{event:?}");
        }
    }

    #[test]
    fn an_updated_bot_moves_behind_bots_created_at_the_same_time_as_the_javascript_sort_does() {
        let mut state = AppState::default();
        connected(
            &mut state,
            snapshot(
                vec![
                    bot("b1", "2026-09-27T09:00:00.000Z"),
                    bot("b2", "2026-09-27T09:00:00.000Z"),
                ],
                vec![],
                vec![],
            ),
        );
        apply_event(
            &mut state,
            &Event::BotUpdated(bot("b1", "2026-09-27T09:00:00.000Z")),
        );
        assert_eq!(bot_ids(&state.bots), vec!["b2", "b1"]);
    }

    #[test]
    fn forget_conversation_drops_a_chat_and_says_whether_it_was_there() {
        let mut state = ready(vec![message("m1")]);
        assert!(forget_conversation(&mut state, "b1"));
        assert!(!forget_conversation(&mut state, "b1"));
        assert!(state.conversations.is_empty());
    }

    #[test]
    fn only_a_loaded_chat_takes_older_pages_and_only_a_loading_one_finishes() {
        let mut loading = AppState::default();
        start_loading(&mut loading, "b1");
        let before = loading.clone();
        assert!(!add_older(
            &mut loading,
            "b1",
            Page {
                messages: vec![message("m0")],
                has_more: true,
            },
        ));
        assert_eq!(loading, before);
        assert!(!finish_loading(
            &mut AppState::default(),
            "b1",
            Page {
                messages: vec![message("m0")],
                has_more: true,
            },
        ));
    }
}
