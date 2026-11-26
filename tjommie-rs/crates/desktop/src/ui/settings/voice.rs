//! Voice in Settings (Phase 3 plan T5.5; `settings/voice-section.tsx`, row 14): this device's local dictation — its
//! engine, availability, microphone permission, language, input device and model — with Refresh voice status and the
//! model's download, cancel and removal. The status never records audio. Android's on-device branch (native capture,
//! SDK preparation) is the phone's. Capture records from the Windows default microphone (`desktop-host::speech`), so
//! the input device is shown, not chosen.

use gpui::{
    AnyElement, App, Entity, IntoElement, ParentElement as _, SharedString, Styled as _, Window,
    div, relative,
};
use protocol::speech::{SpeechCapture, SpeechEngine, SpeechModelState, SpeechStatus};

use super::diagnostics::value;
use crate::guard::guarded_async;
use crate::speech::{self, Speech, VoiceChoice};
use crate::ui::fields::problem;
use crate::ui::primitives::button::Pill;
use crate::ui::primitives::menu::{Menu, MenuItem};
use crate::ui::primitives::settings_row::SettingsRow;
use crate::ui::style::{TypeExt as _, hsla, palette, r, theme};

const NOT_FINISHED: &str = "The local speech action could not finish. Check this device's storage and network, then try again.";

#[derive(Clone, Copy, PartialEq)]
enum Action {
    Refresh,
    Download,
    Cancel,
    Remove,
}

/// The section's own state: the action under way (each has its number, so a late answer to an older one is fenced
/// off), and what the last one said.
#[derive(Default)]
struct Voice {
    asked: bool,
    working: Option<Action>,
    operation: u64,
    notice: Option<SharedString>,
    problem: Option<SharedString>,
}

