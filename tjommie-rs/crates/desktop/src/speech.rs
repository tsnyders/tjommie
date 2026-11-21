//! This device's local dictation, wired into the app (T6.5's runtime; the Screens lane's live wiring): whisper-cli
//! shipped beside the app in `resources\speech` (dictation is available only if its manifest verifies), the English
//! model in the identity's speech folder (`AppDirs::speech`: production keeps 0.5.x's, so no second download), and the
//! status Settings, Voice and the diagnostic report read, and dictation for the chat's composers (the microphone through
//! desktop-host's `Dictation`). The runtime is tokio's: its calls run on the live runtime, and its status and its
//! sessions' events come back to the UI thread through channels.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use desktop_host::app_dirs::{AppDirs, DirsEnv};
use desktop_host::identity::Identity;
use desktop_host::shell::registry;
use desktop_host::speech::{
    CpalSource, Dictation, RuntimeOptions, SpeechModel, SpeechRuntime, WhisperProcess,
    microphone_permission, packaged_speech_digest, pinned,
};
use desktop_host::updater::{Policy, ReqwestHttp};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::future::BoxFuture;
use futures::{FutureExt as _, StreamExt as _};
use gpui::{App, Global};
use protocol::speech::{SpeechEvent, SpeechPhase, SpeechProblem, SpeechStatus};

use crate::guard::guarded_async;
use crate::ui::chat::voice::{self, VoiceBackend};

/// What Settings, Voice asks of the speech runtime: its status, and the model's download, cancel and removal.
pub trait SpeechActions: Send + Sync {
    fn status(&self) -> BoxFuture<'static, Option<SpeechStatus>>;
    fn download_model(&self) -> BoxFuture<'static, Result<(), String>>;
    fn cancel_model_download(&self) -> BoxFuture<'static, Result<(), String>>;
    fn remove_model(&self) -> BoxFuture<'static, Result<(), String>>;
}

/// The app's speech: its actions, and the latest status (from a read or the runtime's own status events).
pub struct Speech {
    pub actions: Arc<dyn SpeechActions>,
    pub status: Option<SpeechStatus>,
}

impl Global for Speech {}

/// The latest status the app knows, as JSON for the diagnostic report (`Null` without speech).
pub fn status_value(cx: &App) -> serde_json::Value {
    cx.try_global::<Speech>()
        .and_then(|speech| speech.status.as_ref())
        .and_then(|status| serde_json::to_value(status).ok())
        .unwrap_or(serde_json::Value::Null)
}

const UNFINISHED: &str = "The local speech action could not finish.";

struct Live {
    runtime: SpeechRuntime,
    tokio: tokio::runtime::Handle,
}

impl Live {
    fn act(
        &self,
        work: impl FnOnce(SpeechRuntime) -> BoxFuture<'static, Result<(), String>>,
    ) -> BoxFuture<'static, Result<(), String>> {
        // unguarded: a tokio task on the live runtime, not a GPUI handler; a panic in it ends that task alone (Err)
        let task = self.tokio.spawn(work(self.runtime.clone()));
        async move { task.await.unwrap_or_else(|_| Err(UNFINISHED.to_owned())) }.boxed()
    }
}

impl SpeechActions for Live {
    fn status(&self) -> BoxFuture<'static, Option<SpeechStatus>> {
        let runtime = self.runtime.clone();
        // unguarded: a tokio task on the live runtime, not a GPUI handler; a panic in it ends that task alone (None)
        let task = self.tokio.spawn(async move { runtime.status().await });
        async move { task.await.ok() }.boxed()
    }

    fn download_model(&self) -> BoxFuture<'static, Result<(), String>> {
        self.act(|runtime| async move { runtime.download_model().await }.boxed())
    }

    fn cancel_model_download(&self) -> BoxFuture<'static, Result<(), String>> {
        self.act(|runtime| async move { runtime.cancel_model_download().await }.boxed())
    }

    fn remove_model(&self) -> BoxFuture<'static, Result<(), String>> {
        self.act(|runtime| async move { runtime.remove_model().await }.boxed())
    }
}

/// What dictation runs on the live runtime: desktop-host's `Dictation` over this run's runtime, or a test's fake (a
/// real `Dictation` needs the verified whisper-cli and model on disk). `abort` is the runtime's own cancel: it ends a
/// session at once, a transcription in flight included.
trait Dictate: Send + Sync {
    fn begin(&self, session_id: String) -> BoxFuture<'_, Result<(), String>>;
    fn finish(&self) -> BoxFuture<'_, Result<(), String>>;
    fn cancel(&self) -> BoxFuture<'_, Result<(), String>>;
    fn abort(&self, session_id: String) -> BoxFuture<'_, Result<(), String>>;
    /// The window lost focus: a recording with audio in it is cancelled. True if it was.
    fn blur(&self) -> BoxFuture<'_, bool>;
}

