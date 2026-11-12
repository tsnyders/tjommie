//! `native-browser-handlers.ts`: `browser.*` for an authenticated connection. Each call becomes a remote command under
//! the caller's own controller id (`rpc:<connection id>`), with the current control epoch where it acts on a page;
//! when the connection ends its controller is disconnected and a late result is discarded. Native state is checked
//! before it reaches a client, and a private page's frames are never passed on.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::FutureExt;
use futures_util::future::{BoxFuture, Shared};
use protocol::ErrorCode;
use protocol::browser::{BrowserFrame, BrowserFrameResult, BrowserState};
use protocol::native_browser::NativeBrowserRemoteAction as Action;
use protocol::wire::{Empty, method};
use serde_json::{Value, json};

use crate::native_browser_broker::Authorized;
use crate::server::{Handlers, RequestContext, RpcError};

/// `request(botId, command, authorized)`: the daemon's `nativeRequest`, which checks the bot and uses the broker.
pub type NativeRequest = Arc<
    dyn Fn(String, Value, Option<Authorized>) -> BoxFuture<'static, Result<Value, RpcError>>
        + Send
        + Sync,
>;

type Frame = Shared<BoxFuture<'static, Result<BrowserFrameResult, RpcError>>>;

fn bad_state(message: &str) -> RpcError {
    RpcError::new(ErrorCode::BadState, message)
}

fn epoch(value: Option<u64>) -> Result<u64, RpcError> {
    value.ok_or_else(|| bad_state("Use the current browser control epoch."))
}

fn browser_state(value: Value) -> Result<BrowserState, RpcError> {
    let invalid = || bad_state("The desktop returned an invalid browser state.");
    protocol::validate("browser.open", &value).map_err(|_| invalid())?;
    serde_json::from_value(value).map_err(|_| invalid())
}

struct Inner {
    request: NativeRequest,
    /// Connection and bot pairs whose end already disconnects the controller.
    watched: Mutex<HashSet<(String, String)>>,
    frames: Mutex<HashMap<String, (u64, Frame)>>,
    next_frame: AtomicU64,
}

#[derive(Clone)]
struct Browser(Arc<Inner>);

impl Browser {
    fn live(context: Option<RequestContext>) -> Result<RequestContext, RpcError> {
        context
            .filter(|context| !context.signal.is_cancelled())
            .ok_or_else(|| bad_state("An active authenticated browser connection is required."))
    }

    async fn send(
        self,
        bot: String,
        action: Action,
        context: Option<RequestContext>,
    ) -> Result<Value, RpcError> {
        let context = Self::live(context)?;
        let controller = format!("rpc:{}", context.connection_id);
        let pair = (context.connection_id.clone(), bot.clone());
        if self.0.watched.lock().unwrap().insert(pair.clone()) {
            let (browser, signal, controller, bot) = (
                self.clone(),
                context.signal.clone(),
                controller.clone(),
                bot.clone(),
            );
            tokio::spawn(async move {
                signal.cancelled().await;
                browser.0.watched.lock().unwrap().remove(&pair);
                let disconnect = json!({ "kind": "remote", "controllerId": controller, "action": { "kind": "disconnect" } });
                let _ = (browser.0.request)(bot, disconnect, None).await;
            });
        }
        let command = json!({ "kind": "remote", "controllerId": controller, "action": action });
        let live = context.signal.clone();
        let value =
            (self.0.request)(bot, command, Some(Arc::new(move || !live.is_cancelled()))).await?;
        if context.signal.is_cancelled() {
            return Err(bad_state(
                "This browser connection ended; the result was discarded.",
            ));
        }
        Ok(value)
    }

    async fn state(
        self,
        bot: String,
        action: Action,
        context: Option<RequestContext>,
    ) -> Result<BrowserState, RpcError> {
        browser_state(self.send(bot, action, context).await?)
    }

