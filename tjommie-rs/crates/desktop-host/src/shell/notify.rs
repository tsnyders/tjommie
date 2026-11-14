//! Toast notifications (T6.2; port of `notification-delivery.ts` and `main.ts:401-449`).
//!
//! Three layers, so the rules can be tested without a desktop:
//! - `Delivery` decides when a notice is shown and when the daemon is told it was (`notifications.ack`);
//! - `Notifier` skips a toast while the window is visible and focused, and says whether the OS took one;
//! - `WinRtToasts` is the real `ToastNotificationManager` behind the `ToastBackend` trait.
//!
//! A routine failure stays pending in the daemon until it is acknowledged, and only an OS-backed `Show()` with
//! notifications enabled permits that: otherwise it is shown again on the next start.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, PoisonError};

use futures_util::future::{BoxFuture, FutureExt, Shared};
use protocol::wire::{Notify, PendingNotification};
use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{
    NotificationSetting, ToastNotification, ToastNotificationManager, ToastNotifier,
};
use windows::core::HSTRING;

use super::registry::Registry;
use crate::identity::{Identity, Kind};

/// `encodeURIComponent`: everything but letters, digits and `-_.!~*'()` is percent-encoded as UTF-8.
fn encode_uri_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

struct Parts<'a> {
    title: &'a str,
    body: &'a str,
    bot_id: &'a str,
    room_id: Option<&'a str>,
    run_id: Option<&'a str>,
}

fn parts(notice: &Notify) -> Parts<'_> {
    match notice {
        Notify::Approval {
            bot_id,
            title,
            body,
            ..
        }
        | Notify::Limit {
            bot_id,
            title,
            body,
        } => Parts {
            title,
            body,
            bot_id,
            room_id: None,
            run_id: None,
        },
        Notify::RoutineFailed {
            bot_id,
            title,
            body,
            run_id,
            ..
        } => Parts {
            title,
            body,
            bot_id,
            room_id: None,
            run_id: run_id.as_deref(),
        },
        Notify::Done {
            bot_id,
            title,
            body,
            room_id,
        }
        | Notify::Failed {
            bot_id,
            title,
            body,
            room_id,
        } => Parts {
            title,
            body,
            bot_id,
            room_id: room_id.as_deref(),
            run_id: None,
        },
    }
}

/// Where a click on the toast goes: the room, else the agent's chat (`notificationRoute`).
pub fn notification_route(notice: &Notify) -> String {
    let parts = parts(notice);
    match parts.room_id {
        Some(room) if !room.is_empty() => format!("/room/?id={}", encode_uri_component(room)),
        _ => format!("/chat/?bot={}", encode_uri_component(parts.bot_id)),
    }
}

/// A notice the daemon kept because nothing acknowledged it, as a live notice (`notifications.pending`).
pub fn notice_from_pending(pending: PendingNotification) -> Notify {
    match pending {
        PendingNotification::RoutineFailed {
            bot_id,
            routine_id,
            run_id,
            title,
            body,
        } => Notify::RoutineFailed {
            bot_id,
            title,
            body,
            routine_id: Some(routine_id),
            run_id: Some(run_id),
        },
    }
}