/// The default microphone (WASAPI through `CpalSource`) into this run's runtime.
struct Microphone {
    dictation: Dictation,
    runtime: SpeechRuntime,
}

impl Dictate for Microphone {
    fn begin(&self, session_id: String) -> BoxFuture<'_, Result<(), String>> {
        async move { self.dictation.begin(&session_id).await }.boxed()
    }

    fn finish(&self) -> BoxFuture<'_, Result<(), String>> {
        self.dictation.finish().boxed()
    }

    fn cancel(&self) -> BoxFuture<'_, Result<(), String>> {
        self.dictation.cancel().boxed()
    }

    fn abort(&self, session_id: String) -> BoxFuture<'_, Result<(), String>> {
        async move { self.runtime.cancel(&session_id).await }.boxed()
    }

    fn blur(&self) -> BoxFuture<'_, bool> {
        self.dictation.on_window_blur().boxed()
    }
}

/// What a composer asks of dictation, in the order it asked; `Blur` is the window's, when it loses focus.
enum Ask {
    Begin(String),
    Finish,
    Cancel,
    Blur,
}

/// Runs the composers' asks on the live runtime, one at a time, so a finish never overtakes the begin it ends. A begin
/// that failed is its session's error: the runtime says nothing of a session that never started. A finish that failed
/// found its recording already ended by the runtime (its 60-second limit, a cancel), which reports that itself.
async fn serve(
    dictation: Arc<dyn Dictate>,
    mut asks: UnboundedReceiver<Ask>,
    report: UnboundedSender<SpeechEvent>,
) {
    while let Some(ask) = asks.next().await {
        match ask {
            Ask::Begin(session_id) => {
                if let Err(message) = dictation.begin(session_id.clone()).await {
                    let _ = report.unbounded_send(SpeechEvent::Session {
                        session_id,
                        phase: SpeechPhase::Error,
                        problem: Some(SpeechProblem {
                            code: "start_failed".into(),
                            message,
                        }),
                    });
                }
            }
            Ask::Finish => {
                if let Err(message) = dictation.finish().await {
                    log::info!("Dictation had no recording to finish: {message}");
                }
            }
            Ask::Cancel => {
                let _ = dictation.cancel().await;
            }
            Ask::Blur => {
                if dictation.blur().await {
                    log::info!("Dictation was cancelled: the window lost focus");
                }
            }
        }
    }
}

/// The composers' voice (`voice::VoiceBackend`) on the UI thread: each ask goes to the live runtime (`serve`). A cancel
/// also ends the session in the runtime at once, so a transcription in flight stops too; the queued cancel then closes
/// the microphone even if a begin was still opening it.
struct LiveVoice {
    asks: UnboundedSender<Ask>,
    dictation: Arc<dyn Dictate>,
    tokio: tokio::runtime::Handle,
}

impl VoiceBackend for LiveVoice {
    fn begin(&self, session_id: &str, _: &mut App) {
        let _ = self.asks.unbounded_send(Ask::Begin(session_id.to_owned()));
    }

    fn finish(&self, _: &str, _: &mut App) {
        let _ = self.asks.unbounded_send(Ask::Finish);
    }

    fn cancel(&self, session_id: &str, _: &mut App) {
        let (dictation, session_id) = (Arc::clone(&self.dictation), session_id.to_owned());
        // unguarded: a tokio task on the live runtime, not a GPUI handler; a panic in it ends that task alone
        self.tokio.spawn(async move {
            let _ = dictation.abort(session_id).await;
        });
        let _ = self.asks.unbounded_send(Ask::Cancel);
    }

    fn blur(&self, _: &mut App) {
        let _ = self.asks.unbounded_send(Ask::Blur);
    }
}

/// Dictation as the composers' voice (`voice::install`), its asks served on the live runtime. `events` carries what the
/// runtime says of its sessions, and `report` (its other end) a begin that failed; both reach the composers on the UI
/// thread (`voice::emit`).
fn install_voice(
    dictation: Arc<dyn Dictate>,
    (report, mut events): (UnboundedSender<SpeechEvent>, UnboundedReceiver<SpeechEvent>),
    tokio: &tokio::runtime::Handle,
    cx: &mut App,
) {
    let (asks, asked) = unbounded::<Ask>();
    let served = Arc::clone(&dictation);
    // unguarded: a tokio task on the live runtime, not a GPUI handler; its body is guarded, so a panic ends it alone
    tokio.spawn(async move {
        guarded_async("serve dictation", serve(served, asked, report)).await;
    });
    voice::install(
        Rc::new(LiveVoice {
            asks,
            dictation,
            tokio: tokio.clone(),
        }),
        cx,
    );
    cx.spawn(async move |cx| {
        guarded_async("follow dictation", async move {
            while let Some(event) = events.next().await {
                cx.update(|cx| voice::emit(event, cx));
            }
        })
        .await;
    })
    .detach();
}

