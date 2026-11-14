//! `native-browser-host.ts`: the desktop's client of the daemon's private `/browser-host` route.
//!
//! The daemon queues a bot's browser jobs; this client long-polls for them with the private desktop host token on two
//! lanes (a normal one, at most four jobs at once, and an urgent one for `cancel_tools` and remote control changes),
//! runs each job through an [`Executor`] and posts the answer back. It also posts the live stream's frames.
//!
//! The bridge lives on a thread and tokio runtime of its own. It never touches WebView2 or the UI: a job crosses to the
//! UI thread through the [`Executor`] (see `host`), and what comes back is plain typed data.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures::future::BoxFuture;
use protocol::native_browser::{NativeBrowserCommand, NativeBrowserStreamMessage};
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, watch};

use super::BotIdentity;
use super::engine::Reply;

/// How many normal-lane jobs run at once; the urgent lane is separate so cancellation never waits behind them.
const NORMAL_JOBS: usize = 4;
/// How long a posted frame, or the end of a stream, may wait for the daemon's answer.
const STREAM_POST_TIMEOUT: Duration = Duration::from_secs(2);
const RETRY_AFTER_ERROR: Duration = Duration::from_secs(1);
const ERROR_CHARS: usize = 2000;
const TOO_LARGE: &str = "The browser's answer was too large to send.";
const MISMATCH: &str = "The desktop browser command did not match the installed host protocol.";

/// A job the daemon queued for this desktop.
#[derive(Debug, Deserialize)]
pub struct Job {
    pub id: String,
    pub bot: BotIdentity,
    pub command: NativeBrowserCommand,
}

impl Job {
    /// The limits the daemon's job schema promises (`native-browser-host.ts:4`).
    fn is_well_formed(&self) -> bool {
        let chars = |text: &str| text.chars().count();
        uuid::Uuid::parse_str(&self.id).is_ok()
            && (1..=256).contains(&chars(&self.bot.id))
            && chars(&self.bot.name) <= 1000
            && (1..=32_768).contains(&chars(&self.bot.home_folder))
            && self
                .bot
                .project_folder
                .as_deref()
                .is_none_or(|folder| chars(folder) <= 32_768)
    }
}

/// Runs a job and answers with a reply, or with the text of why it failed. Called from the bridge's thread, so it
/// must hand the work to whoever owns the browser.
pub trait Executor: Send + Sync + 'static {
    fn execute(&self, job: Job) -> BoxFuture<'static, Result<Reply, String>>;
}

pub struct BridgeConfig {
    pub port: u16,
    /// The private desktop host token, never the renderer's daemon token.
    pub token: String,
}

#[derive(Serialize)]
struct Answer<'a> {
    id: &'a str,
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<&'a Reply>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// What the lanes share.
struct Shared {
    client: reqwest::Client,
    url: String,
    token: String,
    executor: Arc<dyn Executor>,
    on_error: Arc<dyn Fn() + Send + Sync>,
    /// Normal-lane jobs running now.
    active: AtomicUsize,
    /// Answers the daemon refused (a status other than 204): they were not delivered.
    undelivered: AtomicUsize,
    finished: Notify,
}

enum Polled {
    Nothing,
    Job(Vec<u8>),
}

impl Shared {
    async fn poll(&self, control: bool) -> Result<Polled, reqwest::Error> {
        let url = if control {
            format!("{}?control=1", self.url)
        } else {
            self.url.clone()
        };
        let response = self.client.get(url).bearer_auth(&self.token).send().await?;
        if response.status() == reqwest::StatusCode::NO_CONTENT {
            return Ok(Polled::Nothing);
        }
        let response = response.error_for_status()?;
        Ok(Polled::Job(response.bytes().await?.to_vec()))
    }

    async fn post(
        &self,
        body: Vec<u8>,
        timeout: Option<Duration>,
    ) -> Result<reqwest::StatusCode, reqwest::Error> {
        let mut request = self
            .client
            .post(&self.url)
            .bearer_auth(&self.token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body);
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        Ok(request.send().await?.status())
    }

    async fn answer(&self, id: &str, result: Result<Reply, String>) {
        let answer = match &result {
            Ok(reply) => Answer {
                id,
                ok: true,
                result: Some(reply),
                error: None,
            },
            Err(error) => Answer {
                id,
                ok: false,
                result: None,
                error: Some(error.chars().take(ERROR_CHARS).collect()),
            },
        };
        let Ok(body) = serde_json::to_vec(&answer) else {
            return;
        };
        match self.post(body, None).await {
            Ok(status) if status.is_success() => {}
            Ok(reqwest::StatusCode::PAYLOAD_TOO_LARGE) if result.is_ok() => {
                // Too big for the daemon: it would wait out its timeout on a job nobody answers. Say so briefly.
                log::warn!("the daemon refused a browser answer as too large");
                self.undelivered.fetch_add(1, Ordering::SeqCst);
                let short = Answer {
                    id,
                    ok: false,
                    result: None,
                    error: Some(TOO_LARGE.to_string()),
                };
                if let Ok(body) = serde_json::to_vec(&short) {
                    let _ = self.post(body, None).await;
                }
            }
            Ok(status) => {
                // 404: the job was gone (it timed out at the daemon). Anything else is as unexpected. Not delivered.
                log::warn!("the daemon refused a browser answer: {status}");
                self.undelivered.fetch_add(1, Ordering::SeqCst);
            }
            Err(_) => (self.on_error)(),
        }
    }

