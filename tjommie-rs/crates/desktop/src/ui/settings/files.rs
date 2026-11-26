//! Files in Settings (Phase 3 plan T5.5; `settings/device-settings.tsx`'s `FilesSection`), the desktop branch. Files
//! are previewed and saved (T4.4), and Clear temporary data stops the downloads that haven't finished.

use gpui::{
    AnyElement, App, IntoElement, ParentElement as _, Role, StatefulInteractiveElement as _,
    Styled as _, Window, div,
};
use protocol::attachments::{ATTACHMENT_CHUNK_BYTES, MAX_ATTACHMENT_BYTES};

use crate::guard::guarded;
use crate::semantic::SemanticExt as _;
use crate::ui::files::card;
use crate::ui::primitives::button::Pill;
use crate::ui::primitives::settings_row::SettingsRow;
use crate::ui::style::{TypeExt as _, hsla, palette, theme};

const TEMPORARY: &str = "Cancels unfinished downloads on this device. Saved downloads, delivered host files, chats, \
                         accounts and worktrees are kept.";

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
    let cleared = window.use_keyed_state("settings.files.cleared", cx, |_, _| false);
    let limits = format!(
        "Files up to {} MiB; {} KiB chunks. The host limits concurrent transfers.",
        MAX_ATTACHMENT_BYTES / 1_048_576,
        ATTACHMENT_CHUNK_BYTES / 1024
    );
    let clear = Pill::new("settings.files.clear", "Clear temporary data")
        .small()
        .on_press({
            let cleared = cleared.clone();
            move |_, cx| {
                guarded("clear temporary file data", || {
                    card::cancel_downloads(cx);
                    cleared.update(cx, |cleared, cx| {
                        *cleared = true;
                        cx.notify();
                    });
                });
            }
        });
    let notice = (*cleared.read(cx)).then(|| {
        value(
            "settings.files.notice",
            "Temporary file data cleared on this device.",
            cx,
        )
    });
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
                    .control(value("settings.files.support", "Available", cx))
                    .ruled(),
            )
            .child(SettingsRow::new("Transfer limits").description(limits).ruled())
            .child(
                SettingsRow::new("Preview limits")
                    .description(
                        "Markdown opens in 32 KiB sections. PDF reads use ranges up to 256 KiB. Previews have no search, \
                         and password-protected PDFs and animated GIF or WebP images aren't previewed. Unsupported \
                         previews still offer Download. Codec support depends on this device.",
                    )
                    .ruled(),
            )
            .child(
                SettingsRow::new("Temporary file data")
                    .description(TEMPORARY)
                    .control(clear)
                    .ruled(),
            ),
    )
    .children(notice)
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use crate::model::document_ranges::Signal;
    use crate::model::testing::FakeRpc;
    use crate::ui::files::card::{self, FileKey};
    use crate::ui::shell::route::{Navigate as _, Route, Section};
    use crate::ui::testing::{app, click, draw, node, state, text};

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
        assert_eq!(text("settings.files.support"), "Available");
        assert!(node("settings.files.clear").is_some_and(|clear| clear.enabled));
        cx.update(|_, cx| cx.navigate(Route::Settings(Section::General)));
        draw(cx);
        assert_eq!(
            text("settings.general.updates"),
            "Unavailable in this app version."
        );
        cx.update(|_, cx| cx.navigate(Route::Settings(Section::Notifications)));
        draw(cx);
        assert!(node("settings.notifications.title").is_some());
    }

    /// Clear temporary data stops a download that hasn't finished, and says it cleared.
    #[gpui::test]
    fn clear_temporary_data_cancels_unfinished_downloads(cx: &mut TestAppContext) {
        let (cx, _) = app(
            cx,
            Route::Settings(Section::Files),
            state(vec![], vec![], vec![]),
            FakeRpc::new(),
        );
        let signal = Signal::new();
        let key = FileKey {
            message_id: "m1".into(),
            index: 0,
        };
        cx.update(|_, cx| card::saving_for_test(&key, signal.clone(), cx));
        assert!(node("settings.files.notice").is_none());
        click(cx, "settings.files.clear");
        draw(cx);
        assert!(signal.aborted(), "the download is stopped");
        assert_eq!(
            text("settings.files.notice"),
            "Temporary file data cleared on this device."
        );
    }
}