/// A daemon-style enum's wire name ("granted", "missing").
fn named(value: &impl serde::Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

fn run(state: &Entity<Voice>, action: Action, cx: &mut App) {
    let Some(actions) = cx
        .try_global::<Speech>()
        .map(|speech| speech.actions.clone())
    else {
        return;
    };
    if state.read(cx).working.is_some() && action != Action::Cancel {
        return;
    }
    let operation = state.update(cx, |voice, cx| {
        voice.operation += 1;
        voice.working = Some(action);
        voice.notice = None;
        voice.problem = None;
        cx.notify();
        voice.operation
    });
    let state = state.downgrade();
    cx.spawn(async move |cx| {
        guarded_async("change the local speech model", async move {
            let done = match action {
                Action::Refresh => Ok(()),
                Action::Download => actions.download_model().await,
                Action::Cancel => actions.cancel_model_download().await,
                Action::Remove => actions.remove_model().await,
            };
            let next = actions.status().await;
            cx.update(|cx| {
                if let Some(next) = next.clone() {
                    speech::show(next, cx);
                }
                let _ = state.update(cx, |voice, cx| {
                    // A newer action has the section now.
                    if voice.operation != operation {
                        return;
                    }
                    voice.working = None;
                    match (done, next) {
                        (Ok(()), Some(_)) => {
                            voice.notice = Some(
                                match action {
                                    Action::Cancel => "Model download cancelled.",
                                    Action::Remove => "Downloaded model removed from this device.",
                                    Action::Refresh | Action::Download => {
                                        "Local speech status refreshed."
                                    }
                                }
                                .into(),
                            );
                        }
                        (_, next) => {
                            voice.problem = Some(
                                next.and_then(|status| {
                                    status.problem.map(|problem| problem.message)
                                })
                                .unwrap_or_else(|| NOT_FINISHED.to_owned())
                                .into(),
                            );
                        }
                    }
                    cx.notify();
                });
            });
        })
        .await;
    })
    .detach();
}

/// Reads the status once when the section first shows; a status that can't be read says so.
fn ask(state: &Entity<Voice>, cx: &mut App) {
    let Some(actions) = cx
        .try_global::<Speech>()
        .map(|speech| speech.actions.clone())
    else {
        return;
    };
    state.update(cx, |voice, _| voice.asked = true);
    let state = state.downgrade();
    cx.spawn(async move |cx| {
        guarded_async("read the local speech status", async move {
            let status = actions.status().await;
            cx.update(|cx| match status {
                Some(status) => speech::show(status, cx),
                None => {
                    let _ = state.update(cx, |voice, cx| {
                        voice.problem =
                            Some("Local speech status is unavailable in this app.".into());
                        cx.notify();
                    });
                }
            });
        })
        .await;
    })
    .detach();
}

/// The model's line: its state, and how far a download has come.
fn model_line(status: Option<&SpeechStatus>) -> String {
    match status {
        None => "Unknown".to_owned(),
        Some(status) => {
            let model = &status.model;
            let state = named(&model.state);
            if matches!(
                model.state,
                SpeechModelState::Downloading | SpeechModelState::Verifying
            ) {
                let percent = (model.received.saturating_mul(100) / model.bytes.max(1)).min(100);
                format!("{state} · {percent}%")
            } else {
                state
            }
        }
    }
}

/// A picker for the language or the microphone: a pill with the choice that opens a menu of them, named for what it
/// picks (`what`, M5 review Minor 25).
fn picker(
    id: &'static str,
    what: &'static str,
    label: String,
    choices: Vec<(String, String)>,
    chosen: String,
    pick: fn(&mut VoiceChoice, String),
) -> AnyElement {
    let name = format!("{what}: {label}");
    if choices.is_empty() {
        return Pill::new(id, label)
            .name(name)
            .small()
            .disabled(true)
            .into_any_element();
    }
    choices
        .into_iter()
        .fold(
            Menu::new(
                SharedString::from(format!("{id}.menu")),
                move |open, _, _| {
                    Pill::new(id, label.clone())
                        .name(name.clone())
                        .small()
                        .icon("chevron-down")
                        .expanded(open)
                        .into_any_element()
                },
            )
            .anchor(gpui::Anchor::TopRight)
            .offset(gpui::px(6.)),
            |menu, (key, text)| {
                let picked = key.clone();
                menu.item(
                    MenuItem::new(
                        format!(
                            "{id}.{}",
                            if key.is_empty() {
                                "default"
                            } else {
                                key.as_str()
                            }
                        ),
                        text,
                        move |_, cx| {
                            let mut choice = speech::choice(cx);
                            pick(&mut choice, picked.clone());
                            cx.set_global(choice);
                            cx.refresh_windows();
                        },
                    )
                    .checked(key == chosen),
                )
            },
        )
        .into_any_element()
}

pub(super) fn section(window: &mut Window, cx: &mut App) -> AnyElement {
    let state = window.use_keyed_state("settings.voice.state", cx, |_, _| Voice::default());
    let api = cx.has_global::<Speech>();
    if api && !state.read(cx).asked {
        ask(&state, cx);
    }
    let status = cx
        .try_global::<Speech>()
        .and_then(|speech| speech.status.clone());
    let choice = speech::choice(cx);
    let (working, notice, failed) = {
        let voice = state.read(cx);
        (voice.working, voice.notice.clone(), voice.problem.clone())
    };
    let languages: Vec<(String, String)> = status
        .as_ref()
        .map(|status| {
            status
                .languages
                .iter()
                .map(|language| (language.tag.clone(), language.label.clone()))
                .collect()
        })
        .unwrap_or_default();
    let selected = if languages.iter().any(|(tag, _)| *tag == choice.language) {
        choice.language.clone()
    } else {
        languages
            .first()
            .map(|(tag, _)| tag.clone())
            .unwrap_or_default()
    };
    let language_label = languages
        .iter()
        .find(|(tag, _)| *tag == selected)
        .map_or_else(
            || "No supported language available".to_owned(),
            |(_, label)| label.clone(),
        );
    let pcm = status
        .as_ref()
        .is_some_and(|status| status.capture == SpeechCapture::Pcm);
    let state_now = status.as_ref().map(|status| status.model.state);
    let downloading = matches!(
        state_now,
        Some(SpeechModelState::Downloading | SpeechModelState::Verifying)
    );
    let runtime_missing = status
        .as_ref()
        .and_then(|status| status.problem.as_ref())
        .is_some_and(|problem| problem.code == "runtime_unavailable");
    let action = |id: &'static str, label: &'static str, enabled: bool, what: Action| {
        let state = state.clone();
        Pill::new(id, label)
            .small()
            .disabled(!enabled)
            .on_press(move |_, cx| run(&state, what, cx))
            .into_any_element()
    };
    let mut buttons = vec![action(
        "settings.voice.refresh",
        "Refresh voice status",
        api && working.is_none(),
        Action::Refresh,
    )];
    if downloading {
        buttons.push(action(
            "settings.voice.cancel",
            "Cancel model download",
            working != Some(Action::Cancel),
            Action::Cancel,
        ));
    }
    if pcm && !downloading && !runtime_missing && state_now != Some(SpeechModelState::Ready) {
        buttons.push(action(
            "settings.voice.download",
            "Download English model",
            working.is_none(),
            Action::Download,
        ));
    }
    if pcm && state_now == Some(SpeechModelState::Ready) {
        buttons.push(action(
            "settings.voice.remove",
            "Remove downloaded model",
            working.is_none(),
            Action::Remove,
        ));
    }
    let theme = theme(cx);
    let colors = palette(cx);
    let text = |id: &'static str, said: &'static str| {
        div()
            .mt(r(theme.space.s12))
            .child(value(id, said, cx))
            .into_any_element()
    };
    let mut card = super::card(cx)
        .child(SettingsRow::new("Local engine").control(value(
            "settings.voice.engine",
            match status.as_ref().map(|status| status.engine) {
                Some(SpeechEngine::WhisperCpp) => "whisper.cpp on this device",
                Some(SpeechEngine::AndroidOnDevice) => "Android on-device recognition",
                None => "Unavailable or not checked",
            },
            cx,
        )))
        .child(
            SettingsRow::new("Dictation availability")
                .control(value(
                    "settings.voice.availability",
                    match &status {
                        Some(status) if status.model.confirmation.is_some() => {
                            "Prepared · Start confirms before recording"
                        }
                        Some(status) if status.available => "Ready",
                        Some(_) => "Unavailable",
                        None => "Unknown",
                    },
                    cx,
                ))
                .ruled(),
        )
        .child(
            SettingsRow::new("Microphone permission")
                .description(
                    "Tjommie reads Windows' microphone privacy settings and never asks for access itself. Windows has to \
                     let desktop apps use the microphone, in Windows Settings, then Privacy & security, then Microphone.",
                )
                .control(value(
                    "settings.voice.permission",
                    status
                        .as_ref()
                        .map_or_else(|| "Unknown".to_owned(), |status| named(&status.permission)),
                    cx,
                ))
                .ruled(),
        )
        .child(
            SettingsRow::new("Language")
                .description("Only languages reported by the local engine are listed.")
                .control(picker(
                    "settings.voice.language",
                    "Dictation language",
                    language_label,
                    languages,
                    selected,
                    |choice, picked| choice.language = picked,
                ))
                .ruled(),
        );
    if pcm {
        card = card.child(
            SettingsRow::new("Input device")
                .description(
                    "Dictation records from the Windows default microphone. Change it in Windows Settings, under Sound.",
                )
                .control(value("settings.voice.device", "Windows default", cx))
                .ruled(),
        );
    }
    let model_note = match status.as_ref().map(|status| status.model.bytes) {
        Some(bytes) if bytes > 0 => format!(
            "{} MiB on this device. Downloading a model does not start recording.",
            bytes.div_ceil(1_048_576)
        ),
        _ => "No local model information is available.".to_owned(),
    };
    card = card.child(
        SettingsRow::new("Local model")
            .description(model_note)
            .control(value(
                "settings.voice.model",
                model_line(status.as_ref()),
                cx,
            ))
            .ruled(),
    );
    if let Some(trouble) = status.as_ref().and_then(|status| status.problem.as_ref()) {
        card = card.child(
            SettingsRow::new("On-device status")
                // At most 60% of the row, so a long message wraps there and the label keeps its room.
                .control(div().min_w_0().max_w(relative(0.6)).child(value(
                    "settings.voice.status_problem",
                    trouble.message.clone(),
                    cx,
                )))
                .ruled(),
        );
    }
    let unavailable = status.as_ref().is_some_and(|status| !status.available);
    super::frame(
        "voice",
        "Voice",
        "Dictation runs locally on the device capturing your voice, even when the host PC is offline. Transcripts stay \
         editable until you choose Send.",
        cx,
    )
    .child(card)
    .child(div().mt(r(theme.space.s16)).flex().flex_wrap().gap(r(theme.space.s8)).children(buttons))
    .children((!api).then(|| {
        text(
            "settings.voice.no_api",
            "Local dictation is unavailable in this app. Typing remains available; there is no cloud fallback.",
        )
    }))
    .children(unavailable.then(|| {
        text(
            "settings.voice.needs",
            "A supported local engine, verified or installed model, and microphone permission are needed. No online \
             recognizer is substituted.",
        )
    }))
    .children(notice.map(|notice| {
        div()
            .mt(r(theme.space.s12))
            .type_style(&theme.typography.desktop.ui)
            .text_color(hsla(colors.text.secondary))
            .child(value("settings.voice.notice", notice, cx))
    }))
    .children(failed.map(|said| problem("settings.voice.problem", said, cx)))
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use futures::FutureExt as _;
    use futures::future::BoxFuture;
    use gpui::TestAppContext;
    use protocol::speech::{SpeechModelState, SpeechStatus};
    use serde_json::json;

    use crate::model::testing::FakeRpc;
    use crate::speech::{Speech, SpeechActions};
    use crate::ui::shell::route::{Route, Section};
    use crate::ui::testing::{app, click, draw, node, state, text};

    /// A speech runtime that only says its status and records what it was asked.
    struct FakeSpeech {
        status: Mutex<SpeechStatus>,
        asked: Mutex<Vec<&'static str>>,
        /// A download that fails, as one cancelled midway does.
        download_fails: bool,
    }

    fn status(state: &str, available: bool, problem: Option<(&str, &str)>) -> SpeechStatus {
        let mut status = json!({
            "engine": "whisper.cpp", "capture": "pcm", "available": available,
            "languages": [{ "tag": "en", "label": "English" }], "permission": "granted",
            "model": { "state": state, "bytes": 147_951_464, "received": 73_975_732 }
        });
        if let Some((code, message)) = problem {
            status["problem"] = json!({ "code": code, "message": message });
        }
        serde_json::from_value(status).unwrap()
    }

    impl FakeSpeech {
        fn set(&self, state: SpeechModelState) {
            self.status.lock().unwrap().model.state = state;
        }
    }

    impl SpeechActions for FakeSpeech {
        fn status(&self) -> BoxFuture<'static, Option<SpeechStatus>> {
            self.asked.lock().unwrap().push("status");
            let status = self.status.lock().unwrap().clone();
            async move { Some(status) }.boxed()
        }
        fn download_model(&self) -> BoxFuture<'static, Result<(), String>> {
            self.asked.lock().unwrap().push("download");
            let result = if self.download_fails {
                Err("cancelled".to_owned())
            } else {
                self.set(SpeechModelState::Ready);
                Ok(())
            };
            async move { result }.boxed()
        }
        fn cancel_model_download(&self) -> BoxFuture<'static, Result<(), String>> {
            self.asked.lock().unwrap().push("cancel");
            self.set(SpeechModelState::Missing);
            async { Ok(()) }.boxed()
        }
        fn remove_model(&self) -> BoxFuture<'static, Result<(), String>> {
            self.asked.lock().unwrap().push("remove");
            self.set(SpeechModelState::Missing);
            async { Ok(()) }.boxed()
        }
    }

    fn with_speech(cx: &mut gpui::VisualTestContext, fake: Arc<FakeSpeech>) {
        cx.update(|_, cx| {
            cx.set_global(Speech {
                actions: fake,
                status: None,
            });
            cx.refresh_windows();
        });
        draw(cx);
        draw(cx);
    }

    fn fake(status: SpeechStatus, download_fails: bool) -> Arc<FakeSpeech> {
        Arc::new(FakeSpeech {
            status: Mutex::new(status),
            asked: Mutex::new(Vec::new()),
            download_fails,
        })
    }

    /// voice-section.test.tsx: "degrades honestly without the current-device bridge".
    #[gpui::test]
    fn degrades_honestly_without_local_speech(cx: &mut TestAppContext) {
        let (_, _) = app(
            cx,
            Route::Settings(Section::Voice),
            state(vec![], vec![], vec![]),
            FakeRpc::new(),
        );
        assert_eq!(text("settings.voice.engine"), "Unavailable or not checked");
        assert_eq!(text("settings.voice.availability"), "Unknown");
        assert!(text("settings.voice.no_api").contains("there is no cloud fallback"));
        assert!(node("settings.voice.download").is_none());
    }

    /// voice-section.test.tsx: "reads status without recording and persists supported language choices on this device".
    #[gpui::test]
    fn reads_the_status_and_keeps_the_language_chosen(cx: &mut TestAppContext) {
        let (cx, _) = app(
            cx,
            Route::Settings(Section::Voice),
            state(vec![], vec![], vec![]),
            FakeRpc::new(),
        );
        let speech = fake(status("ready", true, None), false);
        with_speech(cx, Arc::clone(&speech));
        assert_eq!(text("settings.voice.engine"), "whisper.cpp on this device");
        assert_eq!(text("settings.voice.availability"), "Ready");
        assert_eq!(text("settings.voice.permission"), "granted");
        assert_eq!(text("settings.voice.model"), "ready");
        assert_eq!(
            *speech.asked.lock().unwrap(),
            ["status"],
            "only the status is read"
        );
        click(cx, "settings.voice.language");
        click(cx, "settings.voice.language.en");
        assert_eq!(cx.update(|_, cx| crate::speech::choice(cx).language), "en");
    }

    /// voice-section.test.tsx: "does not enumerate microphone names or ask permission until access was already
    /// granted". The desktop records from the Windows default microphone, so it shows that and offers no choice.
    #[gpui::test]
    fn never_lists_microphones_or_asks_for_access(cx: &mut TestAppContext) {
        let (cx, _) = app(
            cx,
            Route::Settings(Section::Voice),
            state(vec![], vec![], vec![]),
            FakeRpc::new(),
        );
        let mut prompt = status("missing", false, None);
        prompt.permission = protocol::speech::SpeechPermission::Prompt;
        let speech = fake(prompt, false);
        with_speech(cx, Arc::clone(&speech));
        assert_eq!(text("settings.voice.device"), "Windows default");
        assert!(
            node("settings.voice.microphones").is_none(),
            "no list of microphones to refresh"
        );
        assert_eq!(*speech.asked.lock().unwrap(), ["status"]);
    }

    /// voice-section.test.tsx: "cancels a download and fences its late failure", and "removes only the downloaded
    /// device model" (the desktop half).
    #[gpui::test]
    fn downloads_cancels_and_removes_the_model(cx: &mut TestAppContext) {
        let (cx, _) = app(
            cx,
            Route::Settings(Section::Voice),
            state(vec![], vec![], vec![]),
            FakeRpc::new(),
        );
        let speech = fake(status("downloading", false, None), true);
        with_speech(cx, Arc::clone(&speech));
        assert_eq!(text("settings.voice.model"), "downloading · 50%");
        assert!(
            node("settings.voice.download").is_none(),
            "not while downloading"
        );
        click(cx, "settings.voice.cancel");
        draw(cx);
        assert_eq!(text("settings.voice.notice"), "Model download cancelled.");
        assert_eq!(text("settings.voice.model"), "missing");
        // The download that fails says why, from the status.
        click(cx, "settings.voice.download");
        draw(cx);
        assert!(node("settings.voice.problem").is_some());
        speech.set(SpeechModelState::Ready);
        click(cx, "settings.voice.refresh");
        draw(cx);
        assert_eq!(
            text("settings.voice.notice"),
            "Local speech status refreshed."
        );
        click(cx, "settings.voice.remove");
        draw(cx);
        assert_eq!(
            text("settings.voice.notice"),
            "Downloaded model removed from this device."
        );
        assert!(speech.asked.lock().unwrap().contains(&"remove"));
    }

    /// A missing runtime: no download to offer, and the reason shown.
    #[gpui::test]
    fn a_missing_runtime_offers_no_download(cx: &mut TestAppContext) {
        let (cx, _) = app(
            cx,
            Route::Settings(Section::Voice),
            state(vec![], vec![], vec![]),
            FakeRpc::new(),
        );
        let speech = fake(
            status(
                "missing",
                false,
                Some(("runtime_unavailable", "Local dictation is not installed.")),
            ),
            false,
        );
        with_speech(cx, speech);
        assert!(node("settings.voice.download").is_none());
        assert_eq!(
            text("settings.voice.status_problem"),
            "Local dictation is not installed."
        );
        assert!(text("settings.voice.needs").contains("No online recognizer is substituted."));
    }
}