type DeliveryResult = Result<(), String>;
/// A delivery in flight, shared by everyone who asks for it, with the number that tells it from a later one.
type InFlight = (u64, Shared<BoxFuture<'static, DeliveryResult>>);

#[derive(Default)]
struct DeliveryState {
    /// Run ids already shown in this process, so a retried acknowledgement doesn't show the toast twice.
    shown: Mutex<HashSet<String>>,
    /// Deliveries in flight, keyed by run id: a live event and the startup replay of the same failure are one delivery.
    pending: Mutex<HashMap<String, InFlight>>,
    next: AtomicU64,
}

/// `notificationDelivery()`: retains delivery identities across notifier reconnects.
#[derive(Clone, Default)]
pub struct Delivery {
    state: Arc<DeliveryState>,
}

impl Delivery {
    /// Shows `notice` with `show` and, for a routine failure with a run id, acknowledges it with `acknowledge` once
    /// shown. `show` returning false (failed, unsupported, notifications off) leaves it pending. A failed
    /// acknowledgement is an error here and is retried by the next delivery without showing the toast again.
    pub async fn deliver<S, SF, A, AF>(
        &self,
        notice: Notify,
        show: S,
        acknowledge: A,
    ) -> Result<(), String>
    where
        S: FnOnce(Notify) -> SF + Send + 'static,
        SF: Future<Output = bool> + Send + 'static,
        A: FnOnce(String) -> AF + Send + 'static,
        AF: Future<Output = Result<(), String>> + Send + 'static,
    {
        let Some(run_id) = parts(&notice).run_id.map(str::to_owned) else {
            show(notice).await;
            return Ok(());
        };
        let (id, delivery) = {
            let mut pending = self
                .state
                .pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if let Some((id, active)) = pending.get(&run_id) {
                (*id, active.clone())
            } else {
                let state = Arc::clone(&self.state);
                let key = run_id.clone();
                let future: BoxFuture<'static, DeliveryResult> = async move {
                    let already = state
                        .shown
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .contains(&key);
                    if !already {
                        if !show(notice).await {
                            return Ok(());
                        }
                        state
                            .shown
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner)
                            .insert(key.clone());
                    }
                    acknowledge(key).await
                }
                .boxed();
                let shared = future.shared();
                let id = self.state.next.fetch_add(1, Ordering::SeqCst);
                pending.insert(run_id.clone(), (id, shared.clone()));
                (id, shared)
            }
        };
        let result = delivery.await;
        let mut pending = self
            .state
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if pending
            .get(&run_id)
            .is_some_and(|(current, _)| *current == id)
        {
            pending.remove(&run_id);
        }
        result
    }
}

/// One toast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toast {
    pub title: String,
    pub body: String,
    /// Where a click goes (`notification_route`); `None` for a toast that only informs.
    pub route: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToastError {
    /// The user or policy turned notifications off for the app.
    Disabled(String),
    Failed(String),
}

pub trait ToastBackend: Send + Sync {
    /// Asks the OS to show the toast. `Ok` only when notifications are enabled and the OS accepted it.
    fn show(&self, toast: &Toast) -> Result<(), ToastError>;
}

/// Whether the app's own window is up front, in which case the notice is already in the app's history.
pub trait WindowState: Send + Sync {
    fn visible_and_focused(&self) -> bool;
}

pub struct Notifier {
    backend: Arc<dyn ToastBackend>,
    window: Arc<dyn WindowState>,
}

impl Notifier {
    pub fn new(backend: Arc<dyn ToastBackend>, window: Arc<dyn WindowState>) -> Notifier {
        Notifier { backend, window }
    }

    /// `showNotice`: true when the notice may be acknowledged, because it was shown or didn't need to be.
    pub fn show_notice(&self, notice: &Notify) -> bool {
        if self.window.visible_and_focused() {
            return true;
        }
        let parts = parts(notice);
        self.show(&Toast {
            title: parts.title.into(),
            body: parts.body.into(),
            route: Some(notification_route(notice)),
        })
    }

    /// A toast that only informs, such as the first-close "Tjommie is still running". True when the OS took it.
    pub fn show_plain(&self, title: &str, body: &str) -> bool {
        self.show(&Toast {
            title: title.into(),
            body: body.into(),
            route: None,
        })
    }

    fn show(&self, toast: &Toast) -> bool {
        match self.backend.show(toast) {
            Ok(()) => true,
            Err(ToastError::Disabled(why)) => {
                log::info!("A notification was not shown because notifications are off: {why}");
                false
            }
            Err(ToastError::Failed(why)) => {
                log::warn!("A notification could not be shown: {why}");
                false
            }
        }
    }
}

/// How the app hands notices to the user: shown through a `Notifier`, acknowledged through the daemon.
pub type Acknowledge = Arc<dyn Fn(String) -> BoxFuture<'static, Result<(), String>> + Send + Sync>;

pub struct Notifications {
    delivery: Delivery,
    notifier: Arc<Notifier>,
}

impl Notifications {
    pub fn new(notifier: Arc<Notifier>) -> Notifications {
        Notifications {
            delivery: Delivery::default(),
            notifier,
        }
    }