/// Starts this run's speech runtime and dictation, or leaves Settings, Voice saying dictation is unavailable (and logs
/// why). ponytail: the runtime's digest is checked here on the UI thread at start (a few MiB hashed); move it to the
/// tokio runtime if the start ever shows it.
pub fn start(identity: &Identity, root: &Path, tokio: &tokio::runtime::Handle, cx: &mut App) {
    let user_data = std::env::var_os("TJOMMIE_USER_DATA").map(PathBuf::from);
    let roaming = std::env::var_os("APPDATA").map(PathBuf::from);
    let env = DirsEnv {
        user_data: user_data.as_deref(),
        roaming: roaming.as_deref(),
    };
    let dirs = match AppDirs::resolve(identity, root, &env) {
        Ok(dirs) => dirs,
        Err(refused) => {
            log::warn!("No local speech: the speech folder was refused ({refused:?})");
            return;
        }
    };
    let http = match ReqwestHttp::new(Policy::HuggingFace) {
        Ok(http) => http,
        Err(error) => {
            log::warn!("No local speech: {error}");
            return;
        }
    };
    let resources = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("resources").join("speech")))
        .unwrap_or_default();
    // A status from the runtime's events (`Some`), or a nudge to read it again (`None`: the model changed).
    let (sender, mut events) = futures::channel::mpsc::unbounded::<Option<SpeechStatus>>();
    let changed = sender.clone();
    // What the runtime says of a dictation session (its phases, its transcript), for the composers.
    let (said, heard) = unbounded::<SpeechEvent>();
    let report = said.clone();
    let model = SpeechModel::new(
        dirs.speech.join("models"),
        pinned(),
        Arc::new(http),
        Arc::new(move || {
            let _ = changed.unbounded_send(None);
        }),
    );
    let runtime = {
        let _context = tokio.enter();
        SpeechRuntime::new(RuntimeOptions {
            runtime_path: resources.join("whisper-cli.exe"),
            runtime_sha256: packaged_speech_digest(&resources),
            data_dir: dirs.speech,
            emit: Arc::new(move |event| match event {
                SpeechEvent::Status { status } => {
                    let _ = sender.unbounded_send(Some(status));
                }
                event => {
                    let _ = said.unbounded_send(event);
                }
            }),
            model,
            process: Arc::new(WhisperProcess),
            permission: Arc::new(|| microphone_permission(&registry::Windows)),
        })
    };
    let microphone = Arc::new(Microphone {
        dictation: Dictation::new(runtime.clone(), Arc::new(CpalSource)),
        runtime: runtime.clone(),
    });
    let actions: Arc<dyn SpeechActions> = Arc::new(Live {
        runtime,
        tokio: tokio.clone(),
    });
    cx.set_global(Speech {
        actions: Arc::clone(&actions),
        status: None,
    });
    install_voice(microphone, (report, heard), tokio, cx);
    cx.spawn(async move |cx| {
        guarded_async("follow the speech status", async move {
            while let Some(event) = events.next().await {
                let status = match event {
                    Some(status) => Some(status),
                    None => actions.status().await,
                };
                if let Some(status) = status {
                    cx.update(|cx| show(status, cx));
                }
            }
        })
        .await;
    })
    .detach();
}

/// Keeps `status` as the latest, and draws again.
pub fn show(status: SpeechStatus, cx: &mut App) {
    if cx.has_global::<Speech>() {
        cx.global_mut::<Speech>().status = Some(status);
        cx.refresh_windows();
    }
}

/// This device's dictation choices (Settings, Voice): the language and the input device, `""` for the default.
/// ponytail: kept for the run; the TypeScript saved them on the device (localStorage), and `DeviceSettings` has no voice
/// fields yet (desktop-host). With one English model the language rarely changes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VoiceChoice {
    pub language: String,
    pub input_device: String,
}

impl Global for VoiceChoice {}

