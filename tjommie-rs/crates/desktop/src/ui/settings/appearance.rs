//! Appearance in Settings (Phase 3 plan T5.5; `settings/appearance-section.tsx`): the theme, the text size and the
//! spacing, shown at once and saved on this device (`desktop-ui.json`). The phone's style and Reduce effects are
//! Android's.

use std::sync::Arc;

use desktop_host::device_settings::{Density, TextSize, Theme as Mode};
use gpui::{AnyElement, App, AppContext as _, IntoElement, ParentElement as _, Window};

use crate::ui::fields::{Look, RadioGroup};
use crate::ui::primitives::settings_row::SettingsRow;
use crate::ui::style::{Appearance, appearance, show_appearance};

const MODES: [(Mode, &str, &str); 3] = [
    (Mode::Dark, "dark", "Dark"),
    (Mode::Light, "light", "Light"),
    (Mode::System, "system", "System"),
];
const TEXT: [(TextSize, &str, &str); 3] = [
    (TextSize::System, "system", "System"),
    (TextSize::Large, "large", "Large"),
    (TextSize::Larger, "larger", "Larger"),
];
const SPACING: [(Density, &str, &str); 2] = [
    (Density::Comfortable, "comfortable", "Comfortable"),
    (Density::Compact, "compact", "Compact"),
];

/// Shows a choice at once and saves it on this device, off the UI thread. A write that fails is logged and the choice
/// kept for this run, so the TypeScript's "could not save" never shows.
fn choose(next: Appearance, cx: &mut App) {
    if let Some(file) = cx.try_global::<crate::live::DeviceFile>() {
        let file = Arc::clone(&file.0);
        cx.background_spawn(async move {
            file.update(|stored| {
                stored.theme = next.mode;
                stored.text_size = next.text;
                stored.density = next.density;
            });
        })
        .detach();
    }
    show_appearance(next, cx);
}

/// A segmented choice; `change` puts the chosen value into the appearance shown.
fn group<V: Copy + PartialEq + 'static>(
    id: &'static str,
    label: &'static str,
    value: V,
    choices: &[(V, &'static str, &'static str)],
    change: fn(Appearance, V) -> Appearance,
) -> AnyElement {
    choices
        .iter()
        .fold(
            RadioGroup::new(id, label, value).look(Look::Segmented),
            |group, (value, key, text)| group.choice(*value, *key, *text),
        )
        .on_choose(move |next, _, cx| choose(change(appearance(cx), next), cx))
        .into_any_element()
}

pub(super) fn section(window: &mut Window, cx: &mut App) -> AnyElement {
    let _ = window;
    let now = appearance(cx);
    super::frame(
        "appearance",
        "Appearance",
        "Saved on this device. Switching PCs keeps your appearance choices.",
        cx,
    )
    .child(
        super::card(cx)
            .child(SettingsRow::new("Theme").control(group(
                "settings.appearance.theme",
                "Theme",
                now.mode,
                &MODES,
                |shown, mode| Appearance { mode, ..shown },
            )))
            .child(
                SettingsRow::new("Text size")
                    .description(
                        "System follows your device's default text size. Larger choices also give controls more room.",
                    )
                    .control(group(
                        "settings.appearance.text",
                        "Text size",
                        now.text,
                        &TEXT,
                        |shown, text| Appearance { text, ..shown },
                    ))
                    .ruled(),
            )
            .child(
                SettingsRow::new("Spacing")
                    .description(
                        "Compact brings cards and settings rows closer together. Phone touch targets keep their size.",
                    )
                    .control(group(
                        "settings.appearance.spacing",
                        "Spacing",
                        now.density,
                        &SPACING,
                        |shown, density| Appearance { density, ..shown },
                    ))
                    .ruled(),
            ),
    )
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use desktop_host::device_settings::{Density, TextSize, Theme as Mode};
    use gpui::TestAppContext;

    use crate::model::testing::FakeRpc;
    use crate::ui::shell::route::{Route, Section};
    use crate::ui::style::{appearance, palette};
    use crate::ui::testing::{app, click, node, state};

    #[gpui::test]
    fn the_choices_show_at_once_light_dark_text_size_and_spacing(cx: &mut TestAppContext) {
        let (cx, _) = app(
            cx,
            Route::Settings(Section::Appearance),
            state(vec![], vec![], vec![]),
            FakeRpc::new(),
        );
        let colors = |cx: &mut gpui::VisualTestContext| {
            cx.update(|_, cx| {
                let theme = crate::ui::style::theme(cx);
                (palette(cx).surface.frame, theme.color.dark.surface.frame)
            })
        };
        click(cx, "settings.appearance.theme.light");
        assert!(node("settings.appearance.theme.light").unwrap().toggled);
        let (shown, dark) = colors(cx);
        assert_ne!(shown, dark, "light mode shows the light colours");
        click(cx, "settings.appearance.theme.dark");
        let (shown, dark) = colors(cx);
        assert_eq!(shown, dark, "dark mode shows the dark ones");
        click(cx, "settings.appearance.theme.light");
        let (shown, dark) = colors(cx);
        assert_ne!(shown, dark, "and light again");
        assert_eq!(cx.update(|_, cx| appearance(cx).mode), Mode::Light);

        let rem = |cx: &mut gpui::VisualTestContext| {
            cx.update(|_, cx| f32::from(gpui_kit::component::Theme::global(cx).font_size))
        };
        let system = rem(cx);
        click(cx, "settings.appearance.text.larger");
        assert!((rem(cx) - system * 1.25).abs() < 0.01, "Larger is 125 %");
        click(cx, "settings.appearance.text.system");
        assert!((rem(cx) - system).abs() < 0.01);
        assert_eq!(cx.update(|_, cx| appearance(cx).text), TextSize::System);

        click(cx, "settings.appearance.spacing.compact");
        assert_eq!(cx.update(|_, cx| appearance(cx).density), Density::Compact);
        assert!(node("settings.appearance.spacing.compact").unwrap().toggled);
    }
}
