//! `vault-entry.ts`: how a login is typed into the secure entry window and turned into a [`VaultSecret`]. The window
//! itself (a GPUI window on the trusted surface: no devtools, no navigation, no webview) is not here. This is the logic its
//! answers pass through: the field limits, the trimmed name, the exact origin, the secret rules, the wiping of what was
//! typed, and the cancel and timeout rules.

use std::fmt;
use std::future::Future;
use std::time::Duration;

use protocol::vault::{LoginEntry, LoginOrigin};
use zeroize::Zeroizing;

use super::login::{Cancel, LoginError};
use super::storage::{VaultSecret, validate_vault_secret};

/// Keeps a window out of screen capture, as Electron's `setContentProtection` kept the Logins window: Windows shows it
/// black in screenshots and recordings. `hwnd` is the window's handle. Windows' error if it didn't take it.
pub fn exclude_from_capture(hwnd: isize) -> windows::core::Result<()> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE,
    };
    // SAFETY: a plain Win32 call on a window handle; a stale handle only makes it fail.
    unsafe { SetWindowDisplayAffinity(HWND(hwnd as *mut _), WDA_EXCLUDEFROMCAPTURE) }
}

/// The window closes itself, and the answer is a cancel, after this long without an answer (`vault-entry.ts`).
pub const ENTRY_TIMEOUT: Duration = Duration::from_secs(120);

/// What the window shows when its fields do not pass. The window stays open and asks again.
pub const REFUSED: &str = "Check the login fields in this native window.";

/// The typed fields did not pass. The text is [`REFUSED`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldsRefused;

impl fmt::Display for FieldsRefused {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(REFUSED)
    }
}

impl std::error::Error for FieldsRefused {}

/// What the entry window opens with: a blank form for a new login, or the saved name and origin when editing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryPrefill {
    pub bot_id: String,
    pub name: String,
    pub origin: String,
}

impl EntryPrefill {
    pub fn new(bot_id: &str, previous: Option<&LoginEntry>) -> Self {
        Self {
            bot_id: bot_id.to_owned(),
            name: previous.map_or_else(String::new, |entry| entry.name.clone()),
            origin: previous.map_or_else(String::new, |entry| entry.origin.as_str().to_owned()),
        }
    }
}

/// The fields the person typed. The username and password are wiped when the form drops, whether it was accepted or
/// refused. `Debug` shows nothing of it.
pub struct EntryForm {
    name: String,
    origin: String,
    username: Zeroizing<String>,
    password: Zeroizing<String>,
}

impl EntryForm {
    pub fn new(name: String, origin: String, username: String, password: String) -> Self {
        Self {
            name,
            origin,
            username: Zeroizing::new(username),
            password: Zeroizing::new(password),
        }
    }

    /// The window's submit checks (`vault-entry.ts`): the name trimmed then 1 to 100 code points, the origin an exact
    /// HTTPS origin of at most 2048 characters, the username 1 to 320 and the password 1 to 2400 code points, then the
    /// secret rules. The typed values move into a [`VaultSecret`], which wipes them when it drops.
    pub fn into_input(self) -> Result<EntryInput, FieldsRefused> {
        let EntryForm {
            name,
            origin,
            mut username,
            mut password,
        } = self;
        let name = js_trim(&name).to_owned();
        let valid = within(&name, 1, 100)
            && origin.chars().count() <= 2048
            && within(&username, 1, 320)
            && within(&password, 1, 2400);
        if !valid {
            return Err(FieldsRefused);
        }
        let origin = LoginOrigin::new(&origin).map_err(|_| FieldsRefused)?;
        let secret = VaultSecret::new(
            std::mem::take(&mut *username),
            std::mem::take(&mut *password),
        );
        validate_vault_secret(&secret).map_err(|_| FieldsRefused)?;
        Ok(EntryInput {
            name,
            origin,
            secret,
        })
    }
}

impl fmt::Debug for EntryForm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EntryForm(<redacted>)")
    }
}

/// A checked login: the trimmed name, the exact origin and the secret.
#[derive(Debug)]
pub struct EntryInput {
    pub name: String,
    pub origin: LoginOrigin,
    pub secret: VaultSecret,
}

/// What the window sends back for one request: the typed form (an entry), a yes (a confirmation), or nothing (a cancel).
#[derive(Debug)]
pub enum WindowAnswer {
    Cancel,
    Confirm,
    Form(EntryForm),
}

/// The entry window's rule for an answer: a cancel gives `None`, a valid form gives its input, and anything else is
/// refused with [`FieldsRefused`], which the window shows.
pub fn accept_entry(answer: WindowAnswer) -> Result<Option<EntryInput>, FieldsRefused> {
    match answer {
        WindowAnswer::Cancel => Ok(None),
        WindowAnswer::Form(form) => form.into_input().map(Some),
        WindowAnswer::Confirm => Err(FieldsRefused),
    }
}

