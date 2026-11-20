//! The Tjommie daemon: a port of `packages/daemon`.

// The `provider-fixtures` feature swaps every provider CLI for recorded conversations (src/provider_fixtures.rs). It is
// for tests only, so a shipped daemon must never have it. Two locks, because either mistake alone is easy to make: the
// production identity (build.rs reads TJOMMIE_IDENTITY), and any build without debug assertions, which is what
// `--release` is, in case the identity was simply forgotten.
#[cfg(all(feature = "provider-fixtures", tjommie_identity = "production"))]
compile_error!(
    "The provider-fixtures feature replays recorded conversations instead of starting Claude or Codex, so it must never \
     be built into the production daemon (TJOMMIE_IDENTITY=production). Build the production daemon without \
     --features provider-fixtures."
);
#[cfg(all(feature = "provider-fixtures", not(debug_assertions)))]
compile_error!(
    "The provider-fixtures feature must not be built without debug assertions (a --release build): it replays recorded \
     conversations instead of starting Claude or Codex, and the production identity may simply have been forgotten. \
     Build the daemon without --features provider-fixtures, or in the dev profile."
);

pub mod account_handlers;
pub mod account_pricing;
pub mod account_quota;
pub mod account_runtime;
pub mod accounts_store;
pub mod approvals;
pub mod attachments;
pub mod bots;
pub mod browser_stream;
pub mod chat;
pub mod cli;
pub mod context;
pub mod db;
pub mod devices_store;
pub mod diagnostics;
pub mod fsid;
pub mod google;
pub mod host_poll;
pub mod ids;
pub mod import;
pub mod m4;
pub mod m5;
pub mod m6;
pub mod mcp;
pub mod models;
pub mod native_browser_broker;
pub mod native_browser_handlers;
pub mod native_browser_mcp;
pub mod paths;
pub mod peer_wire;
pub mod project_checks;
pub mod project_handlers;
pub mod project_review;
pub mod project_review_account;
pub mod project_review_cli;
pub mod project_review_policy;
pub mod project_review_runtime;
pub mod projects;
pub mod protect;
#[cfg(feature = "provider-fixtures")]
pub mod provider_fixtures;
pub mod providers;
pub mod push;
pub mod queue;
pub mod relay_client;
pub mod relay_identity;
pub mod remote;
pub mod remote_host;
pub mod remote_identity;
pub mod rooms;
pub mod routines;
pub mod runner;
pub mod scheduler;
pub mod server;
pub mod settings;
pub mod setup;
pub mod spawn;
pub mod startup;
pub mod task_execution;
pub mod task_integration;
pub mod tasks;
pub mod team;
pub mod turn_notices;
pub mod usage;
pub mod vault_handlers;
pub mod vault_host_broker;
pub mod win;
pub mod worker_boundary_host;
pub mod worktrees;
