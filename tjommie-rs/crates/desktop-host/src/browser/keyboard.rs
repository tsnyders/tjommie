//! Keyboard hand-off between the page and the app (Phase 3 plan T3.5; `native-browser.ts:693-697` for Electron's
//! Shift+Tab, GPUI limits 9 and 10): which chords the app keeps for itself while a page has the keyboard, and the script
//! that lets Shift+Tab leave the page.
//!
//! WebView2 reports a key to the host only if it is an accelerator key (`AcceleratorKeyPressed`). Shift+Tab is not one,
//! so it never reaches the host that way. A script in an isolated world listens for it instead, and tells the host
//! through a binding that the page's own world can neither see nor call.

use serde::{Deserialize, Serialize};

use super::cdp::{Method, Nothing, method};

/// The name of the isolated world the hand-off script lives in, and of the binding it calls.
pub const WORLD: &str = "tjommie";
pub const BINDING: &str = "tjommieFocusReturn";

/// Which way focus leaves the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusDirection {
    /// Past the page's last control: to the next control of the app.
    Next,
    /// Before its first control, or Shift+Tab: to the previous one.
    Previous,
}

/// Runs in the isolated world of every document: Shift+Tab goes to the host, not into the page. Only a key the browser
/// itself made (`isTrusted`) counts: a page can dispatch a keydown of its own, which crosses into this world like any DOM
/// event, and must not be able to move the person's keyboard out of the panel.
pub const HANDOFF_SCRIPT: &str = r#"addEventListener("keydown", (event) => {
  if (event.isTrusted && event.key === "Tab" && event.shiftKey && !event.ctrlKey && !event.altKey && !event.metaKey) {
    event.preventDefault();
    event.stopImmediatePropagation();
    tjommieFocusReturn("previous");
  }
}, true);"#;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddScriptParams {
    pub source: String,
    pub world_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddBindingParams {
    pub name: String,
    pub execution_context_name: String,
}

/// `Runtime.bindingCalled`: the isolated world called the binding.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct BindingCalled {
    pub name: String,
    pub payload: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct ScriptId {
    pub identifier: String,
}

method!(
    /// Has a script run in a named isolated world of every document, before the page's own scripts.
    PageAddScriptToEvaluateOnNewDocument, "Page.addScriptToEvaluateOnNewDocument", AddScriptParams, ScriptId
);
method!(
    /// Exposes a function to the execution contexts of that name only.
    RuntimeAddBinding, "Runtime.addBinding", AddBindingParams, Nothing
);

/// One chord: a virtual key and the modifiers that must be held with it (and no others).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Chord {
    pub key: u32,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// The virtual-key code of a key name (`A`, `7`, `F5`, `Left`, `=`), as Windows numbers them.
fn virtual_key(name: &str) -> Option<u32> {
    let mut characters = name.chars();
    if let (Some(only), None) = (characters.next(), characters.next()) {
        let upper = only.to_ascii_uppercase();
        if upper.is_ascii_uppercase() || upper.is_ascii_digit() {
            return Some(u32::from(upper));
        }
        return Some(match only {
            '=' | '+' => 0xBB,
            ',' => 0xBC,
            '-' => 0xBD,
            '.' => 0xBE,
            '/' => 0xBF,
            ';' => 0xBA,
            '`' => 0xC0,
            '[' => 0xDB,
            '\\' => 0xDC,
            ']' => 0xDD,
            '\'' => 0xDE,
            ' ' => 0x20,
            _ => return None,
        });
    }
    if let Some(number) = name
        .strip_prefix(['F', 'f'])
        .and_then(|n| n.parse::<u32>().ok())
    {
        return (1..=24).contains(&number).then_some(0x6F + number);
    }
    Some(match name.to_ascii_lowercase().as_str() {
        "backspace" => 0x08,
        "tab" => 0x09,
        "enter" | "return" => 0x0D,
        "esc" | "escape" => 0x1B,
        "space" => 0x20,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "end" => 0x23,
        "home" => 0x24,
        "left" | "arrowleft" => 0x25,
        "up" | "arrowup" => 0x26,
        "right" | "arrowright" => 0x27,
        "down" | "arrowdown" => 0x28,
        "insert" => 0x2D,
        "delete" | "del" => 0x2E,
        _ => return None,
    })
}

