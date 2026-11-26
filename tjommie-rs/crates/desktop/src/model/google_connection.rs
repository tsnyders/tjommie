//! Port of `packages/ui/lib/google-connection.ts` (T1.4): the Google connection as the UI sees it, the daemon's status and
//! nothing else (no token reaches the UI). The sign-in buttons' Electron opener is not ported: the desktop opens the consent
//! page itself.

use std::time::Duration;

use client::{Rpc, RpcError};
use protocol::google::{GoogleAttemptState, GoogleService, GoogleStatus};
use protocol::wire::{Empty, method};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::{Instant, sleep_until};

/// `POLL_MS`: how often a waiting sign-in is read.
pub const POLL: Duration = Duration::from_millis(1000);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoogleStep {
    Setup,
    Connect,
    Waiting,
    Connected,
}

fn is_waiting(status: &GoogleStatus) -> bool {
    status
        .attempt
        .as_ref()
        .is_some_and(|attempt| attempt.state == GoogleAttemptState::Waiting)
}

/// Set up, connect, waiting for the browser, or connected.
pub fn google_step(status: &GoogleStatus) -> GoogleStep {
    if status.connected {
        GoogleStep::Connected
    } else if is_waiting(status) {
        GoogleStep::Waiting
    } else if status.configured {
        GoogleStep::Connect
    } else {
        GoogleStep::Setup
    }
}

/// The Desktop app client ID's shape, which Google issues; the daemon rejects any other.
pub fn valid_client_id(id: &str) -> bool {
    id.trim()
        .strip_suffix(".apps.googleusercontent.com")
        .is_some_and(|head| {
            !head.is_empty()
                && head
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        })
}

/// What a connection asks for: Gmail, Drive and Calendar, with full access.
pub const GOOGLE_SERVICES: [GoogleService; 3] = [
    GoogleService::Gmail,
    GoogleService::Drive,
    GoogleService::Calendar,
];

fn service_name(service: GoogleService) -> &'static str {
    match service {
        GoogleService::Gmail => "Gmail",
        GoogleService::Drive => "Drive",
        GoogleService::Calendar => "Calendar",
    }
}

fn ability(service: GoogleService) -> &'static str {
    match service {
        GoogleService::Gmail => "send and manage your email",
        GoogleService::Drive => "create, edit and delete Drive files",
        GoogleService::Calendar => "create, edit and delete Calendar events",
    }
}