/// The confirmation window's rule for an answer: a cancel is `false`, a yes is `true`, and a form is refused.
pub fn accept_confirmation(answer: WindowAnswer) -> Result<bool, FieldsRefused> {
    match answer {
        WindowAnswer::Cancel => Ok(false),
        WindowAnswer::Confirm => Ok(true),
        WindowAnswer::Form(_) => Err(FieldsRefused),
    }
}

/// `String.prototype.trim()`, which zod's `.trim()` uses. Rust's `trim()` differs at U+0085 and U+FEFF. This is the
/// storage module's `js_trim`, which is private there.
fn js_trim(text: &str) -> &str {
    text.trim_matches(|c: char| {
        matches!(
            c,
            '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
                ..='\u{200a}'
                    | '\u{2028}'
                    | '\u{2029}'
                    | '\u{202f}'
                    | '\u{205f}'
                    | '\u{3000}'
                    | '\u{feff}'
        )
    })
}

/// zod's length check: between `min` and `max` code points.
fn within(text: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&text.chars().count())
}

/// Waits for a window's answer, but gives up as a cancel (`None`) when `cancel` fires or `timeout` passes first. A
/// cancel that has already fired never opens the window.
pub(crate) async fn answer_or_cancel<T, F>(
    answer: F,
    cancel: &Cancel,
    timeout: Duration,
) -> Option<Result<T, LoginError>>
where
    F: Future<Output = Result<T, LoginError>>,
{
    if cancel.is_cancelled() {
        return None;
    }
    tokio::select! {
        biased;
        result = answer => Some(result),
        () = cancel.cancelled() => None,
        () = tokio::time::sleep(timeout) => None,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures::FutureExt;
    use protocol::vault::LoginEntry;

    use super::*;

    const TYPED: &str = "Synthetic-Login-Secret-ABSENT-FROM-OUTPUT-7d14";

    fn form(name: &str, origin: &str, username: &str, password: &str) -> EntryForm {
        EntryForm::new(
            name.to_owned(),
            origin.to_owned(),
            username.to_owned(),
            password.to_owned(),
        )
    }

    fn valid(name: &str) -> EntryForm {
        form(name, "https://login.invalid", "synthetic-user", TYPED)
    }

    fn accepted(form: EntryForm) -> EntryInput {
        form.into_input().expect("the fields pass")
    }

    fn refused(form: EntryForm) -> bool {
        form.into_input().is_err()
    }

    fn saved_login() -> LoginEntry {
        LoginEntry {
            id: "00000000-0000-4000-8000-000000000123".to_owned(),
            name: "Saved account".to_owned(),
            origin: LoginOrigin::new("https://saved.invalid").expect("a valid origin"),
            bot_id: "bot-1".to_owned(),
            revision: 3,
            created_at: "2026-11-15T10:00:00.000Z".to_owned(),
            updated_at: "2026-11-15T10:00:00.000Z".to_owned(),
        }
    }

    #[test]
    fn the_name_is_trimmed_as_javascript_trims_it_and_limited_to_100_code_points() {
        assert_eq!(accepted(valid("  Synthetic  ")).name, "Synthetic");
        assert_eq!(accepted(valid("\u{feff}Synthetic")).name, "Synthetic");
        assert!(refused(valid("")));
        assert!(refused(valid("\u{feff} ")));
        assert!(refused(valid(&"a".repeat(101))));
        assert!(!refused(valid(&"a".repeat(100))));
        // zod counts code points, so 100 emoji are within the limit.
        assert!(!refused(valid(&"\u{1F600}".repeat(100))));
    }

    #[test]
    fn the_origin_must_be_an_exact_https_origin() {
        for origin in ["https://login.invalid", "https://login.invalid:8443"] {
            assert!(!refused(form("Synthetic", origin, "u", "p")), "{origin}");
        }
        for origin in [
            "",
            "http://login.invalid",
            "https://login.invalid/",
            "https://login.invalid/path",
            "https://user@login.invalid",
            "https://login.invalid?query",
        ] {
            assert!(refused(form("Synthetic", origin, "u", "p")), "{origin}");
        }
        let long = format!("https://{}.invalid", "a".repeat(2100));
        assert!(refused(form("Synthetic", &long, "u", "p")));
    }

    #[test]
    fn username_and_password_follow_the_secret_rules() {
        assert!(refused(form("Synthetic", "https://login.invalid", "", "p")));
        assert!(refused(form("Synthetic", "https://login.invalid", "u", "")));
        for bad in ["line\nbreak", "carriage\rreturn", "nul\0byte"] {
            assert!(refused(form(
                "Synthetic",
                "https://login.invalid",
                bad,
                "p"
            )));
            assert!(refused(form(
                "Synthetic",
                "https://login.invalid",
                "u",
                bad
            )));
        }
        assert!(!refused(form(
            "Synthetic",
            "https://login.invalid",
            &"a".repeat(320),
            "p"
        )));
        assert!(refused(form(
            "Synthetic",
            "https://login.invalid",
            &"a".repeat(321),
            "p"
        )));
        // zod's password limit is 2400 code points.
        assert!(!refused(form(
            "Synthetic",
            "https://login.invalid",
            "u",
            &"b".repeat(2400)
        )));
        assert!(refused(form(
            "Synthetic",
            "https://login.invalid",
            "u",
            &"b".repeat(2401)
        )));
        // The stored JSON is capped at 2500 bytes, which the TypeScript and the storage both enforce.
        assert!(!refused(form(
            "Synthetic",
            "https://login.invalid",
            &"a".repeat(300),
            &"b".repeat(2171)
        )));
        assert!(refused(form(
            "Synthetic",
            "https://login.invalid",
            &"a".repeat(300),
            &"b".repeat(2172)
        )));
    }

    #[test]
    fn a_refused_form_shows_the_fixed_message() {
        assert_eq!(
            FieldsRefused.to_string(),
            "Check the login fields in this native window."
        );
        assert_eq!(REFUSED, "Check the login fields in this native window.");
    }

    #[test]
    fn debug_output_shows_no_typed_field() {
        let typed = valid("Synthetic");
        let shown = format!("{typed:?}");
        assert!(!shown.contains(TYPED) && !shown.contains("synthetic-user"));
        let input = accepted(valid("Synthetic"));
        let shown = format!("{input:?}");
        assert!(!shown.contains(TYPED) && !shown.contains("synthetic-user"));
        let answer = format!("{:?}", WindowAnswer::Form(valid("Synthetic")));
        assert!(!answer.contains(TYPED) && !answer.contains("synthetic-user"));
    }

    #[test]
    fn the_window_answers_follow_the_submit_rules() {
        assert!(matches!(accept_entry(WindowAnswer::Cancel), Ok(None)));
        assert_eq!(
            accept_entry(WindowAnswer::Confirm).map(|input| input.is_some()),
            Err(FieldsRefused)
        );
        assert!(matches!(
            accept_entry(WindowAnswer::Form(valid("Synthetic"))),
            Ok(Some(_))
        ));
        assert_eq!(
            accept_entry(WindowAnswer::Form(valid(""))).map(|input| input.is_some()),
            Err(FieldsRefused)
        );
        assert_eq!(accept_confirmation(WindowAnswer::Cancel), Ok(false));
        assert_eq!(accept_confirmation(WindowAnswer::Confirm), Ok(true));
        assert_eq!(
            accept_confirmation(WindowAnswer::Form(valid("Synthetic"))),
            Err(FieldsRefused)
        );
    }

    #[test]
    fn a_new_login_opens_blank_and_an_edit_opens_with_the_saved_name_and_origin() {
        assert_eq!(
            EntryPrefill::new("bot-1", None),
            EntryPrefill {
                bot_id: "bot-1".to_owned(),
                name: String::new(),
                origin: String::new(),
            }
        );
        assert_eq!(
            EntryPrefill::new("bot-1", Some(&saved_login())),
            EntryPrefill {
                bot_id: "bot-1".to_owned(),
                name: "Saved account".to_owned(),
                origin: "https://saved.invalid".to_owned(),
            }
        );
    }

    #[test]
    fn an_unanswered_window_times_out_after_two_minutes() {
        assert_eq!(ENTRY_TIMEOUT, Duration::from_secs(120));
    }

    #[tokio::test]
    async fn a_window_that_is_answered_keeps_its_answer() {
        let (_sender, cancel) = Cancel::pair();
        let answer = async { Ok::<u8, LoginError>(7) };
        assert_eq!(
            answer_or_cancel(answer, &cancel, Duration::from_secs(5)).await,
            Some(Ok(7))
        );
    }

    #[tokio::test]
    async fn a_silent_window_is_a_cancel_once_its_signal_fires_or_its_timeout_passes() {
        let (sender, cancel) = Cancel::pair();
        let timed_out = answer_or_cancel(
            std::future::pending::<Result<u8, LoginError>>(),
            &cancel,
            Duration::from_millis(20),
        )
        .await;
        assert_eq!(timed_out, None);

        let fired = tokio::spawn({
            let cancel = cancel.clone();
            async move {
                answer_or_cancel(
                    std::future::pending::<Result<u8, LoginError>>(),
                    &cancel,
                    Duration::from_secs(60),
                )
                .await
            }
        });
        sender.send_replace(true);
        assert_eq!(fired.await.expect("joined"), None);
    }

    #[tokio::test]
    async fn a_signal_that_already_fired_never_waits_for_the_window() {
        let (sender, cancel) = Cancel::pair();
        sender.send_replace(true);
        let answer = std::future::pending::<Result<u8, LoginError>>().boxed();
        assert_eq!(
            answer_or_cancel(answer, &cancel, Duration::from_secs(60)).await,
            None
        );
    }
}
