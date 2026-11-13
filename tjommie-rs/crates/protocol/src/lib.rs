//! Wire types for the Tjommie daemon protocol: one module per `packages/protocol/src/*.ts` file.
//!
//! `tjommie-rs/protocol/manifest.json`, generated from the zod schemas, is the contract. Values arriving from outside are
//! decoded through it: `Request::decode` for method params and `parse` for the standalone types. Both trim what zod trims,
//! check the JSON Schema, then deserialize; refinements the schema can't express (time zones, https-only URLs, canonical
//! keys, unique names) live in the newtypes and checks of each module. `assert_valid` checks what the daemon sends.
//! Deserializing these types with plain serde skips the schema, so never do that at a trust boundary.

#[macro_use]
mod refined;
mod contract;

pub mod accounts;
pub mod attachments;
pub mod browser;
pub mod diagnostics;
pub mod google;
pub mod mentions;
pub mod model;
pub mod native_browser;
pub mod projects;
pub mod relay;
pub mod remote;
pub mod speech;
pub mod vault;
pub mod vault_host;
pub mod wire;

pub use contract::{Invalid, Wire, assert_valid, parse, validate};
pub use wire::{
    ClientMessage, DecodeError, EVENTS, ErrorCode, Event, METHODS, PROTOCOL_VERSION, Request,
    Response, ServerMessage,
};

/// `nullable().optional()` fields: absent is `None`, `null` is `Some(None)`. Patches need the difference.
pub(crate) fn double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    serde::Deserialize::deserialize(deserializer).map(Some)
}
