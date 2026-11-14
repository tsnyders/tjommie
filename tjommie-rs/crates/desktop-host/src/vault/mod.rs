//! The login vault (Phase 3 plan T3.6): saved logins whose records live in `data/logins.sqlite` and whose secrets live in
//! Credential Manager, the trusted request gate, the client of the daemon's `/vault-host`, and the remote worker isolation.
//! Ported from `login-vault.ts`, `vault-storage.ts`, `vault-command-gate.ts`, `vault-host.ts` and `worker-isolation.ts`.

pub mod credential;
pub mod entry;
pub mod fill;
pub mod gate;
pub mod host;
pub mod login;
pub mod storage;
pub mod worker;
