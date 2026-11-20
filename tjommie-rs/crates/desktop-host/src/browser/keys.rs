//! `BrowserInput` keys (DOM `key` names, as the phone and the desktop UI send them) to CDP key events.

use super::cdp::{DispatchKeyParams, KeyEventType};
use protocol::browser::KeyAction;

/// A key's CDP fields: the DOM `code` and the Windows virtual-key code.
struct Named {
    key: &'static str,
    code: &'static str,
    vk: i32,
}

const NAMED: &[Named] = &[
    Named {
        key: "Enter",
        code: "Enter",
        vk: 13,
    },
    Named {
        key: "Backspace",
        code: "Backspace",
        vk: 8,
    },
    Named {
        key: "Tab",
        code: "Tab",
        vk: 9,
    },
    Named {
        key: "Escape",
        code: "Escape",
        vk: 27,
    },
    Named {
        key: " ",
        code: "Space",
        vk: 32,
    },
    Named {
        key: "ArrowLeft",
        code: "ArrowLeft",
        vk: 37,
    },
    Named {
        key: "ArrowUp",
        code: "ArrowUp",
        vk: 38,
    },
    Named {
        key: "ArrowRight",
        code: "ArrowRight",
        vk: 39,
    },
    Named {
        key: "ArrowDown",
        code: "ArrowDown",
        vk: 40,
    },
    Named {
        key: "Delete",
        code: "Delete",
        vk: 46,
    },
    Named {
        key: "Insert",
        code: "Insert",
        vk: 45,
    },
    Named {
        key: "Home",
        code: "Home",
        vk: 36,
    },
    Named {
        key: "End",
        code: "End",
        vk: 35,
    },
    Named {
        key: "PageUp",
        code: "PageUp",
        vk: 33,
    },
    Named {
        key: "PageDown",
        code: "PageDown",
        vk: 34,
    },
    Named {
        key: "Shift",
        code: "ShiftLeft",
        vk: 16,
    },
    Named {
        key: "Control",
        code: "ControlLeft",
        vk: 17,
    },
    Named {
        key: "Alt",
        code: "AltLeft",
        vk: 18,
    },
    Named {
        key: "Meta",
        code: "MetaLeft",
        vk: 91,
    },
    Named {
        key: "F1",
        code: "F1",
        vk: 112,
    },
    Named {
        key: "F2",
        code: "F2",
        vk: 113,
    },
    Named {
        key: "F3",
        code: "F3",
        vk: 114,
    },
    Named {
        key: "F4",
        code: "F4",
        vk: 115,
    },
    Named {
        key: "F5",
        code: "F5",
        vk: 116,
    },
    Named {
        key: "F6",
        code: "F6",
        vk: 117,
    },
    Named {
        key: "F7",
        code: "F7",
        vk: 118,
    },
    Named {
        key: "F8",
        code: "F8",
        vk: 119,
    },
    Named {
        key: "F9",
        code: "F9",
        vk: 120,
    },
    Named {
        key: "F10",
        code: "F10",
        vk: 121,
    },
    Named {
        key: "F11",
        code: "F11",
        vk: 122,
    },
    Named {
        key: "F12",
        code: "F12",
        vk: 123,
    },
];

/// Modifier bits in a CDP event: alt 1, control 2, meta 4, shift 8.
pub fn modifier_bit(key: &str) -> u32 {
    match key {
        "Alt" => 1,
        "Control" => 2,
        "Meta" => 4,
        "Shift" => 8,
        _ => 0,
    }
}

/// The CDP event for `key` going down or up with `modifiers` held. A single printable character types itself unless
/// control, alt or meta are held (then it is a shortcut, which types nothing).
pub fn key_event(key: &str, action: KeyAction, modifiers: u32) -> DispatchKeyParams {
    let mut chars = key.chars();
    let single = match (chars.next(), chars.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    };
    let (code, vk) = match NAMED.iter().find(|named| named.key == key) {
        Some(named) => (named.code.to_string(), named.vk),
        None => match single {
            Some(c) if c.is_ascii_alphabetic() => (
                format!("Key{}", c.to_ascii_uppercase()),
                i32::from(c.to_ascii_uppercase() as u8),
            ),
            Some(c) if c.is_ascii_digit() => (format!("Digit{c}"), i32::from(c as u8)),
            _ => (String::new(), 0),
        },
    };
    let types_text = action == KeyAction::Down && single.is_some() && modifiers & (1 | 2 | 4) == 0;
    let kind = match action {
        KeyAction::Up => KeyEventType::KeyUp,
        KeyAction::Down if types_text => KeyEventType::KeyDown,
        KeyAction::Down => KeyEventType::RawKeyDown,
    };
    DispatchKeyParams {
        kind,
        modifiers,
        key: key.to_string(),
        code,
        windows_virtual_key_code: vk,
        text: types_text.then(|| key.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_digits_and_named_keys_map_to_their_codes() {
        let a = key_event("a", KeyAction::Down, 0);
        assert_eq!(
            (
                a.code.as_str(),
                a.windows_virtual_key_code,
                a.text.as_deref()
            ),
            ("KeyA", 65, Some("a"))
        );
        assert_eq!(a.kind, KeyEventType::KeyDown);
        let seven = key_event("7", KeyAction::Down, 0);
        assert_eq!(
            (seven.code.as_str(), seven.windows_virtual_key_code),
            ("Digit7", 55)
        );
        let left = key_event("ArrowLeft", KeyAction::Down, 0);
        assert_eq!(
            (left.code.as_str(), left.windows_virtual_key_code, left.text),
            ("ArrowLeft", 37, None)
        );
        assert_eq!(left.kind, KeyEventType::RawKeyDown);
        let space = key_event(" ", KeyAction::Down, 0);
        assert_eq!(
            (
                space.code.as_str(),
                space.windows_virtual_key_code,
                space.text.as_deref()
            ),
            ("Space", 32, Some(" "))
        );
    }

    #[test]
    fn a_shortcut_types_nothing_and_a_release_is_a_plain_key_up() {
        let copy = key_event("c", KeyAction::Down, modifier_bit("Control"));
        assert_eq!(copy.kind, KeyEventType::RawKeyDown);
        assert_eq!(copy.text, None);
        assert_eq!(copy.modifiers, 2);
        let shifted = key_event("C", KeyAction::Down, modifier_bit("Shift"));
        assert_eq!(shifted.text.as_deref(), Some("C"));
        let up = key_event("a", KeyAction::Up, 0);
        assert_eq!((up.kind, up.text), (KeyEventType::KeyUp, None));
    }

    #[test]
    fn modifier_keys_have_bits_and_unknown_keys_still_produce_an_event() {
        assert_eq!(
            ["Alt", "Control", "Meta", "Shift", "x"].map(modifier_bit),
            [1, 2, 4, 8, 0]
        );
        let odd = key_event("é", KeyAction::Down, 0);
        assert_eq!(
            (
                odd.code.as_str(),
                odd.windows_virtual_key_code,
                odd.text.as_deref()
            ),
            ("", 0, Some("é"))
        );
    }
}
