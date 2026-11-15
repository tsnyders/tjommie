//! Files in Settings (Phase 3 plan T5.5; `settings/device-settings.tsx`'s `FilesSection`), the desktop branch. The file
//! session that Clear temporary data clears (the TypeScript's `attachmentDevice()`) comes with the Chat lane's files
//! (T4.4); until then File support says it is unavailable and the button is off, as the TypeScript shows a device with
//! no file session.

use gpui::{
    AnyElement, App, IntoElement, ParentElement as _, Role, StatefulInteractiveElement as _, Styled as _,
    Window, div,
};
use protocol::attachments::{ATTACHMENT_CHUNK_BYTES, MAX_ATTACHMENT_BYTES};

use crate::semantic::SemanticExt as _;
use crate::ui::primitives::button::Pill;
use crate::ui::primitives::settings_row::SettingsRow;
use crate::ui::style::{TypeExt as _, hsla, palette, theme};

const TEMPORARY: &str = "Closes previews and cancels unfinished downloads on this device. Saved downloads, delivered \
                         host files, chats, accounts and worktrees are kept.";

/// A value at a row's right, in secondary text.
fn value(id: &'static str, text: &'static str, cx: &App) -> AnyElement {
    div()
        .semantic(id, Role::Paragraph)
        .aria_label(text)
        .type_style(&theme(cx).typography.desktop.ui)
        .text_color(hsla(palette(cx).text.secondary))
        .child(text)
        .into_any_element()
}

pub(super) fn section(window: &mut Window, cx: &mut App) -> AnyElement {
    let _ = window;
    let limits = format!(
        "Files up to {} MiB; {} KiB chunks. The host limits concurrent transfers.",
        MAX_ATTACHMENT_BYTES / 1_048_576,
        ATTACHMENT_CHUNK_BYTES / 1024
    );
    super::frame(
        "files",
        "Files",
        "Previews and saves use this device. Files are read from the selected host through your existing connection.",
        cx,
    )
    .child(
        super::card(cx)
            .child(
                SettingsRow::new("Download location")
                    .description(
                        "Ask where to save each file, using Save As or Android's system document picker. No folder \
                         permission is needed in advance.",
                    )
                    .control(value("settings.files.location", "Ask each time", cx)),
            )
            .child(
                SettingsRow::new("File support")
                    .description("Open shared files and save a copy on this device.")
                    .control(value("settings.files.support", "Unavailable in this app", cx))
                    .ruled(),
            )
            .child(SettingsRow::new("Transfer limits").description(limits).ruled())
            .child(
                SettingsRow::new("Preview limits")
                    .description(
                        "Markdown opens in 32 KiB sections. PDF reads use ranges up to 256 KiB. Unsupported previews \
                         still offer Download. Codec support depends on this device.",
                    )
                    .ruled(),
            )
            .child(
                SettingsRow::new("Temporary file data")
                    .description(TEMPORARY)
                    // Off: there is no file session to clear yet.
                    .control(
                        div()
                            .opacity(0.38)
                            .child(Pill::new("settings.files.clear", "Clear temporary data").small()),
                    )
                    .ruled(),
            ),
    )
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use crate::model::testing::FakeRpc;
    use crate::ui::shell::route::{Navigate as _, Route, Section};
    use crate::ui::testing::{app, draw, node, state, text};

    /// `device-settings.test.tsx`'s sections on the desktop: General, Notifications and Files say what this device does.
    #[gpui::test]
    fn general_notifications_and_files_say_what_this_device_does(cx: &mut TestAppContext) {
        let (cx, _) = app(
            cx,
            Route::Settings(Section::Files),
            state(vec![], vec![], vec![]),
            FakeRpc::new(),
        );
        assert_eq!(text("settings.files.location"), "Ask each time");
        assert_eq!(text("settings.files.support"), "Unavailable in this app");
        assert!(node("settings.files.clear").is_some());
        cx.update(|_, cx| cx.navigate(Route::Settings(Section::General)));
        draw(cx);
        assert_eq!(text("settings.general.updates"), "Unavailable in this app version.");
        cx.update(|_, cx| cx.navigate(Route::Settings(Section::Notifications)));
        draw(cx);
        assert!(node("settings.notifications.title").is_some());
    }
}