    /// A live `notify` event.
    pub async fn deliver(&self, notice: Notify, acknowledge: Acknowledge) -> Result<(), String> {
        let notifier = Arc::clone(&self.notifier);
        self.delivery
            .deliver(
                notice,
                move |notice| async move { notifier.show_notice(&notice) },
                move |run_id| acknowledge(run_id),
            )
            .await
    }

    /// `notifications.pending`, fetched after subscribing, so a failure that arrives during the fetch is one delivery.
    /// Stops at the first failure, as the TypeScript loop did: notifications then stay off until the next start.
    pub async fn replay(
        &self,
        pending: Vec<PendingNotification>,
        acknowledge: &Acknowledge,
    ) -> Result<(), String> {
        for notice in pending {
            self.deliver(notice_from_pending(notice), Arc::clone(acknowledge))
                .await?;
        }
        Ok(())
    }
}

/// XML text: the five markup characters escaped, and the control characters XML 1.0 can't hold dropped.
fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(c),
            c if (c as u32) < 0x20 || c == '\u{FFFE}' || c == '\u{FFFF}' => {}
            c => out.push(c),
        }
    }
    out
}

/// The toast's content. Titles and bodies come from agents and routines, so they are escaped, never trusted as markup.
pub fn toast_xml(toast: &Toast) -> String {
    let launch = toast
        .route
        .as_ref()
        .map(|route| format!(" launch=\"{}\"", xml_escape(route)))
        .unwrap_or_default();
    format!(
        "<toast{launch}><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
        xml_escape(&toast.title),
        xml_escape(&toast.body)
    )
}

const AUMID_KEY: &str = r"Software\Classes\AppUserModelId";

/// Windows shows a toast only for an app it knows by AppUserModelID, and `Setting()` fails ("Element not found") for
/// one it doesn't. The installers give production and update-test builds a Start-menu shortcut that carries the id
/// (M8). A dev or e2e build has no installer, so it registers its own id under the current user, with its own product
/// name. The production id is never written here, so the installed Tjommie's notification settings are untouched.
/// True when something was written.
pub fn register_aumid(registry: &dyn Registry, identity: &Identity) -> Result<bool, String> {
    if matches!(identity.kind, Kind::Production | Kind::UpdateTest) {
        return Ok(false);
    }
    registry
        .write_string(
            &format!(r"{AUMID_KEY}\{}", identity.app_id),
            "DisplayName",
            identity.product,
        )
        .map(|()| true)
}

/// The real toasts. A click on one sends its route to `clicks`; the app shows the window and goes there.
pub struct WinRtToasts {
    app_id: HSTRING,
    clicks: Sender<String>,
    /// Kept until dismissed, so their click handlers stay alive (Electron's `notifications` set).
    live: Arc<Mutex<HashMap<u64, ToastNotification>>>,
    next: AtomicU64,
}

/// At most this many toasts are kept alive for clicks; older ones just stop reacting.
const LIVE_TOASTS: usize = 32;

impl WinRtToasts {
    pub fn new(app_id: &str, clicks: Sender<String>) -> WinRtToasts {
        WinRtToasts {
            app_id: HSTRING::from(app_id),
            clicks,
            live: Arc::new(Mutex::new(HashMap::new())),
            next: AtomicU64::new(0),
        }
    }

    fn notifier(&self) -> Result<ToastNotifier, ToastError> {
        ToastNotificationManager::CreateToastNotifierWithId(&self.app_id)
            .map_err(|error| ToastError::Failed(format!("no notifier: {error}")))
    }
}

