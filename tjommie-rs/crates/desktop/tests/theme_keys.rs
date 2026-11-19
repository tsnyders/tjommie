//! The theme files (spec Phase 0): every theme has Aegis's key set, and `Theme` reads Aegis exactly.
// Tests may unwrap (Phase 3 plan T0.4); clippy.toml covers #[test] functions, this covers the helpers.
#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::path::Path;

use desktop::theme::{AEGIS, Theme};

/// Every leaf path of a JSON document. Objects are walked; anything else, arrays included, is a leaf.
fn leaves(value: &serde_json::Value, path: &str, out: &mut BTreeSet<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                leaves(child, &child_path, out);
            }
        }
        _ => {
            out.insert(path.to_owned());
        }
    }
}

fn key_set(text: &str) -> BTreeSet<String> {
    let value: serde_json::Value = serde_json::from_str(text).unwrap();
    let mut out = BTreeSet::new();
    leaves(&value, "", &mut out);
    out
}

#[test]
fn every_theme_has_the_aegis_key_set() {
    let aegis = key_set(AEGIS);
    let themes = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../design/themes");
    let mut checked = 0;
    for entry in std::fs::read_dir(&themes).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let theme = key_set(&std::fs::read_to_string(&path).unwrap());
            assert_eq!(
                theme,
                aegis,
                "{} has a different key set from aegis.json",
                path.display()
            );
            checked += 1;
        }
    }
    assert!(checked >= 1, "no theme files in {}", themes.display());
}

#[test]
fn theme_reads_aegis_and_refuses_unknown_keys() {
    if let Err(error) = Theme::aegis() {
        panic!("aegis.json does not match Theme: {error}");
    }
    let mut extra: serde_json::Value = serde_json::from_str(AEGIS).unwrap();
    extra["motion"]["spring"]["calm"]["mass"] = 1.into();
    assert!(
        serde_json::from_value::<Theme>(extra).is_err(),
        "an unknown key was accepted"
    );
}

#[test]
fn calm_spring_is_stiffness_260_and_damping_ratio_1() {
    let calm = Theme::aegis().unwrap().motion.spring.calm;
    assert_eq!((calm.stiffness, calm.damping_ratio), (260.0, 1.0));
}

/// WCAG 2's relative luminance of an opaque colour.
fn luminance(color: gpui::Rgba) -> f32 {
    let channel = |c: f32| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
}

fn contrast(a: gpui::Rgba, b: gpui::Rgba) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// M5 review M7: the danger text clears 4.5:1 on every surface it sits on (the Danger pill at rest and on hover, an
/// agent card's problem line), in both modes.
#[test]
fn danger_text_clears_4_5_to_1_where_it_sits() {
    let theme = Theme::aegis().unwrap();
    for (mode, palette) in [("light", &theme.color.light), ("dark", &theme.color.dark)] {
        for (name, surface) in [
            ("surface.sunk", palette.surface.sunk),
            ("state.sunkHover", palette.state.sunk_hover),
            ("surface.raised", palette.surface.raised),
        ] {
            let ratio = contrast(palette.danger.text, surface);
            assert!(ratio >= 4.5, "{mode} danger.text on {name} is {ratio:.2}:1");
        }
    }
}
