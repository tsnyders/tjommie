//! Whether Windows lets desktop apps use the microphone. Settings > Privacy & security > Microphone has two switches
//! that matter to a desktop app: "Microphone access" and "Let desktop apps access your microphone". When either is off
//! (or policy turns it off for the machine), capture yields silence rather than an error, so dictation reports it
//! up front: "Microphone access is off for desktop apps in Windows Settings". Windows records each switch as
//! "Allow" or "Deny" under the capability consent store.

use protocol::speech::SpeechPermission;

use crate::shell::registry::{Hive, Registry};

const CONSENT: &str =
    r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";

/// What the message says when `Denied`.
pub const BLOCKED: &str = "Microphone access is off for desktop apps in Windows Settings.";

pub fn microphone_permission(registry: &dyn Registry) -> SpeechPermission {
    let desktop_apps = format!(r"{CONSENT}\NonPackaged");
    let value = |hive, key: &str| registry.read_string(hive, key, "Value");
    let user = value(Hive::CurrentUser, CONSENT);
    let switches = [
        value(Hive::LocalMachine, CONSENT),
        user.clone(),
        value(Hive::CurrentUser, &desktop_apps),
        value(Hive::LocalMachine, &desktop_apps),
    ];
    if switches
        .iter()
        .flatten()
        .any(|value| value.eq_ignore_ascii_case("Deny"))
    {
        SpeechPermission::Denied
    } else if user.is_some_and(|value| value.eq_ignore_ascii_case("Allow")) {
        SpeechPermission::Granted
    } else {
        SpeechPermission::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::registry::Fake;

    fn with(entries: &[(&str, &str)]) -> Fake {
        let registry = Fake::default();
        for (key, value) in entries {
            registry.set_string(key, "Value", value);
        }
        registry
    }

    #[test]
    fn follows_the_windows_privacy_switches() {
        let nonpackaged = format!(r"{CONSENT}\NonPackaged");
        assert_eq!(
            microphone_permission(&Fake::default()),
            SpeechPermission::Unknown
        );
        assert_eq!(
            microphone_permission(&with(&[(CONSENT, "Allow")])),
            SpeechPermission::Granted
        );
        assert_eq!(
            microphone_permission(&with(&[(CONSENT, "Allow"), (&nonpackaged, "Allow")])),
            SpeechPermission::Granted
        );
        // Either switch off blocks desktop apps.
        assert_eq!(
            microphone_permission(&with(&[(CONSENT, "Deny")])),
            SpeechPermission::Denied
        );
        assert_eq!(
            microphone_permission(&with(&[(CONSENT, "Allow"), (&nonpackaged, "Deny")])),
            SpeechPermission::Denied
        );
        assert_eq!(
            microphone_permission(&with(&[(&nonpackaged, "deny")])),
            SpeechPermission::Denied
        );
        assert!(
            BLOCKED.starts_with("Microphone access is off for desktop apps in Windows Settings")
        );
    }
}
