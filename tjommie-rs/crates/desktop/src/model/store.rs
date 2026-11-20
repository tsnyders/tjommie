//! Port of `packages/ui/lib/store.ts`: the app state, kept in one place and told to its subscribers after each change. The
//! TypeScript's `getState` and `update` are `state` and `update` here; `useApp` is React's hook and is left out, since the
//! screens read the store through GPUI. Subscribers are told only when a change changed the state's content.

use std::sync::Arc;

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
    /// Shared, so a snapshot is a pointer: a change copies the state only while a snapshot is still held.
    // ponytail: the live app always holds one (the UI's store keeps the last frame's), so every change copies the whole state
    // once; the connection applies a burst of events as one change (`apply_all`), so that is once per burst, not per event.
    // A persistent structure for the conversations if a long chat's copy ever shows in a profile (M7).
    state: Arc<AppState>,
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

    /// The state as it is now, for something that must outlive this borrow: a pointer to it, not a copy. Later changes don't
    /// reach it.
    pub fn snapshot(&self) -> Arc<AppState> {
        Arc::clone(&self.state)
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    /// Applies one protocol event (`applyEvent` through `update`), then tells the event listeners about it.
    pub fn apply(&mut self, event: &Event) {
        self.apply_all(std::slice::from_ref(event));
    }

    /// Applies events in order as ONE change: one copy of the state while a snapshot is held, and the subscribers told once,
    /// with the state after the last. Then each event listener hears each event, in order.
    pub fn apply_all(&mut self, events: &[Event]) {
        self.update(|state| {
            events
                .iter()
                .fold(false, |changed, event| apply_event(state, event) | changed)
        });
        for event in events {
            for (_, listener) in &mut self.event_listeners {
                listener(event);
            }
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
        let changed = match Arc::get_mut(&mut self.state) {
            // Nobody holds a snapshot: the change is made in place.
            Some(state) => change(state),
            // One is held (the screens' last frame): the change is made on a copy, which replaces the state only if it changed
            // something, so a held snapshot is never changed and an unchanged state keeps its pointer.
            None => {
                let mut next = AppState::clone(&self.state);
                let changed = change(&mut next);
                if changed {
                    self.state = Arc::new(next);
                }
                changed
            }
        };
        if !changed {
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
        assert_eq!(*before, AppState::default());
        assert_eq!(
            store.state().settings.as_ref().map(|s| s.chain_limit),
            Some(5)
        );
    }

    /// M5 review M5, as the host-wiring review M1 corrected it: a snapshot costs nothing, a change copies the state when it
    /// meets a snapshot that is still held (which in the live app is always: the UI keeps the last frame's), and leaves that
    /// snapshot as it was; with none held the change is made in place.
    #[test]
    fn a_change_copies_the_state_while_a_snapshot_is_held_and_leaves_that_snapshot_alone() {
        let mut store = AppStore::new();
        store.apply(&chain(5));
        let (first, second) = (store.snapshot(), store.snapshot());
        assert!(
            Arc::ptr_eq(&first, &second),
            "taking a snapshot copied the state"
        );
        store.apply(&chain(6));
        let after = store.snapshot();
        assert!(
            !Arc::ptr_eq(&first, &after),
            "a held snapshot was changed under its holder"
        );
        assert_eq!(first.settings.as_ref().map(|s| s.chain_limit), Some(5));
        assert_eq!(after.settings.as_ref().map(|s| s.chain_limit), Some(6));
        // With no snapshot held, the change is made in place: no copy, the same allocation.
        drop((first, second, after));
        let before = store.state() as *const AppState;
        store.apply(&chain(7));
        assert_eq!(
            store.state() as *const AppState,
            before,
            "the change copied the state"
        );
    }

    /// A batch is one change: one copy while a snapshot is held, one notification with the state after the last event, and the
    /// event listeners still hear every event, in order.
    #[test]
    fn a_batch_of_events_is_one_change_and_every_event_is_still_heard() {
        let (log, heard) = (
            Arc::new(Mutex::new(Vec::new())),
            Arc::new(Mutex::new(Vec::new())),
        );
        let mut store = AppStore::new();
        store.subscribe(recording(&log));
        let record = Arc::clone(&heard);
        store.subscribe_events(move |event| {
            if let Event::SettingsChanged(settings) = event {
                record
                    .lock()
                    .expect("the log isn't poisoned")
                    .push(settings.chain_limit);
            }
        });
        let held = store.snapshot();
        store.apply_all(&[chain(5), chain(6), chain(7)]);
        assert_eq!(store.version(), 1, "three events were three changes");
        assert!(!Arc::ptr_eq(&held, &store.snapshot()));
        assert_eq!(*log.lock().expect("the log isn't poisoned"), vec![7]);
        assert_eq!(
            *heard.lock().expect("the log isn't poisoned"),
            vec![5, 6, 7]
        );
        // Events that change nothing are not a change.
        let unchanged = store.snapshot();
        store.apply_all(&[chain(7), chain(7)]);
        assert_eq!(store.version(), 1);
        assert!(Arc::ptr_eq(&unchanged, &store.snapshot()));
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
