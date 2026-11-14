//! The typed WebSocket client to a daemon, plus Noise connections to other PCs (Phase 3 plan, M1). Phase 4 reuses it.
//!
//! A port of `packages/client/src/client.ts`. [`Client::call`] sends one request and waits for its answer; replies are
//! matched by id, so calls can overlap. [`Client::events`] hands out receivers of everything the daemon pushes. Every
//! result and event is checked against the protocol manifest before it is used, like the TypeScript client's zod parse:
//! a frame that breaks the contract closes the connection with 4400.

mod connection;
mod error;

pub use connection::{Client, ConnectOptions, Endpoint};
pub use error::{CloseReason, ConnectError, RpcError};
pub use protocol::ClientMethod;

use std::future::Future;

/// What code that only makes calls needs from a daemon connection. [`Client`] is one; a test gives the view models a fake
/// that answers from a table, with no daemon and no socket.
pub trait Rpc: Send + Sync {
    fn call<M: ClientMethod>(
        &self,
        params: M::Params,
    ) -> impl Future<Output = Result<M::Result, RpcError>> + Send;
}

impl Rpc for Client {
    fn call<M: ClientMethod>(
        &self,
        params: M::Params,
    ) -> impl Future<Output = Result<M::Result, RpcError>> + Send {
        Client::call::<M>(self, params)
    }
}
