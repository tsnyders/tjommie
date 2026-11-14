//! `native-browser-policy.ts`: what a bot's browser may navigate to, how its profile is named, and how the panel's
//! bounds are clipped to the window.

use protocol::native_browser::NativeBrowserBounds;
use sha2::{Digest as _, Sha256};
use url::Url;

/// Why the policy refused something. The messages are the TypeScript ones, shown to the person and the bot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyError(pub &'static str);

impl std::fmt::Display for PolicyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for PolicyError {}

/// The caption band: the page never covers it (`native-browser-policy.ts:24`).
const CAPTION_BAND: f64 = 44.0;

/// The bot's WebView2 profile name: the full 64-hex SHA-256 of its id. That fits WebView2's 64-character limit, uses
/// only allowed characters, and is the hash Electron used for its profile folders. An id can never pick a path.
pub fn profile_name(bot_id: &str) -> Result<String, PolicyError> {
    // JavaScript counts UTF-16 units.
    if bot_id.is_empty() || bot_id.encode_utf16().count() > 1000 {
        return Err(PolicyError("Invalid browser agent identity."));
    }
    let digest = Sha256::digest(bot_id.as_bytes());
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

/// HTTP, HTTPS and about:blank only, without credentials. Returns the normalized address.
pub fn native_browser_url(value: &str) -> Result<String, PolicyError> {
    let url = Url::parse(value).map_err(|_| PolicyError("Invalid URL"))?;
    if !(matches!(url.scheme(), "http" | "https") || url.as_str() == "about:blank") {
        return Err(PolicyError(
            "Browser navigation only supports HTTP, HTTPS and about:blank.",
        ));
    }
    if !url.username().is_empty() || url.password().is_some_and(|password| !password.is_empty()) {
        return Err(PolicyError("Browser addresses cannot contain credentials."));
    }
    Ok(url.into())
}

/// Child documents need ordinary web-created frames; none may navigate into app or OS privileges.
pub fn native_browser_frame_url(value: &str, is_main_frame: bool) -> Result<String, PolicyError> {
    if !is_main_frame
        && let Ok(url) = Url::parse(value)
        && (url.as_str() == "about:srcdoc" || matches!(url.scheme(), "blob" | "data"))
    {
        return Ok(url.into());
    }
    native_browser_url(value)
}

/// Window-space bounds from the panel's, at the app's zoom: clipped to the window and below the caption band.
pub fn native_browser_bounds(
    bounds: &NativeBrowserBounds,
    width: f64,
    height: f64,
    zoom: f64,
) -> NativeBrowserBounds {
    let left = (bounds.x * zoom).ceil().max(0.0).min(width);
    let top = (bounds.y * zoom).ceil().max(CAPTION_BAND).min(height);
    let right = ((bounds.x + bounds.width) * zoom).floor().min(width);
    let bottom = ((bounds.y + bounds.height) * zoom).floor().min(height);
    NativeBrowserBounds {
        x: left,
        y: top,
        width: (right - left).max(0.0),
        height: (bottom - top).max(0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds(x: f64, y: f64, width: f64, height: f64) -> NativeBrowserBounds {
        NativeBrowserBounds {
            x,
            y,
            width,
            height,
        }
    }

    #[test]
    fn isolates_deterministic_profiles_without_letting_ids_choose_paths() {
        let profile = profile_name("../../outside").unwrap();
        assert_eq!(profile.len(), 64);
        assert!(
            profile
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
        );
        assert_eq!(profile_name("../../outside").unwrap(), profile);
        assert_ne!(profile_name("other").unwrap(), profile);
        // The same hash Electron used: sha256("abc").
        assert_eq!(
            profile_name("abc").unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(profile_name("").is_err());
        assert!(profile_name(&"x".repeat(1001)).is_err());
        assert!(profile_name(&"x".repeat(1000)).is_ok());
    }

    #[test]
    fn allows_web_addresses_and_blank_pages_refusing_privileges_and_credentials() {
        assert_eq!(
            native_browser_url("https://example.test/path").unwrap(),
            "https://example.test/path"
        );
        assert_eq!(native_browser_url("about:blank").unwrap(), "about:blank");
        for url in [
            "file:///C:/secret",
            "javascript:alert(1)",
            "app://tjommie",
            "chrome://settings",
            "data:text/html,hi",
            "https://name:password@example.test",
            "not a url",
        ] {
            assert!(native_browser_url(url).is_err(), "{url} must be refused");
        }
    }

    #[test]
    fn clips_browser_surfaces_to_the_owner_and_protects_the_caption_band() {
        assert_eq!(
            native_browser_bounds(&bounds(900.0, 0.0, 400.0, 800.0), 1000.0, 700.0, 1.0),
            bounds(900.0, 44.0, 100.0, 656.0)
        );
        assert_eq!(
            native_browser_bounds(&bounds(1200.0, 1000.0, 100.0, 100.0), 1000.0, 700.0, 1.0),
            bounds(1000.0, 700.0, 0.0, 0.0)
        );
    }

    #[test]
    fn scales_css_bounds_once_before_clipping_the_native_surface() {
        assert_eq!(
            native_browser_bounds(&bounds(240.0, 160.0, 480.0, 360.0), 1000.0, 700.0, 0.75),
            bounds(180.0, 120.0, 360.0, 270.0)
        );
        assert_eq!(
            native_browser_bounds(&bounds(640.0, 20.0, 480.0, 600.0), 1000.0, 700.0, 1.25),
            bounds(800.0, 44.0, 200.0, 656.0)
        );
    }

    #[test]
    fn allows_web_created_child_frames_without_relaxing_main_frame_navigation() {
        for url in [
            "about:srcdoc",
            "blob:https://example.test/fixture",
            "data:text/html,child",
        ] {
            assert_eq!(native_browser_frame_url(url, false).unwrap(), url);
            assert!(native_browser_frame_url(url, true).is_err(), "{url}");
        }
        for url in [
            "file:///C:/secret",
            "chrome://settings",
            "devtools://devtools",
            "javascript:alert(1)",
            "app://tjommie",
            "about:config",
        ] {
            assert!(native_browser_frame_url(url, false).is_err(), "{url}");
        }
    }
}
