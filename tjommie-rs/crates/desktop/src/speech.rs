//! This device's local dictation, wired into the app (T6.5's runtime; the Screens lane's live wiring): whisper-cli
//! shipped beside the app in `resources\speech` (dictation is available only if its manifest verifies), the English
//! model in the identity's speech folder (`AppDirs::speech`: production keeps 0.5.x's, so no second download), and the
//! status Settings, Voice and the diagnostic report read. The runtime is tokio's: its calls run on the live runtime,
//! and its status comes back to the UI thread through a channel.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use desktop_host::app_dirs::{AppDirs, DirsEnv};
use desktop_host::identity::Identity;
use desktop_host::shell::registry;
use desktop_host::speech::{
    RuntimeOptions, SpeechModel, SpeechRuntime, WhisperProcess, microphone_permission,
    packaged_speech_digest, pinned,
};
use desktop_host::updater::{Policy, ReqwestHttp};
use futures::future::BoxFuture;
use futures::{FutureExt as _, StreamExt as _};
use gpui::{App, Global};
use protocol::speech::{SpeechEvent, SpeechStatus};

use crate::guard::guarded_async;

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

/// Starts this run's speech runtime, or leaves Settings, Voice saying dictation is unavailable (and logs why).
/// ponytail: the runtime's digest is checked here on the UI thread at start (a few MiB hashed); move it to the tokio
/// runtime if the start ever shows it.
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
            emit: Arc::new(move |event| {
                if let SpeechEvent::Status { status } = event {
                    let _ = sender.unbounded_send(Some(status));
                }
            }),
            model,
            process: Arc::new(WhisperProcess),
            permission: Arc::new(|| microphone_permission(&registry::Windows)),
        })
    };
    let actions: Arc<dyn SpeechActions> = Arc::new(Live {
        runtime,
        tokio: tokio.clone(),
    });
    cx.set_global(Speech {
        actions: Arc::clone(&actions),
        status: None,
    });
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
