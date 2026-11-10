//! `vault-handlers.ts`: `vault.*` for an authenticated connection. The controller id comes from the caller's own
//! connection (`rpc:<connection id>`), never from params; when that connection ends, its controller is disconnected
//! so a private browser it was filling doesn't stay open. Every failure is one fixed message.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;
use protocol::ErrorCode;
use protocol::vault::{VaultHostAction, VaultHostCommand};
use protocol::vault_host::vault_host_result;
use protocol::wire::method;
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::server::{Handlers, RequestContext, RpcError};
use crate::vault_host_broker::{VaultRequest, VaultRequestOptions};

fn unsafe_to_finish() -> RpcError {
    RpcError::new(
        ErrorCode::BadState,
        "Logins could not complete the request safely. Check the host; any private browser remains private.",
    )
}

#[derive(Clone)]
struct Vault {
    request: VaultRequest,
    /// Connections whose end already disconnects their controller.
    watched: Arc<Mutex<HashSet<String>>>,
}

impl Vault {
    async fn send<T: DeserializeOwned>(
        self,
        action: VaultHostAction,
        context: Option<RequestContext>,
    ) -> Result<T, RpcError> {
        let context = context
            .filter(|context| !context.signal.is_cancelled() && !context.connection_id.is_empty())
            .ok_or_else(|| {
                RpcError::new(
                    ErrorCode::Forbidden,
                    "An active authenticated Logins connection is required.",
                )
            })?;
        let controller = format!("rpc:{}", context.connection_id);
        if self
            .watched
            .lock()
            .unwrap()
            .insert(context.connection_id.clone())
        {
            let (vault, signal, controller, connection) = (
                self.clone(),
                context.signal.clone(),
                controller.clone(),
                context.connection_id.clone(),
            );
            tokio::spawn(async move {
                signal.cancelled().await;
                vault.watched.lock().unwrap().remove(&connection);
                let disconnect =
                    json!({ "controllerId": controller, "action": { "kind": "disconnect" } });
                let _ = (vault.request)(disconnect, VaultRequestOptions::default()).await;
            });
        }
        let command = VaultHostCommand {
            controller_id: controller,
            action: action.clone(),
        };
        let live = context.signal.clone();
        let options = VaultRequestOptions {
            signal: Some(context.signal.clone()),
            authorized: Some(Arc::new(move || !live.is_cancelled())),
        };
        let value = (self.request)(
            serde_json::to_value(&command).expect("commands serialize"),
            options,
        )
        .await
        .map_err(|_| unsafe_to_finish())?;
        if context.signal.is_cancelled() {
            return Err(unsafe_to_finish());
        }
        let value = vault_host_result(&action, value).map_err(|_| unsafe_to_finish())?;
        serde_json::from_value(value).map_err(|_| unsafe_to_finish())
    }

    fn on<P, R>(
        &self,
        action: impl Fn(P) -> VaultHostAction + Send + Sync + 'static,
    ) -> impl Fn(P, Option<RequestContext>) -> BoxFuture<'static, Result<R, RpcError>>
    + Send
    + Sync
    + 'static
    where
        R: DeserializeOwned + Send + 'static,
    {
        let vault = self.clone();
        move |params, context| Box::pin(vault.clone().send(action(params), context))
    }
}

/// Adds `vault.*`, sending through `request` (`VaultHostBroker::requester` in the daemon).
pub fn register(handlers: Handlers, request: VaultRequest) -> Handlers {
    let vault = Vault {
        request,
        watched: Arc::default(),
    };
    handlers
        .on::<method::VaultStatus, _>(vault.on(|_| VaultHostAction::Status))
        .on::<method::VaultList, _>(vault.on(VaultHostAction::List))
        .on::<method::VaultUnlock, _>(vault.on(|_| VaultHostAction::Unlock))
        .on::<method::VaultLock, _>(vault.on(|_| VaultHostAction::Lock))
        .on::<method::VaultCancel, _>(vault.on(|_| VaultHostAction::Cancel))
        .on::<method::VaultEntry, _>(vault.on(VaultHostAction::Entry))
        .on::<method::VaultDelete, _>(vault.on(VaultHostAction::Delete))
        .on::<method::VaultFill, _>(vault.on(VaultHostAction::Fill))
        .on::<method::VaultFinish, _>(vault.on(VaultHostAction::Finish))
}