/// The choices so far.
pub fn choice(cx: &App) -> VoiceChoice {
    cx.try_global::<VoiceChoice>().cloned().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::{Sender, channel};
    use std::time::Duration;

    use gpui::TestAppContext;
    use protocol::model::BotState;

    use super::*;
    use crate::model::dictation::TRANSCRIPT_ADDED;
    use crate::model::state::{Conversation, ConversationStatus};
    use crate::model::testing::FakeRpc;
    use crate::ui::shell::route::Route;
    use crate::ui::testing::{app, bot, click, draw, state, text};

    /// A microphone that says what it was asked, and won't begin when it has a reason not to.
    struct FakeMicrophone {
        asked: Sender<String>,
        refuse: Option<&'static str>,
    }

    /// An answer that's already there.
    fn done(outcome: Result<(), String>) -> BoxFuture<'static, Result<(), String>> {
        Box::pin(futures::future::ready(outcome))
    }

    impl Dictate for FakeMicrophone {
        fn begin(&self, session_id: String) -> BoxFuture<'_, Result<(), String>> {
            let _ = self.asked.send(format!("begin {session_id}"));
            done(self.refuse.map_or(Ok(()), |why| Err(why.to_owned())))
        }

        fn finish(&self) -> BoxFuture<'_, Result<(), String>> {
            let _ = self.asked.send("finish".to_owned());
            done(Err("This recording is no longer active.".to_owned()))
        }

        fn cancel(&self) -> BoxFuture<'_, Result<(), String>> {
            let _ = self.asked.send("cancel".to_owned());
            done(Ok(()))
        }

        fn abort(&self, session_id: String) -> BoxFuture<'_, Result<(), String>> {
            let _ = self.asked.send(format!("abort {session_id}"));
            done(Ok(()))
        }

        fn blur(&self) -> BoxFuture<'_, bool> {
            let _ = self.asked.send("blur".to_owned());
            Box::pin(futures::future::ready(true))
        }
    }

    /// The composers' asks run in order. A begin that failed is its session's error; a finish that found nothing to
    /// finish isn't (the runtime said how that recording ended).
    #[test]
    fn asks_run_in_order_and_a_begin_that_failed_is_its_sessions_error() {
        let (asked, told) = channel();
        let microphone = Arc::new(FakeMicrophone {
            asked,
            refuse: Some("No microphone could be opened."),
        });
        let (asks, queue) = unbounded();
        for ask in [
            Ask::Begin("dictation-1".into()),
            Ask::Finish,
            Ask::Cancel,
            Ask::Blur,
        ] {
            asks.unbounded_send(ask).unwrap();
        }
        drop(asks);
        let (report, mut reported) = unbounded();
        futures::executor::block_on(serve(microphone, queue, report));
        assert_eq!(
            told.try_iter().collect::<Vec<_>>(),
            ["begin dictation-1", "finish", "cancel", "blur"]
        );
        let events: Vec<SpeechEvent> = std::iter::from_fn(|| reported.try_recv().ok()).collect();
        assert_eq!(
            events,
            [SpeechEvent::Session {
                session_id: "dictation-1".into(),
                phase: SpeechPhase::Error,
                problem: Some(SpeechProblem {
                    code: "start_failed".into(),
                    message: "No microphone could be opened.".into(),
                }),
            }]
        );
    }

    /// M4 review M4: with the runtime wired the mic has its backend, and a transcript the runtime reports lands in the
    /// draft (and only Enter sends it).
    #[gpui::test]
    fn the_microphone_is_the_composers_voice_and_its_words_land_in_the_draft(
        cx: &mut TestAppContext,
    ) {
        // The asks run on a real Tokio thread, as on the live runtime.
        cx.executor().allow_parking();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let mut forge = state(vec![bot("b1", "Forge", BotState::Idle)], vec![], vec![]);
        forge.conversations.insert(
            "b1".into(),
            Conversation {
                status: ConversationStatus::Ready,
                messages: Vec::new(),
                has_more: false,
                pending: Vec::new(),
            },
        );
        let (cx, calls) = app(cx, Route::Chat("b1".into()), forge, FakeRpc::new());
        let (asked, told) = channel();
        let (said, heard) = unbounded();
        let runtime_says = said.clone();
        cx.update(|_, cx| {
            let microphone = Arc::new(FakeMicrophone {
                asked,
                refuse: None,
            });
            install_voice(microphone, (said, heard), runtime.handle(), cx);
            assert!(voice::backend(cx).is_some(), "the mic has its backend");
        });
        click(cx, "composer.b1.field");
        cx.simulate_input("Please ");
        click(cx, "composer.b1.dictate");
        let begun = told.recv_timeout(Duration::from_secs(10)).unwrap();
        let session = begun.strip_prefix("begin ").unwrap().to_owned();
        // The runtime's transcript, as its `emit` hands it over.
        runtime_says
            .unbounded_send(SpeechEvent::Final {
                session_id: session,
                text: "run the tests".into(),
            })
            .unwrap();
        draw(cx);
        assert_eq!(text("composer.b1.voice"), TRANSCRIPT_ADDED);
        click(cx, "composer.b1.field");
        cx.simulate_keystrokes("enter");
        draw(cx);
        let sent: Vec<_> = calls
            .calls()
            .into_iter()
            .filter(|(name, _)| *name == "chat.send")
            .collect();
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert_eq!(sent[0].1["text"], "Please run the tests");
    }
}
