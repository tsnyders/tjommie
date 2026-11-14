//! `native-browser-permissions.ts`: a bot's browser grants a page nothing. Every permission request is denied, with
//! a notice, and device pickers do not exist. The policy is a pure function so it is tested without a browser; the
//! engine calls it from WebView2's `PermissionRequested` event.

/// What a page can ask for (WebView2's `COREWEBVIEW2_PERMISSION_KIND`), named as Electron named them, so the notice
/// reads the same as before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Unknown,
    Microphone,
    Camera,
    Geolocation,
    Notifications,
    OtherSensors,
    ClipboardRead,
    MultipleDownloads,
    FileReadWrite,
    Autoplay,
    LocalFonts,
    MidiSystemExclusiveMessages,
    WindowManagement,
}

impl Permission {
    /// From the raw `COREWEBVIEW2_PERMISSION_KIND` value. A kind this version does not know is `Unknown`, and denied.
    pub fn from_kind(kind: i32) -> Self {
        match kind {
            1 => Self::Microphone,
            2 => Self::Camera,
            3 => Self::Geolocation,
            4 => Self::Notifications,
            5 => Self::OtherSensors,
            6 => Self::ClipboardRead,
            7 => Self::MultipleDownloads,
            8 => Self::FileReadWrite,
            9 => Self::Autoplay,
            10 => Self::LocalFonts,
            11 => Self::MidiSystemExclusiveMessages,
            12 => Self::WindowManagement,
            _ => Self::Unknown,
        }
    }

    /// Electron's name for it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Microphone => "media",
            Self::Camera => "media",
            Self::Geolocation => "geolocation",
            Self::Notifications => "notifications",
            Self::OtherSensors => "sensors",
            Self::ClipboardRead => "clipboard-read",
            Self::MultipleDownloads => "multiple-downloads",
            Self::FileReadWrite => "fileSystem",
            Self::Autoplay => "autoplay",
            Self::LocalFonts => "local-fonts",
            Self::MidiSystemExclusiveMessages => "midiSysex",
            Self::WindowManagement => "window-management",
        }
    }
}

/// The notice shown for a denied request (`native-browser.ts:810`).
pub fn notice(permission: Permission) -> String {
    format!(
        "Browser permission '{}' is unavailable in Tjommie.",
        permission.name()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_known_or_not_has_a_notice() {
        // The engine denies whatever a page asks for; the person is told what, whatever it was.
        for kind in -3..40 {
            assert!(notice(Permission::from_kind(kind)).starts_with("Browser permission '"));
        }
    }

    #[test]
    fn names_requests_the_way_electron_did_and_unknown_kinds_are_still_named() {
        assert_eq!(
            notice(Permission::from_kind(3)),
            "Browser permission 'geolocation' is unavailable in Tjommie."
        );
        assert_eq!(Permission::from_kind(1).name(), "media");
        assert_eq!(Permission::from_kind(2).name(), "media");
        assert_eq!(Permission::from_kind(4).name(), "notifications");
        assert_eq!(Permission::from_kind(99), Permission::Unknown);
        assert_eq!(Permission::from_kind(-1), Permission::Unknown);
    }
}