/// Words joined the way the UI writes lists: "A", "A and B", "A, B and C".
fn in_words(words: &[&str]) -> String {
    match words {
        [] => String::new(),
        [only] => (*only).to_owned(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// The services a connection covers, in words. Before one is connected, the services it will ask for.
pub fn service_list(services: &[GoogleService]) -> String {
    let covered = if services.is_empty() {
        &GOOGLE_SERVICES[..]
    } else {
        services
    };
    let names: Vec<&str> = covered
        .iter()
        .map(|service| service_name(*service))
        .collect();
    in_words(&names)
}

/// What full access lets a bot do for the services covered.
pub fn abilities(services: &[GoogleService]) -> String {
    let covered = if services.is_empty() {
        &GOOGLE_SERVICES[..]
    } else {
        services
    };
    let each: Vec<&str> = covered.iter().map(|service| ability(*service)).collect();
    in_words(&each)
}

/// A connection made when Google was asked for read-only access can't do the rest. The `access` of "read" is the daemon's
/// field when it sends one; the wire status has none, so the caller passes it in. Such a connection must be made again.
pub fn needs_reconsent(status: &GoogleStatus, access: Option<&str>) -> bool {
    status.connected && access == Some("read")
}

/// Whether the saved local settings can be cleared: when nothing is connected but something is saved or went wrong.
pub fn can_clear_local(status: &GoogleStatus) -> bool {
    let failed = status
        .attempt
        .as_ref()
        .is_some_and(|attempt| attempt.state == GoogleAttemptState::Failed);
    !status.connected && (status.configured || failed)
}

pub fn can_read(status: &GoogleStatus, bot_id: &str) -> bool {
    status
        .granted_bot_ids
        .iter()
        .any(|granted| granted == bot_id)
}

/// Why the last sign-in didn't work, if it didn't.
pub fn attempt_problem(status: &GoogleStatus) -> Option<String> {
    status
        .attempt
        .as_ref()
        .filter(|attempt| attempt.state == GoogleAttemptState::Failed)
        .map(|attempt| {
            attempt
                .message
                .clone()
                .unwrap_or_else(|| "Google sign-in didn't finish.".to_owned())
        })
}

/// A short line for the bot panel's card. `None` status is still being read.
pub fn card_line(
    status: Option<&GoogleStatus>,
    access: Option<&str>,
    bot_name: &str,
    bot_id: &str,
) -> String {
    let Some(status) = status else {
        return "Checking…".to_owned();
    };
    match google_step(status) {
        GoogleStep::Setup => return "Not set up yet".to_owned(),
        GoogleStep::Connect => return "Not connected".to_owned(),
        GoogleStep::Waiting => return "Waiting for you to finish in your browser".to_owned(),
        GoogleStep::Connected => {}
    }
    let who = status
        .email
        .clone()
        .unwrap_or_else(|| "Connected".to_owned());
    if needs_reconsent(status, access) {
        format!("{who}. Reconnect to give full access.")
    } else if can_read(status, bot_id) {
        format!(
            "{who}. {bot_name} has full access to {}.",
            service_list(&status.services)
        )
    } else {
        format!("{who}. {bot_name} has no access.")
    }
}

/// `watchGoogle`: reads the status now, again every `every` while a sign-in is waiting, and whenever a change arrives on
/// `changes`. A failed read goes to `on_error`, and the reads that follow go on. It runs until it is dropped (the
/// TypeScript's `stop`), or until `changes` has closed and nothing is scheduled.
pub async fn watch_google<R: Rpc>(
    rpc: &R,
    changes: &mut UnboundedReceiver<GoogleStatus>,
    every: Duration,
    mut on_status: impl FnMut(GoogleStatus),
    mut on_error: impl FnMut(RpcError),
) {
    let mut next_read = Some(Instant::now());
    let mut open = true;
    loop {
        tokio::select! {
            biased;
            change = changes.recv(), if open => match change {
                Some(status) => {
                    next_read = is_waiting(&status).then(|| Instant::now() + every);
                    on_status(status);
                }
                None => open = false,
            },
            () = sleep_until(next_read.unwrap_or_else(Instant::now)), if next_read.is_some() => {
                next_read = None;
                match rpc.call::<method::GoogleStatus>(Empty {}).await {
                    Ok(status) => {
                        next_read = is_waiting(&status).then(|| Instant::now() + every);
                        on_status(status);
                    }
                    Err(error) => on_error(error),
                }
            }
            else => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use client::RpcError;
    use protocol::google::{GoogleAttempt, GoogleAttemptState, GoogleService, GoogleStatus};
    use protocol::wire::method;
    use tokio::sync::mpsc;

    use crate::model::testing::FakeRpc;

    use super::*;

    /// Short enough that the polling tests finish quickly; the app passes [`POLL`].
    const EVERY: Duration = Duration::from_millis(10);

    fn base() -> GoogleStatus {
        GoogleStatus {
            configured: true,
            connected: false,
            email: None,
            services: vec![],
            granted_bot_ids: vec![],
            attempt: None,
        }
    }

    fn connected() -> GoogleStatus {
        GoogleStatus {
            connected: true,
            email: Some("me@example.com".to_owned()),
            services: vec![GoogleService::Gmail],
            granted_bot_ids: vec!["b1".to_owned()],
            ..base()
        }
    }

    fn waiting() -> GoogleStatus {
        GoogleStatus {
            attempt: Some(GoogleAttempt {
                id: "a1".to_owned(),
                state: GoogleAttemptState::Waiting,
                message: None,
            }),
            ..base()
        }
    }

    /// A daemon whose `google.status` answers `statuses` in turn, then the last one again.
    fn daemon(statuses: Vec<GoogleStatus>) -> FakeRpc {
        let mut next = 0;
        FakeRpc::new().on::<method::GoogleStatus>(move |_| {
            let status = statuses[next.min(statuses.len() - 1)].clone();
            next += 1;
            Ok(status)
        })
    }

    #[test]
    fn is_set_up_connect_waiting_or_connected() {
        assert_eq!(
            google_step(&GoogleStatus {
                configured: false,
                ..base()
            }),
            GoogleStep::Setup
        );
        assert_eq!(google_step(&base()), GoogleStep::Connect);
        assert_eq!(google_step(&waiting()), GoogleStep::Waiting);
        let failed = GoogleStatus {
            attempt: Some(GoogleAttempt {
                id: "a1".to_owned(),
                state: GoogleAttemptState::Failed,
                message: Some("Denied".to_owned()),
            }),
            ..base()
        };
        assert_eq!(google_step(&failed), GoogleStep::Connect);
        assert_eq!(google_step(&connected()), GoogleStep::Connected);
    }

    #[test]
    fn gives_access_only_to_the_bots_granted_it_and_none_by_default() {
        assert!(can_read(&connected(), "b1"));
        assert!(!can_read(&connected(), "b2"));
        assert!(!can_read(
            &GoogleStatus {
                granted_bot_ids: vec![],
                ..connected()
            },
            "b1"
        ));
    }

    #[test]
    fn says_why_a_sign_in_failed() {
        let failed = |message: Option<&str>| GoogleStatus {
            attempt: Some(GoogleAttempt {
                id: "a1".to_owned(),
                state: GoogleAttemptState::Failed,
                message: message.map(str::to_owned),
            }),
            ..base()
        };
        assert_eq!(
            attempt_problem(&failed(Some("Access denied."))),
            Some("Access denied.".to_owned())
        );
        assert!(attempt_problem(&failed(None)).is_some_and(|text| text.contains("didn't finish")));
        assert_eq!(attempt_problem(&waiting()), None);
    }

    #[test]
    fn describes_itself_for_the_bot_panels_card() {
        assert_eq!(card_line(None, None, "Scout", "b1"), "Checking…");
        assert_eq!(
            card_line(
                Some(&GoogleStatus {
                    configured: false,
                    ..base()
                }),
                None,
                "Scout",
                "b1"
            ),
            "Not set up yet"
        );
        assert_eq!(
            card_line(Some(&connected()), None, "Scout", "b1"),
            "me@example.com. Scout has full access to Gmail."
        );
        assert_eq!(
            card_line(Some(&connected()), None, "Scout", "b2"),
            "me@example.com. Scout has no access."
        );
    }

    #[test]
    fn lists_services_in_words_and_before_any_is_connected_names_what_will_be_asked_for() {
        assert_eq!(service_list(&[GoogleService::Gmail]), "Gmail");
        assert_eq!(
            service_list(&[GoogleService::Gmail, GoogleService::Drive]),
            "Gmail and Drive"
        );
        assert_eq!(
            service_list(&[
                GoogleService::Gmail,
                GoogleService::Drive,
                GoogleService::Calendar
            ]),
            "Gmail, Drive and Calendar"
        );
        assert_eq!(service_list(&[]), "Gmail, Drive and Calendar");
    }

    #[test]
    fn says_what_full_access_allows_for_the_services_covered() {
        assert_eq!(
            abilities(&[GoogleService::Gmail]),
            "send and manage your email"
        );
        assert_eq!(
            abilities(&[]),
            "send and manage your email, create, edit and delete Drive files and create, edit and delete Calendar events"
        );
    }

    #[test]
    fn needs_a_new_consent_only_for_a_connection_the_daemon_says_is_read_only() {
        assert!(needs_reconsent(&connected(), Some("read")));
        assert!(!needs_reconsent(&connected(), Some("full")));
        assert!(!needs_reconsent(&connected(), None));
        assert!(!needs_reconsent(&base(), Some("read")));
        assert_eq!(
            card_line(Some(&connected()), Some("read"), "Scout", "b1"),
            "me@example.com. Reconnect to give full access."
        );
    }

    #[test]
    fn can_clear_local_settings_only_while_not_connected_and_something_is_saved_or_wrong() {
        assert!(!can_clear_local(&GoogleStatus {
            configured: false,
            ..base()
        }));
        assert!(can_clear_local(&base()));
        let failed_load = GoogleStatus {
            configured: false,
            attempt: Some(GoogleAttempt {
                id: "load".to_owned(),
                state: GoogleAttemptState::Failed,
                message: None,
            }),
            ..base()
        };
        assert!(can_clear_local(&failed_load));
        assert!(!can_clear_local(&connected()));
    }

    #[test]
    fn accepts_a_google_client_id_and_nothing_else() {
        assert!(valid_client_id(" 123-abc_d.apps.googleusercontent.com "));
        assert!(!valid_client_id("123-abc"));
        assert!(!valid_client_id("evil.test/x.apps.googleusercontent.com"));
    }

    #[tokio::test]
    async fn reads_once_when_nothing_is_waiting() {
        let rpc = daemon(vec![base()]);
        let (_changes_tx, mut changes) = mpsc::unbounded_channel::<GoogleStatus>();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let watch = watch_google(
            &rpc,
            &mut changes,
            EVERY,
            move |status| sink.lock().unwrap().push(status),
            |_| {},
        );
        let _ = tokio::time::timeout(Duration::from_millis(60), watch).await;
        assert_eq!(rpc.calls().len(), 1);
        assert_eq!(*seen.lock().unwrap(), vec![base()]);
    }

    #[tokio::test]
    async fn polls_every_interval_while_a_sign_in_is_waiting_and_stops_when_it_is_done() {
        let rpc = daemon(vec![waiting(), waiting(), connected()]);
        let (_changes_tx, mut changes) = mpsc::unbounded_channel::<GoogleStatus>();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let watch = watch_google(
            &rpc,
            &mut changes,
            EVERY,
            move |status| sink.lock().unwrap().push(status),
            |_| {},
        );
        let _ = tokio::time::timeout(Duration::from_millis(150), watch).await;
        assert_eq!(rpc.calls().len(), 3);
        assert_eq!(seen.lock().unwrap().last(), Some(&connected()));
    }

    #[tokio::test]
    async fn follows_the_daemons_change_events_and_stops_following_when_stopped() {
        let rpc = daemon(vec![base()]);
        let (changes_tx, mut changes) = mpsc::unbounded_channel::<GoogleStatus>();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let driver = async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            changes_tx.send(connected()).unwrap();
            tokio::time::sleep(Duration::from_millis(30)).await;
        };
        let watch = watch_google(
            &rpc,
            &mut changes,
            EVERY,
            move |status| sink.lock().unwrap().push(status),
            |_| {},
        );
        tokio::select! {
            () = watch => {}
            () = driver => {}
        }
        assert_eq!(seen.lock().unwrap().last(), Some(&connected()));
        let stopped_at = seen.lock().unwrap().len();
        changes_tx.send(base()).unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(seen.lock().unwrap().len(), stopped_at);
    }

    #[tokio::test]
    async fn reports_a_failed_read() {
        let rpc = FakeRpc::new()
            .on::<method::GoogleStatus>(|_| Err(RpcError::BadResult("offline".to_owned())));
        let (_changes_tx, mut changes) = mpsc::unbounded_channel::<GoogleStatus>();
        let errors = Arc::new(Mutex::new(Vec::new()));
        let sink = errors.clone();
        let watch = watch_google(
            &rpc,
            &mut changes,
            EVERY,
            |_| {},
            move |error| sink.lock().unwrap().push(error),
        );
        let _ = tokio::time::timeout(Duration::from_millis(60), watch).await;
        assert_eq!(errors.lock().unwrap().len(), 1);
    }
}
