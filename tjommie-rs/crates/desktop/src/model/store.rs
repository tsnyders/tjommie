//! Port of `packages/ui/lib/store.ts`: the app state, kept in one place and told to its subscribers after each change. The
//! TypeScript's `getState` and `update` are `state` and `update` here; `useApp` is React's hook and is left out, since the
//! screens read the store through GPUI. Subscribers are told only when a change changed the state's content.

use protocol::Event;

use super::state::{AppState, apply_event};

/// Identifies one subscription, for `unsubscribe`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubscriptionId(u64);

type Listener = Box<dyn FnMut(&AppState) + Send>;
type EventListener = Box<dyn FnMut(&Event) + Send>;

/// The state and its subscribers. A listener is `Send`, so the store can move between threads.
#[derive(Default)]
pub struct AppStore {
    state: AppState,
    /// Goes up by one for each change the subscribers were told about.
    version: u64,
    next_id: u64,
    listeners: Vec<(SubscriptionId, Listener)>,
    event_listeners: Vec<(SubscriptionId, EventListener)>,
}

impl AppStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// `getState()`: the state as it is now.
    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// A copy of the state, for something that must outlive this borrow.
    pub fn snapshot(&self) -> AppState {
        self.state.clone()
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// Applies one protocol event (`applyEvent` through `update`), then tells the event listeners about it.
    pub fn apply(&mut self, event: &Event) {
        self.update(|state| apply_event(state, event));
        for (_, listener) in &mut self.event_listeners {
            listener(event);
        }
    }

    /// Told each event as it is applied, whether or not it changed the state: for what the state doesn't keep (projects,
    /// devices, usage), which a screen reads again when its event arrives. The id is what `unsubscribe` takes.
    pub fn subscribe_events(
        &mut self,
        listener: impl FnMut(&Event) + Send + 'static,
    ) -> SubscriptionId {
        let id = SubscriptionId(self.next_id);
        self.next_id += 1;
        let listener: EventListener = Box::new(listener);
        self.event_listeners.push((id, listener));
        id
    }

    /// `update(change)`: `change` edits the state in place and returns whether its content changed. Subscribers are told,
    /// with the new state, only when it did.
    pub fn update(&mut self, change: impl FnOnce(&mut AppState) -> bool) {
        if !change(&mut self.state) {
            return;
        }
        self.version += 1;
        for (_, listener) in &mut self.listeners {
            listener(&self.state);
        }
    }

    /// `subscribe(listener)`: the listener is told the state after each change, in the order subscribed. The id is what
    /// `unsubscribe` takes.
    pub fn subscribe(
        &mut self,
        listener: impl FnMut(&AppState) + Send + 'static,
    ) -> SubscriptionId {
        let id = SubscriptionId(self.next_id);
        self.next_id += 1;
        let listener: Listener = Box::new(listener);
        self.listeners.push((id, listener));
        id
    }

    /// The function `subscribe` returned in the TypeScript. Says whether the subscription was there.
    pub fn unsubscribe(&mut self, id: SubscriptionId) -> bool {
        let before = self.listeners.len() + self.event_listeners.len();
        self.listeners.retain(|(listener_id, _)| *listener_id != id);
        self.event_listeners
            .retain(|(listener_id, _)| *listener_id != id);
        self.listeners.len() + self.event_listeners.len() != before
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// An event listener hears every event, even one the state doesn't keep, until it unsubscribes.
    #[test]
    fn event_listeners_hear_every_event_until_they_unsubscribe() {
        let heard = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&heard);
        let mut store = AppStore::new();
        let id = store.subscribe_events(move |event| {
            if matches!(event, Event::DevicesChanged(_)) {
                counted.fetch_add(1, Ordering::SeqCst);
            }
        });
        store.apply(&Event::DevicesChanged(Empty::default()));
        assert_eq!(heard.load(Ordering::SeqCst), 1);
        assert_eq!(store.version(), 0, "the state didn't change");
        assert!(store.unsubscribe(id));
        store.apply(&Event::DevicesChanged(Empty::default()));
        assert_eq!(heard.load(Ordering::SeqCst), 1);
    }

    use protocol::model::Settings;
    use protocol::wire::{Empty, MessageDelta};

    use super::*;

    /// A settings change: the chain limit is what the state shows.
    fn chain(limit: u32) -> Event {
        Event::SettingsChanged(Settings {
            chain_limit: limit,
            relay_url: None,
        })
    }

    /// A listener that records the chain limit of each state it is told about.
    fn recording(log: &Arc<Mutex<Vec<u32>>>) -> impl FnMut(&AppState) + Send + 'static {
        let log = Arc::clone(log);
        move |state| {
            if let Some(settings) = &state.settings {
                log.lock()
                    .expect("the log isn't poisoned")
                    .push(settings.chain_limit);
            }
        }
    }

    /// A listener that counts how often it is told.
    fn counting(count: &Arc<AtomicUsize>) -> impl FnMut(&AppState) + Send + 'static {
        let count = Arc::clone(count);
        move |_| {
            count.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn subscribers_are_told_after_every_change_with_the_new_state() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut store = AppStore::new();
        store.subscribe(recording(&log));
        store.apply(&chain(5));
        store.apply(&chain(6));
        assert_eq!(*log.lock().expect("the log isn't poisoned"), vec![5, 6]);
    }

    #[test]
    fn subscribers_are_not_told_when_an_event_changes_nothing() {
        let count = Arc::new(AtomicUsize::new(0));
        let mut store = AppStore::new();
        store.subscribe(counting(&count));
        store.apply(&chain(5));
        // The same settings again: nothing changes.
        store.apply(&chain(5));
        // An event the state doesn't keep.
        store.apply(&Event::DevicesChanged(Empty::default()));
        // A message for a chat that isn't open.
        store.apply(&Event::MessageDelta(MessageDelta {
            id: "m1".to_owned(),
            conversation_id: "b9".to_owned(),
            delta: "x".to_owned(),
        }));
        // A change that reports that it changed nothing.
        store.update(|_| false);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert_eq!(store.version(), 1);
    }

    #[test]
    fn unsubscribing_stops_telling_that_subscriber_and_only_that_one() {
        let first = Arc::new(AtomicUsize::new(0));
        let second = Arc::new(AtomicUsize::new(0));
        let mut store = AppStore::new();
        let first_id = store.subscribe(counting(&first));
        store.subscribe(counting(&second));
        store.apply(&chain(5));
        assert!(store.unsubscribe(first_id));
        // Already gone.
        assert!(!store.unsubscribe(first_id));
        store.apply(&chain(6));
        assert_eq!(first.load(Ordering::SeqCst), 1);
        assert_eq!(second.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn update_tells_subscribers_with_the_state_its_change_made() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut store = AppStore::new();
        store.subscribe(recording(&log));
        store.update(|state| {
            state.settings = Some(Settings {
                chain_limit: 9,
                relay_url: None,
            });
            true
        });
        assert_eq!(*log.lock().expect("the log isn't poisoned"), vec![9]);
        assert_eq!(
            store.state().settings.as_ref().map(|s| s.chain_limit),
            Some(9)
        );
    }

    #[test]
    fn a_snapshot_is_a_copy_that_later_changes_do_not_reach() {
        let mut store = AppStore::new();
        let before = store.snapshot();
        store.apply(&chain(5));
        assert_eq!(before, AppState::default());
        assert_eq!(
            store.state().settings.as_ref().map(|s| s.chain_limit),
            Some(5)
        );
    }

    #[test]
    fn subscribers_are_told_in_the_order_they_subscribed() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut store = AppStore::new();
        for name in ["first", "second"] {
            let log = Arc::clone(&log);
            store.subscribe(move |_| {
                log.lock().expect("the log isn't poisoned").push(name);
            });
        }
        store.apply(&chain(5));
        assert_eq!(
            *log.lock().expect("the log isn't poisoned"),
            vec!["first", "second"]
        );
    }
}