impl Chord {
    /// A chord from its text: `Ctrl+Shift+T`, `Alt+Left`, `Ctrl+=`, `F5`. Modifier names are `Ctrl`, `Control`, `Shift` and
    /// `Alt`, in any case and order; a `+` that ends the text after a modifier is the plus key (`Ctrl++`). `None` for text
    /// that is not a chord.
    pub fn parse(text: &str) -> Option<Chord> {
        let text = text.trim();
        let (modifiers, key) = if let Some(head) = text.strip_suffix("++") {
            (head, "+")
        } else if text == "+" {
            ("", "+")
        } else {
            text.rsplit_once('+').unwrap_or(("", text))
        };
        let (mut ctrl, mut shift, mut alt) = (false, false, false);
        for name in modifiers.split('+').filter(|name| !name.is_empty()) {
            match name.trim().to_ascii_lowercase().as_str() {
                "ctrl" | "control" => ctrl = true,
                "shift" => shift = true,
                "alt" => alt = true,
                _ => return None,
            }
        }
        Some(Chord {
            key: virtual_key(key.trim())?,
            ctrl,
            shift,
            alt,
        })
    }

    /// Whether the key just pressed (`key`, with these modifiers held) is this chord.
    pub fn matches(&self, key: u32, ctrl: bool, shift: bool, alt: bool) -> bool {
        self.key == key && self.ctrl == ctrl && self.shift == shift && self.alt == alt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(key: u32, ctrl: bool, shift: bool, alt: bool) -> Chord {
        Chord {
            key,
            ctrl,
            shift,
            alt,
        }
    }

    #[test]
    fn chords_parse_in_any_case_and_order() {
        assert_eq!(
            Chord::parse("Ctrl+T"),
            Some(chord(0x54, true, false, false))
        );
        assert_eq!(
            Chord::parse("shift+CONTROL+t"),
            Some(chord(0x54, true, true, false))
        );
        assert_eq!(
            Chord::parse("Alt+Left"),
            Some(chord(0x25, false, false, true))
        );
        assert_eq!(Chord::parse("F5"), Some(chord(0x74, false, false, false)));
        assert_eq!(
            Chord::parse("Ctrl+F12"),
            Some(chord(0x7B, true, false, false))
        );
        assert_eq!(
            Chord::parse("Ctrl+="),
            Some(chord(0xBB, true, false, false))
        );
        assert_eq!(
            Chord::parse("Ctrl+-"),
            Some(chord(0xBD, true, false, false))
        );
        assert_eq!(
            Chord::parse("Ctrl+0"),
            Some(chord(0x30, true, false, false))
        );
        assert_eq!(
            Chord::parse("Ctrl+Shift+Tab"),
            Some(chord(0x09, true, true, false))
        );
        assert_eq!(
            Chord::parse("Escape"),
            Some(chord(0x1B, false, false, false))
        );
    }

    #[test]
    fn a_plus_after_modifiers_is_the_plus_key_and_nonsense_is_refused() {
        assert_eq!(
            Chord::parse("Ctrl++"),
            Some(chord(0xBB, true, false, false))
        );
        assert_eq!(
            Chord::parse("Ctrl+Shift++"),
            Some(chord(0xBB, true, true, false))
        );
        for bad in [
            "",
            "Ctrl+",
            "Hyper+T",
            "Ctrl+Nonsense",
            "Ctrl+F99",
            "Ctrl+Ctrl",
        ] {
            assert_eq!(Chord::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_chord_matches_only_its_own_modifiers() {
        let copy = Chord::parse("Ctrl+C").expect("a chord");
        assert!(copy.matches(0x43, true, false, false));
        assert!(
            !copy.matches(0x43, true, true, false),
            "an extra Shift is another chord"
        );
        assert!(
            !copy.matches(0x43, false, false, false),
            "the bare key belongs to the page"
        );
        assert!(!copy.matches(0x56, true, false, false));
    }

    #[test]
    fn the_hand_off_script_names_the_binding_it_calls() {
        assert!(HANDOFF_SCRIPT.contains(BINDING));
        assert!(HANDOFF_SCRIPT.contains("preventDefault"));
    }
}