    async fn frame(
        self,
        bot: String,
        thumbnail: Option<bool>,
        context: Option<RequestContext>,
    ) -> Result<BrowserFrameResult, RpcError> {
        let context = Self::live(context)?;
        let size = if thumbnail == Some(true) {
            "thumbnail"
        } else {
            "frame"
        };
        let key = format!("{}:{bot}:{size}", context.connection_id);
        let frame = {
            let mut frames = self.0.frames.lock().unwrap();
            match frames.get(&key) {
                Some((_, frame)) => frame.clone(),
                None => {
                    let id = self.0.next_frame.fetch_add(1, Ordering::Relaxed);
                    let (browser, forget) = (self.clone(), key.clone());
                    let fetch: BoxFuture<'static, _> = Box::pin(async move {
                        let result = browser.clone().fetch_frame(bot, thumbnail, context).await;
                        let mut frames = browser.0.frames.lock().unwrap();
                        if frames
                            .get(&forget)
                            .is_some_and(|(current, _)| *current == id)
                        {
                            frames.remove(&forget);
                        }
                        result
                    });
                    let frame = fetch.shared();
                    frames.insert(key, (id, frame.clone()));
                    frame
                }
            }
        };
        frame.await
    }

    async fn fetch_frame(
        self,
        bot: String,
        thumbnail: Option<bool>,
        context: RequestContext,
    ) -> Result<BrowserFrameResult, RpcError> {
        let value = self
            .send(bot, Action::Frame { thumbnail }, Some(context))
            .await?;
        let (Some(state), Some(frame)) = (value.get("state"), value.get("frame")) else {
            return Err(bad_state("The desktop returned an invalid browser frame."));
        };
        let current = browser_state(state.clone())?;
        let frame: Option<BrowserFrame> = match frame {
            Value::Null => None,
            frame => {
                let shown = json!({ "state": state, "frame": frame });
                protocol::validate("browser.frame", &shown)
                    .map_err(|_| bad_state("The desktop returned an invalid browser frame."))?;
                Some(
                    serde_json::from_value(frame.clone())
                        .map_err(|_| bad_state("The desktop returned an invalid browser frame."))?,
                )
            }
        };
        if current.private_active == Some(true) && frame.is_some() {
            return Err(bad_state("Private browser frames are suppressed."));
        }
        if let Some(frame) = &frame {
            let stale = frame.page_id != current.active_page_id
                || frame.viewport_revision != current.viewport_revision
                || frame.control_epoch != current.control_epoch
                || frame.private_epoch != current.private_epoch;
            if stale {
                return Err(bad_state("The desktop returned a stale browser frame."));
            }
        }
        Ok(BrowserFrameResult {
            state: current,
            frame,
            unchanged: None,
        })
    }
}

/// Adds `browser.*`, sending through `request`.
pub fn register(handlers: Handlers, request: NativeRequest) -> Handlers {
    let browser = Browser(Arc::new(Inner {
        request,
        watched: Mutex::default(),
        frames: Mutex::default(),
        next_frame: AtomicU64::new(0),
    }));
    let b = || browser.clone();
    handlers
        .on::<method::BrowserOpen, _>({
            let browser = b();
            move |target, context| browser.clone().state(target.bot_id, Action::State, context)
        })
        .on::<method::BrowserFrame, _>({
            let browser = b();
            move |params, context| {
                browser
                    .clone()
                    .frame(params.bot_id, params.thumbnail, context)
            }
        })
        .on::<method::BrowserControl, _>({
            let browser = b();
            move |params, context| {
                let action = Action::Control {
                    control: params.control,
                    generation: params.generation,
                    control_epoch: params.control_epoch,
                    release_only: params.release_only,
                };
                browser.clone().state(params.bot_id, action, context)
            }
        })
        .on::<method::BrowserNavigate, _>({
            let browser = b();
            move |params, context| {
                let browser = browser.clone();
                async move {
                    let page = params.page;
                    let action = Action::Navigate {
                        control_epoch: epoch(page.control_epoch)?,
                        generation: page.generation,
                        page_id: page.page_id,
                        url: params.url,
                    };
                    browser.state(page.bot_id, action, context).await
                }
            }
        })
        .on::<method::BrowserHistory, _>({
            let browser = b();
            move |params, context| {
                let browser = browser.clone();
                async move {
                    let page = params.page;
                    let action = Action::History {
                        control_epoch: epoch(page.control_epoch)?,
                        generation: page.generation,
                        page_id: page.page_id,
                        action: params.action,
                    };
                    browser.state(page.bot_id, action, context).await
                }
            }
        })
        .on::<method::BrowserTab, _>({
            let browser = b();
            move |params, context| {
                let browser = browser.clone();
                async move {
                    let action = Action::Tab {
                        generation: params.generation,
                        control_epoch: epoch(params.control_epoch)?,
                        action: params.action,
                        page_id: params.page_id,
                    };
                    browser.state(params.bot_id, action, context).await
                }
            }
        })
        .on::<method::BrowserDialog, _>({
            let browser = b();
            move |params, context| {
                let browser = browser.clone();
                async move {
                    let page = params.page;
                    let action = Action::Dialog {
                        control_epoch: epoch(page.control_epoch)?,
                        generation: page.generation,
                        page_id: page.page_id,
                        accept: params.accept,
                        prompt_text: params.prompt_text,
                    };
                    browser.state(page.bot_id, action, context).await
                }
            }
        })
        .on::<method::BrowserResize, _>({
            let browser = b();
            move |params, context| {
                let browser = browser.clone();
                async move {
                    let page = params.page;
                    let action = Action::Resize {
                        control_epoch: epoch(page.control_epoch)?,
                        generation: page.generation,
                        page_id: page.page_id,
                        width: params.width,
                        height: params.height,
                    };
                    browser.state(page.bot_id, action, context).await
                }
            }
        })
        .on::<method::BrowserInput, _>({
            let browser = b();
            move |params, context| {
                let browser = browser.clone();
                async move {
                    let viewport_revision = params
                        .viewport_revision
                        .ok_or_else(|| bad_state("Use the current browser viewport revision."))?;
                    let page = params.page;
                    let action = Action::Input {
                        control_epoch: epoch(page.control_epoch)?,
                        generation: page.generation,
                        page_id: page.page_id,
                        viewport_revision,
                        input: params.input,
                    };
                    browser
                        .send(page.bot_id, action, context)
                        .await
                        .map(|_| Empty {})
                }
            }
        })
}
