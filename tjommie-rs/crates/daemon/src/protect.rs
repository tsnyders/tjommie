//! The folder-protection service from `daemon.ts:128-145`: the Codex data ACL pass (`win::acl::protect_codex_data`), the
//! status it last returned, a serialising queue so passes never overlap, and the 60 s throttle for passes nobody forced.
//! `refresh` is `refreshProtection`; callers force a pass after accounts or bot homes change.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use protocol::ErrorCode;
use rusqlite::Connection;
use tokio::time::Instant;

use crate::bots::list_stored_bots;
use crate::context::Db;
use crate::server::RpcError;
use crate::win::acl::{BotHome, ProtectedAccount, ProtectedPaths, Protection, protect_codex_data};

/// How long an unforced `refresh` waits before running another pass (`Date.now() - lastProtectionAt < 60_000`).
const THROTTLE: Duration = Duration::from_secs(60);

/// One pass over the protected folders. [`FolderProtection::new`] uses the real, native one; tests substitute their own.
pub type Pass =
    Arc<dyn Fn(&ProtectedPaths, &[BotHome], Option<&[ProtectedAccount]>) -> Protection + Send + Sync>;

/// `listAccounts(db)` as the pass wants it: every account folder and its provider. The accounts store supplies it.
pub type ListAccounts =
    Arc<dyn Fn(&Connection) -> Result<Vec<ProtectedAccount>, RpcError> + Send + Sync>;

struct Inner {
    protection: Protection,
    last: Option<Instant>,
}

pub struct FolderProtection {
    paths: ProtectedPaths,
    db: Db,
    list_accounts: ListAccounts,
    pass: Pass,
    inner: Mutex<Inner>,
    /// `aclQueue`: one pass at a time, in the order they were asked for.
    queue: tokio::sync::Mutex<()>,
}

impl FolderProtection {
    /// `initial` is what the synchronous pass before SQLite opens returned (`protectCodexData(paths)`, see
    /// [`FolderProtection::first_pass`]); the status stays that until the first [`FolderProtection::refresh`].
    pub fn new(
        paths: ProtectedPaths,
        db: Db,
        list_accounts: ListAccounts,
        initial: Protection,
    ) -> Arc<Self> {
        Self::with_pass(
            paths,
            db,
            list_accounts,
            initial,
            Arc::new(protect_codex_data),
        )
    }

    pub fn with_pass(
        paths: ProtectedPaths,
        db: Db,
        list_accounts: ListAccounts,
        initial: Protection,
        pass: Pass,
    ) -> Arc<Self> {
        Arc::new(Self {
            paths,
            db,
            list_accounts,
            pass,
            inner: Mutex::new(Inner {
                protection: initial,
                last: None,
            }),
            queue: tokio::sync::Mutex::new(()),
        })
    }

    /// `protectCodexData(paths)`: the only synchronous pass. It finishes before SQLite opens. Blocking.
    pub fn first_pass(paths: &ProtectedPaths) -> Protection {
        protect_codex_data(paths, &[], None)
    }

    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `protection`: what the last pass found.
    pub fn status(&self) -> Protection {
        self.inner().protection
    }

    /// `protection.reason`: why Codex isn't ready, if it isn't.
    pub fn reason(&self) -> Option<&'static str> {
        self.status().reason()
    }

    /// `refreshProtection(force, override)`. Unforced calls within 60 s of the last pass do nothing. `override_home`
    /// replaces that bot's home folder and provider in this pass, for a provider change not yet saved.
    pub async fn refresh(
        &self,
        force: bool,
        override_home: Option<BotHome>,
    ) -> Result<(), RpcError> {
        let _turn = self.queue.lock().await;
        if !force && self.inner().last.is_some_and(|at| at.elapsed() < THROTTLE) {
            return Ok(());
        }
        let (db, list_accounts) = (self.db.clone(), self.list_accounts.clone());
        let (mut homes, accounts) = blocking(move || -> Result<_, RpcError> {
            let db = db.lock().unwrap_or_else(PoisonError::into_inner);
            let homes: Vec<BotHome> = list_stored_bots(&db)?
                .into_iter()
                .map(|bot| BotHome {
                    id: bot.id,
                    home_folder: PathBuf::from(bot.home_folder),
                    provider: bot.provider,
                })
                .collect();
            Ok((homes, list_accounts(&db)?))
        })
        .await??;
        if let Some(over) = &override_home {
            for home in homes.iter_mut().filter(|home| home.id == over.id) {
                *home = over.clone();
            }
        }
        let (paths, pass) = (self.paths.clone(), self.pass.clone());
        let status = blocking(move || pass(&paths, &homes, Some(&accounts))).await?;
        let mut inner = self.inner();
        inner.protection = status;
        inner.last = Some(Instant::now());
        Ok(())
    }
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Result<T, RpcError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| RpcError::new(ErrorCode::Internal, "Folder protection stopped unexpectedly."))
}