impl ToastBackend for WinRtToasts {
    fn show(&self, toast: &Toast) -> Result<(), ToastError> {
        let notifier = self.notifier()?;
        let failed = |error: windows::core::Error| ToastError::Failed(error.to_string());
        // Windows has no setting to read for an id it hasn't shown a toast for yet ("Element not found"), so the first
        // toast ever can only be checked after it is shown. Anything but Enabled means the user or policy turned
        // notifications off; an id still unknown afterwards was not registered, and nothing was displayed.
        let before = notifier.Setting();
        if let Ok(setting) = &before
            && *setting != NotificationSetting::Enabled
        {
            return Err(ToastError::Disabled(format!("{setting:?}")));
        }
        let document = XmlDocument::new().map_err(failed)?;
        document
            .LoadXml(&HSTRING::from(toast_xml(toast)))
            .map_err(failed)?;
        let notification = ToastNotification::CreateToastNotification(&document).map_err(failed)?;
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        if let Some(route) = toast.route.clone() {
            let clicks = self.clicks.clone();
            notification
                .Activated(&TypedEventHandler::new(move |_, _| {
                    let _ = clicks.send(route.clone());
                    Ok(())
                }))
                .map_err(failed)?;
        }
        let live = Arc::clone(&self.live);
        notification
            .Dismissed(&TypedEventHandler::new(move |_, _| {
                live.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .remove(&id);
                Ok(())
            }))
            .map_err(failed)?;
        notifier.Show(&notification).map_err(failed)?;
        match before.or_else(|_| notifier.Setting()) {
            Ok(NotificationSetting::Enabled) => {}
            Ok(setting) => return Err(ToastError::Disabled(format!("{setting:?}"))),
            Err(error) => {
                return Err(ToastError::Failed(format!(
                    "the notification id isn't registered with Windows: {error}"
                )));
            }
        }
        let mut kept = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        if kept.len() >= LIVE_TOASTS
            && let Some(oldest) = kept.keys().min().copied()
        {
            kept.remove(&oldest);
        }
        kept.insert(id, notification);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use tokio::sync::oneshot;

    use super::*;

    fn routine(run_id: &str) -> Notify {
        Notify::RoutineFailed {
            bot_id: "bot".into(),
            title: "Routine failed".into(),
            body: "Failure".into(),
            routine_id: Some("routine".into()),
            run_id: Some(run_id.into()),
        }
    }

    fn counter() -> Arc<AtomicUsize> {
        Arc::new(AtomicUsize::new(0))
    }

    fn acknowledging(
        count: &Arc<AtomicUsize>,
    ) -> impl FnOnce(String) -> BoxFuture<'static, Result<(), String>> + use<> {
        let count = Arc::clone(count);
        move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            async { Ok(()) }.boxed()
        }
    }

    // `notification-delivery.test.ts` "routes a room notice to the room and other notices to the bot chat"
    #[test]
    fn routes_a_room_notice_to_the_room_and_other_notices_to_the_agent_chat() {
        let done = Notify::Done {
            bot_id: "bot".into(),
            title: "Done".into(),
            body: String::new(),
            room_id: Some("room /?".into()),
        };
        assert_eq!(notification_route(&done), "/room/?id=room%20%2F%3F");
        assert_eq!(notification_route(&routine("run")), "/chat/?bot=bot");
        // An empty room id is no room.
        let empty = Notify::Failed {
            bot_id: "b ü".into(),
            title: String::new(),
            body: String::new(),
            room_id: Some(String::new()),
        };
        assert_eq!(notification_route(&empty), "/chat/?bot=b%20%C3%BC");
    }

    // "coalesces a live event and pending replay until acknowledgement completes"
    #[tokio::test]
    async fn coalesces_a_live_event_and_the_replay_until_acknowledgement_completes() {
        let delivery = Delivery::default();
        let shown = counter();
        let acked = counter();
        let (release, gate) = oneshot::channel::<()>();
        let gate = Arc::new(Mutex::new(Some(gate)));
        let ack_gate = Arc::clone(&gate);
        let ack_count = Arc::clone(&acked);
        let live = delivery.deliver(
            routine("run"),
            {
                let shown = Arc::clone(&shown);
                move |_| {
                    shown.fetch_add(1, Ordering::SeqCst);
                    async { true }
                }
            },
            move |_| {
                ack_count.fetch_add(1, Ordering::SeqCst);
                let gate = ack_gate.lock().unwrap().take();
                async move {
                    if let Some(gate) = gate {
                        let _ = gate.await;
                    }
                    Ok(())
                }
            },
        );
        let replay = delivery.deliver(
            routine("run"),
            |_| async { panic!("the replay must join the live delivery, not show again") },
            |_| async { panic!("nor acknowledge again") },
        );
        let release = async {
            tokio::task::yield_now().await;
            let _ = release.send(());
        };
        let (live, replay, ()) = tokio::join!(live, replay, release);
        live.unwrap();
        replay.unwrap();
        assert_eq!(shown.load(Ordering::SeqCst), 1);
        assert_eq!(acked.load(Ordering::SeqCst), 1);
    }

    // "retries acknowledgement after reconnect without repeating the popup"
    #[tokio::test]
    async fn retries_acknowledgement_without_repeating_the_popup() {
        let delivery = Delivery::default();
        let shown = counter();
        let show = |shown: &Arc<AtomicUsize>| {
            let shown = Arc::clone(shown);
            move |_| {
                shown.fetch_add(1, Ordering::SeqCst);
                async { true }
            }
        };
        let failed = delivery
            .deliver(routine("run"), show(&shown), |_| async {
                Err("Disconnected".to_string())
            })
            .await;
        assert_eq!(failed, Err("Disconnected".to_string()));
        let acked = counter();
        delivery
            .deliver(routine("run"), show(&shown), acknowledging(&acked))
            .await
            .unwrap();
        assert_eq!(shown.load(Ordering::SeqCst), 1);
        assert_eq!(acked.load(Ordering::SeqCst), 1);
    }

    // "keeps unsupported delivery pending and allows it to be retried later"
    #[tokio::test]
    async fn keeps_an_undelivered_notice_pending_and_allows_a_retry() {
        let delivery = Delivery::default();
        let acked = counter();
        delivery
            .deliver(routine("run"), |_| async { false }, acknowledging(&acked))
            .await
            .unwrap();
        assert_eq!(acked.load(Ordering::SeqCst), 0);
        let shown = counter();
        let shown_again = Arc::clone(&shown);
        delivery
            .deliver(
                routine("run"),
                move |_| {
                    shown_again.fetch_add(1, Ordering::SeqCst);
                    async { true }
                },
                acknowledging(&acked),
            )
            .await
            .unwrap();
        assert_eq!(shown.load(Ordering::SeqCst), 1);
        assert_eq!(acked.load(Ordering::SeqCst), 1);
    }

    // "waits for asynchronous OS confirmation and retries a failed display"
    #[tokio::test]
    async fn waits_for_the_os_and_retries_a_failed_display() {
        let delivery = Delivery::default();
        let acked = counter();
        let (confirm, os) = oneshot::channel::<bool>();
        let first = delivery.deliver(
            routine("run"),
            |_| async move { os.await.unwrap_or(false) },
            acknowledging(&acked),
        );
        let answer = async {
            tokio::task::yield_now().await;
            assert_eq!(
                acked.load(Ordering::SeqCst),
                0,
                "not acknowledged before the OS answers"
            );
            let _ = confirm.send(false);
        };
        let (first, ()) = tokio::join!(first, answer);
        first.unwrap();
        assert_eq!(
            acked.load(Ordering::SeqCst),
            0,
            "a failed display is not acknowledged"
        );
        delivery
            .deliver(routine("run"), |_| async { true }, acknowledging(&acked))
            .await
            .unwrap();
        assert_eq!(acked.load(Ordering::SeqCst), 1);
    }

    // "does not coalesce distinct failures or acknowledgement-free notices"
    #[tokio::test]
    async fn does_not_coalesce_distinct_failures_or_notices_without_a_run_id() {
        let delivery = Delivery::default();
        let shown = counter();
        let acked = counter();
        let show = |shown: &Arc<AtomicUsize>| {
            let shown = Arc::clone(shown);
            move |_| {
                shown.fetch_add(1, Ordering::SeqCst);
                async { true }
            }
        };
        delivery
            .deliver(routine("run"), show(&shown), acknowledging(&acked))
            .await
            .unwrap();
        delivery
            .deliver(routine("another"), show(&shown), acknowledging(&acked))
            .await
            .unwrap();
        let limit = Notify::Limit {
            bot_id: "bot".into(),
            title: "Limit".into(),
            body: "Wait".into(),
        };
        delivery
            .deliver(limit, show(&shown), acknowledging(&acked))
            .await
            .unwrap();
        assert_eq!(shown.load(Ordering::SeqCst), 3);
        assert_eq!(acked.load(Ordering::SeqCst), 2);
    }

    struct Window(bool);
    impl WindowState for Window {
        fn visible_and_focused(&self) -> bool {
            self.0
        }
    }

    struct Backend {
        result: Result<(), ToastError>,
        shown: Mutex<Vec<Toast>>,
    }
    impl Backend {
        fn new(result: Result<(), ToastError>) -> Arc<Backend> {
            Arc::new(Backend {
                result,
                shown: Mutex::new(Vec::new()),
            })
        }
    }
    impl ToastBackend for Backend {
        fn show(&self, toast: &Toast) -> Result<(), ToastError> {
            self.shown.lock().unwrap().push(toast.clone());
            self.result.clone()
        }
    }

    fn notifications(backend: &Arc<Backend>, focused: bool) -> Notifications {
        let backend: Arc<dyn ToastBackend> = Arc::clone(backend) as Arc<dyn ToastBackend>;
        Notifications::new(Arc::new(Notifier::new(backend, Arc::new(Window(focused)))))
    }

    fn ack_counter(count: &Arc<AtomicUsize>) -> Acknowledge {
        let count = Arc::clone(count);
        Arc::new(move |_| {
            count.fetch_add(1, Ordering::SeqCst);
            async { Ok(()) }.boxed()
        })
    }

    #[tokio::test]
    async fn not_acked_when_disabled() {
        let backend = Backend::new(Err(ToastError::Disabled("DisabledForUser".into())));
        let acked = counter();
        notifications(&backend, false)
            .deliver(routine("run"), ack_counter(&acked))
            .await
            .unwrap();
        assert_eq!(backend.shown.lock().unwrap().len(), 1, "the OS was asked");
        assert_eq!(
            acked.load(Ordering::SeqCst),
            0,
            "but a notice it didn't show stays pending"
        );
        // A failing Show() is the same.
        let failing = Backend::new(Err(ToastError::Failed("boom".into())));
        notifications(&failing, false)
            .deliver(routine("run"), ack_counter(&acked))
            .await
            .unwrap();
        assert_eq!(acked.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn acked_after_the_os_shows_it_with_the_click_route() {
        let backend = Backend::new(Ok(()));
        let acked = counter();
        notifications(&backend, false)
            .deliver(routine("run"), ack_counter(&acked))
            .await
            .unwrap();
        assert_eq!(acked.load(Ordering::SeqCst), 1);
        assert_eq!(
            backend.shown.lock().unwrap().as_slice(),
            [Toast {
                title: "Routine failed".into(),
                body: "Failure".into(),
                route: Some("/chat/?bot=bot".into()),
            }]
        );
    }

    #[tokio::test]
    async fn skipped_but_acked_while_the_window_is_visible_and_focused() {
        let backend = Backend::new(Err(ToastError::Failed("must not be asked".into())));
        let acked = counter();
        notifications(&backend, true)
            .deliver(routine("run"), ack_counter(&acked))
            .await
            .unwrap();
        assert!(backend.shown.lock().unwrap().is_empty());
        assert_eq!(acked.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn the_pending_replay_goes_through_the_same_delivery() {
        let backend = Backend::new(Ok(()));
        let acked = counter();
        let acknowledge = ack_counter(&acked);
        let notifications = notifications(&backend, false);
        let pending = vec![
            PendingNotification::RoutineFailed {
                bot_id: "bot".into(),
                routine_id: "routine".into(),
                run_id: "run-1".into(),
                title: "T1".into(),
                body: "B1".into(),
            },
            PendingNotification::RoutineFailed {
                bot_id: "bot".into(),
                routine_id: "routine".into(),
                run_id: "run-2".into(),
                title: "T2".into(),
                body: "B2".into(),
            },
        ];
        notifications
            .replay(pending.clone(), &acknowledge)
            .await
            .unwrap();
        assert_eq!(acked.load(Ordering::SeqCst), 2);
        // Replaying again (after a reconnect) shows nothing twice, and acknowledges again.
        notifications.replay(pending, &acknowledge).await.unwrap();
        assert_eq!(backend.shown.lock().unwrap().len(), 2);
        assert_eq!(acked.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn a_toast_escapes_what_agents_and_routines_write() {
        let toast = Toast {
            title: "a<b>&\"c\"".into(),
            body: "</text><text>x</text>\u{7}ok'".into(),
            route: Some("/chat/?bot=a&b=\"".into()),
        };
        let xml = toast_xml(&toast);
        assert!(xml.contains("<text>a&lt;b&gt;&amp;&quot;c&quot;</text>"));
        assert!(xml.contains("<text>&lt;/text&gt;&lt;text&gt;x&lt;/text&gt;ok&apos;</text>"));
        assert!(xml.contains("launch=\"/chat/?bot=a&amp;b=&quot;\""));
        // Windows itself parses it, and finds exactly the two text nodes we wrote.
        let document = XmlDocument::new().unwrap();
        document.LoadXml(&HSTRING::from(xml)).unwrap();
        let texts = document
            .GetElementsByTagName(&HSTRING::from("text"))
            .unwrap();
        assert_eq!(texts.Size().unwrap(), 2);
        assert_eq!(
            texts.Item(0).unwrap().InnerText().unwrap().to_string(),
            "a<b>&\"c\""
        );
        let plain = toast_xml(&Toast {
            title: "t".into(),
            body: "b".into(),
            route: None,
        });
        assert!(plain.starts_with("<toast><visual>"));
    }

    fn identity(kind: Kind, app_id: &'static str) -> Identity {
        Identity {
            kind,
            app_id,
            product: "Tjommie RS",
            ..crate::identity::DEV
        }
    }

    #[test]
    fn only_a_build_without_an_installer_registers_its_own_id() {
        let registry = super::super::registry::Fake::default();
        assert!(
            !register_aumid(
                &registry,
                &identity(Kind::Production, "com.tsnyders.tjommie")
            )
            .unwrap()
        );
        assert!(
            !register_aumid(
                &registry,
                &identity(Kind::UpdateTest, "com.tsnyders.tjommie.updatetest")
            )
            .unwrap()
        );
        assert_eq!(registry.writes(), 0, "never the installed apps' ids");
        assert!(
            register_aumid(&registry, &identity(Kind::Dev, "com.tsnyders.tjommie-rs")).unwrap()
        );
        assert_eq!(
            registry
                .read_string(
                    super::super::registry::Hive::CurrentUser,
                    r"Software\Classes\AppUserModelId\com.tsnyders.tjommie-rs",
                    "DisplayName"
                )
                .as_deref(),
            Some("Tjommie RS")
        );
    }

    // Windows has no setting to read for an id it has never shown a toast for. That is why `show` asks again after
    // showing, and why a notice for an unregistered id is never acknowledged.
    #[test]
    fn an_id_windows_has_not_met_has_no_notification_setting() {
        let (clicks, _received) = std::sync::mpsc::channel();
        let toasts = WinRtToasts::new(
            &format!("com.tsnyders.tjommie-rs.test.{}", uuid::Uuid::new_v4()),
            clicks,
        );
        // (A machine without the notification platform can't make a notifier at all, which is the same answer.)
        if let Ok(notifier) = toasts.notifier() {
            assert!(notifier.Setting().is_err());
        }
    }

    // Shows one real toast ("Tjommie RS notify test") under its own test id, which this test registers and removes.
    // Run by hand: cargo test -p desktop-host --locked real_toast -- --ignored. Not in the normal run: it appears on
    // the screen, and leaves the notification platform's own record of the id (a few bytes under HKCU).
    #[test]
    #[ignore = "shows a real toast"]
    fn real_toast_is_shown_and_clicked_under_its_own_test_id() {
        let app_id = "com.tsnyders.tjommie-rs.notifytest";
        let key = format!(r"Software\Classes\AppUserModelId\{app_id}");
        super::super::registry::Windows
            .write_string(&key, "DisplayName", "Tjommie RS notify test")
            .unwrap();
        let (clicks, received) = std::sync::mpsc::channel();
        let toasts = WinRtToasts::new(app_id, clicks);
        let shown = toasts.show(&Toast {
            title: "Tjommie RS notify test".into(),
            body: "A test toast. It is safe to dismiss.".into(),
            route: Some("/chat/?bot=test".into()),
        });
        assert!(received.try_recv().is_err());
        if let Ok(history) = ToastNotificationManager::History() {
            let _ = history.ClearWithId(&HSTRING::from(app_id));
        }
        super::super::registry::delete_tree(&key);
        assert_eq!(shown, Ok(()));
    }
}