    async fn run_job(self: Arc<Self>, job: Job) {
        let id = job.id.clone();
        let result = self.executor.execute(job).await;
        self.answer(&id, result).await;
    }
}

/// One lane's loop: take a job, run it, never wait for it to finish before asking for the next (the normal lane up to
/// four at a time).
async fn lane(shared: Arc<Shared>, control: bool, mut stop: watch::Receiver<bool>) {
    while !*stop.borrow() {
        if !control && shared.active.load(Ordering::SeqCst) >= NORMAL_JOBS {
            tokio::select! {
                () = shared.finished.notified() => {}
                _ = stop.changed() => break,
            }
            continue;
        }
        let polled = tokio::select! {
            polled = shared.poll(control) => polled,
            _ = stop.changed() => break,
        };
        match polled {
            Ok(Polled::Nothing) => {}
            Ok(Polled::Job(bytes)) => match serde_json::from_slice::<Job>(&bytes) {
                Ok(job) if job.is_well_formed() => {
                    let running = shared.clone();
                    let counted = !control;
                    if counted {
                        shared.active.fetch_add(1, Ordering::SeqCst);
                    }
                    tokio::spawn(async move {
                        running.clone().run_job(job).await;
                        if counted {
                            running.active.fetch_sub(1, Ordering::SeqCst);
                            running.finished.notify_waiters();
                        }
                    });
                }
                _ => {
                    // A job this host cannot read is answered, so the daemon does not wait on it.
                    #[derive(Deserialize)]
                    struct JustId {
                        id: String,
                    }
                    if let Ok(JustId { id }) = serde_json::from_slice(&bytes) {
                        shared.answer(&id, Err(MISMATCH.to_string())).await;
                    }
                }
            },
            Err(_) => {
                (shared.on_error)();
                tokio::select! {
                    () = tokio::time::sleep(RETRY_AFTER_ERROR) => {}
                    _ = stop.changed() => break,
                }
            }
        }
    }
}

/// A running bridge. Close it before a new one starts for a new daemon: after a restart the port and token differ.
pub struct HostBridge {
    stop: watch::Sender<bool>,
    runtime: tokio::runtime::Handle,
    shared: Arc<Shared>,
    thread: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl HostBridge {
    /// Starts the two lanes. `on_error` runs when the daemon cannot be reached or refuses (the app locks the vault).
    pub fn start(
        config: BridgeConfig,
        executor: Arc<dyn Executor>,
        on_error: Arc<dyn Fn() + Send + Sync>,
    ) -> std::io::Result<HostBridge> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(5))
            .build()
            .map_err(std::io::Error::other)?;
        let shared = Arc::new(Shared {
            client,
            url: format!("http://127.0.0.1:{}/browser-host", config.port),
            token: config.token,
            executor,
            on_error,
            active: AtomicUsize::new(0),
            undelivered: AtomicUsize::new(0),
            finished: Notify::new(),
        });
        let (stop, stopped) = watch::channel(false);
        let (ready, handle) = std::sync::mpsc::channel();
        let lanes = shared.clone();
        let thread = std::thread::Builder::new()
            .name("browser-host-bridge".into())
            .spawn(move || {
                let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                let _ = ready.send(runtime.handle().clone());
                runtime.block_on(async {
                    tokio::join!(
                        lane(lanes.clone(), false, stopped.clone()),
                        lane(lanes.clone(), true, stopped.clone())
                    );
                });
                // Jobs still running are dropped: they wait on the UI thread, which may be the one closing the bridge.
                runtime.shutdown_background();
            })?;
        let runtime = handle.recv().map_err(|_| {
            std::io::Error::other("The browser host bridge could not start its runtime.")
        })?;
        Ok(HostBridge {
            stop,
            runtime,
            shared,
            thread: Mutex::new(Some(thread)),
        })
    }

    /// Posts a live frame, or the end of a stream, outside any job. `false` means the daemon did not take it: nobody is
    /// watching, or it is unreachable, and the caller stops sending.
    pub async fn send(&self, message: &NativeBrowserStreamMessage) -> bool {
        let Ok(body) = serde_json::to_vec(message) else {
            return false;
        };
        let shared = self.shared.clone();
        let posted = self
            .runtime
            .spawn(async move { shared.post(body, Some(STREAM_POST_TIMEOUT)).await })
            .await;
        // 204 is the only acceptance; anything else (404: nobody is subscribed) has nothing to read.
        matches!(posted, Ok(Ok(status)) if status == reqwest::StatusCode::NO_CONTENT)
    }

    /// How many answers the daemon refused so far: they were not delivered.
    pub fn undelivered(&self) -> usize {
        self.shared.undelivered.load(Ordering::SeqCst)
    }

    /// Stops both lanes. Quick: it does not wait for jobs still running.
    pub fn close(&self) {
        let _ = self.stop.send(true);
        let thread = self
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }
}

impl Drop for HostBridge {
    fn drop(&mut self) {
        self.close();
    }
}
