//! The isolated-worker acceptance check: `scripts/vault/accept-worker.ts` of 0.4.2, ported. The owner runs it once,
//! unelevated, after `provision`, a restart and installing the runtime. It proves on this PC that each worker account
//! cannot reach what it must not, that it can write its own folder and that both provider CLIs start under it, and only
//! then writes the two receipts (`worker-acceptance.json`, `review-worker-acceptance.json`) that let the daemon enable
//! isolated mode (`worker::ready`).
//!
//! Two halves:
//! - The protocol (`run`, `receipts`, `Denials`, `Round`) is plain logic over a [`Steps`] seam, so it is unit-tested
//!   without Windows: every check must hold in both rounds before either receipt is built, a failed or errored check
//!   writes no receipt, and an earlier run's receipt is removed before the checks start and again if they fail, so it
//!   can never stand in for a check that did not pass.
//! - [`WindowsSteps`] is the real thing: it holds one identity live while the other tries to reach what it owns. Its
//!   probes are the fixed PowerShell programs of 0.4.2, run by the runtime's `node.exe` under the worker accounts
//!   (nothing else is runnable there: a worker may only start programs in the protected runtime folder).

use crate::cli::{
    BoxFuture, CliChild, CliExit, CliOutput, Env, InstalledWorkers, KillTree, Retirement, Runtime,
    SystemCli, WorkerInstall, is_worker_termination, process_env,
};
use crate::startup::{StartupError, install_workers};
use crate::win::boundary::{BoundaryError, WindowsNative, WorkerBoundary, WorkerNative};
use crate::win::worker::{Purpose, WorkerConfiguration};
use crate::win::{cred, logon};
use protocol::model::Provider;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};

const PROBE_FAILED: &str = "The synthetic worker denial probe failed.";
const NOT_ACCEPTED: &str = "The paired protected acceptance receipts were not accepted.";
const NO_CLI: &str = "Install both provider CLI distributions in the protected runtime folder.";
/// A worker tree has this long to be confirmed stopped (`retired`).
const RETIREMENT: Duration = Duration::from_secs(15);
/// The peer fixture has this long to create its marker and registry key.
const READY: Duration = Duration::from_secs(15);
/// The denial probe has this long to report.
const PROBE: Duration = Duration::from_secs(30);
const PROBE_OUTPUT_LIMIT: usize = 4096;

/// What went wrong. `Termination` is the one that matters most: a worker tree might still be running, so its scratch
/// folders are kept for the owner to reconcile (`WorkerLaunchTerminationError`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// A check did not hold, or could not be run.
    Check(String),
    /// A worker's process tree could not be confirmed stopped.
    Termination,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Check(message) => f.write_str(message),
            Self::Termination => f.write_str(crate::cli::WORKER_TERMINATION),
        }
    }
}

impl std::error::Error for Failure {}

impl From<BoundaryError> for Failure {
    fn from(error: BoundaryError) -> Self {
        match error {
            BoundaryError::Termination => Self::Termination,
            other => Self::Check(other.to_string()),
        }
    }
}

impl From<Failure> for BoundaryError {
    fn from(failure: Failure) -> Self {
        match failure {
            Failure::Termination => Self::Termination,
            Failure::Check(message) => Self::Other(message),
        }
    }
}

fn check(message: impl Into<String>) -> Failure {
    Failure::Check(message.into())
}

/// The nine facts one worker's probe reports (`Denials` in `accept-worker.ts`), in the receipt's field order. Each must
/// be `true` for acceptance: eight are things the worker must be refused, and `worker_write_allowed` is the one thing
/// it must be allowed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Denials {
    pub owner_file_denied: bool,
    pub owner_credential_denied: bool,
    pub owner_process_denied: bool,
    pub owner_desktop_denied: bool,
    pub worker_write_allowed: bool,
    pub peer_file_denied: bool,
    pub peer_write_denied: bool,
    pub peer_registry_denied: bool,
    pub peer_process_denied: bool,
}

impl Denials {
    /// The probe's report: exactly these nine fields, as booleans. A true or false is not judged here (`unproven`).
    pub fn parse(text: &str) -> Result<Self, Failure> {
        serde_json::from_str(text).map_err(|_| check(PROBE_FAILED))
    }

    /// The checks that did not hold, by their receipt field names.
    pub fn unproven(&self) -> Vec<&'static str> {
        [
            ("ownerFileDenied", self.owner_file_denied),
            ("ownerCredentialDenied", self.owner_credential_denied),
            ("ownerProcessDenied", self.owner_process_denied),
            ("ownerDesktopDenied", self.owner_desktop_denied),
            ("workerWriteAllowed", self.worker_write_allowed),
            ("peerFileDenied", self.peer_file_denied),
            ("peerWriteDenied", self.peer_write_denied),
            ("peerRegistryDenied", self.peer_registry_denied),
            ("peerProcessDenied", self.peer_process_denied),
        ]
        .into_iter()
        .filter_map(|(name, held)| (!held).then_some(name))
        .collect()
    }
}

/// What one round proved: one identity against a live peer. `review_policy_verified` is that the review identity's
/// policy was observed unchanged around its scoped work in this round (each round has exactly one review scope).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Round {
    pub denials: Denials,
    pub claude_started: bool,
    pub codex_started: bool,
    pub review_policy_verified: bool,
}

impl Round {
    /// Every check of this round that did not hold, by receipt field name.
    pub fn unproven(&self) -> Vec<&'static str> {
        let mut names = self.denials.unproven();
        for (name, held) in [
            ("codexStarted", self.codex_started),
            ("claudeStarted", self.claude_started),
            ("reviewPolicyVerified", self.review_policy_verified),
        ] {
            if !held {
                names.push(name);
            }
        }
        names
    }

    fn require(&self, who: Purpose) -> Result<(), Failure> {
        not_held(who, &self.unproven())
    }
}

/// Ok if nothing is left unproven, else the failure that names what did not hold for this identity.
fn not_held(who: Purpose, unproven: &[&'static str]) -> Result<(), Failure> {
    if unproven.is_empty() {
        Ok(())
    } else {
        Err(check(format!(
            "Acceptance failed for the {} identity: {} did not hold. No receipt was written.",
            who.as_str(),
            unproven.join(", ")
        )))
    }
}

/// One acceptance receipt as 0.4.2 wrote it: field order and compact JSON are the TypeScript's, so 0.4.2 and this
/// daemon's `worker::validate_acceptance` read the same bytes.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Receipt<'a> {
    version: u8,
    purpose: Purpose,
    worker_sid: &'a str,
    peer_sid: &'a str,
    configuration_hash: String,
    peer_configuration_hash: String,
    #[serde(flatten)]
    denials: Denials,
    codex_started: bool,
    claude_started: bool,
    review_policy_verified: bool,
}

/// Both receipts (provider, then review) as JSON, or the first failure: nothing is built unless every check held in
/// both rounds. Each receipt carries its own identity's denials and binds both SIDs and both configuration hashes.
pub fn receipts(
    provider: &WorkerConfiguration,
    review: &WorkerConfiguration,
    provider_round: &Round,
    review_round: &Round,
) -> Result<[String; 2], Failure> {
    provider_round.require(Purpose::Provider)?;
    review_round.require(Purpose::Review)?;
    let build =
        |config: &WorkerConfiguration, peer: &WorkerConfiguration, own: &Round, other: &Round| {
            serde_json::to_string(&Receipt {
                version: 2,
                purpose: config.purpose,
                worker_sid: &config.worker_sid,
                peer_sid: &config.peer_sid,
                configuration_hash: config.hash(),
                peer_configuration_hash: peer.hash(),
                denials: own.denials,
                codex_started: own.codex_started,
                claude_started: own.claude_started,
                review_policy_verified: own.review_policy_verified && other.review_policy_verified,
            })
            .map_err(|_| check(PROBE_FAILED))
        };
    Ok([
        build(provider, review, provider_round, review_round)?,
        build(review, provider, review_round, provider_round)?,
    ])
}

pub fn receipt_path(owner_data_root: &Path, purpose: Purpose) -> PathBuf {
    owner_data_root.join(purpose.acceptance_file())
}

fn pending(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".pending");
    PathBuf::from(name)
}

/// Removes both receipts and any half-written one. Tries all of them even after a failure and says which stayed.
fn clear(paths: &[PathBuf; 2]) -> Result<(), String> {
    let mut stayed = vec![];
    for path in paths {
        for file in [path.clone(), pending(path)] {
            match std::fs::remove_file(&file) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => stayed.push(format!("{}: {error}", file.display())),
            }
        }
    }
    if stayed.is_empty() {
        Ok(())
    } else {
        Err(stayed.join("; "))
    }
}

/// Where both receipts go (provider, then review).
fn receipt_paths(owner_data_root: &Path) -> [PathBuf; 2] {
    [
        receipt_path(owner_data_root, Purpose::Provider),
        receipt_path(owner_data_root, Purpose::Review),
    ]
}

/// Removes an earlier run's receipts before anything else, so no old receipt can stand beside a check that did not
/// pass. One that cannot be removed is an error: nothing is checked while it stands.
fn clear_earlier(paths: &[PathBuf; 2]) -> Result<(), Failure> {
    clear(paths).map_err(|stayed| {
        check(format!(
            "An earlier acceptance receipt could not be removed, so no check was run: {stayed}"
        ))
    })
}

/// `writeFileSync(path + ".pending")` then `renameSync`: a receipt appears whole or not at all.
fn write_receipt(path: &Path, json: &str) -> Result<(), Failure> {
    let (temporary, failed) = (pending(path), |error: std::io::Error| {
        check(format!("Could not write {}: {error}", path.display()))
    });
    let mut file = std::fs::File::create(&temporary).map_err(failed)?;
    file.write_all(json.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(failed)?;
    drop(file);
    std::fs::rename(&temporary, path).map_err(failed)
}

/// What the check does against the machine. [`WindowsSteps`] is the real one; the tests script it.
pub trait Steps: Send + Sync {
    /// Makes the synthetic owner marker and credential and each identity's scratch folder, after checking both
    /// identities are ready to be checked.
    fn prepare(&self) -> BoxFuture<'_, Result<(), Failure>>;
    /// One round (the loop body of `accept-worker.ts`): the peer of `active` is held live with its marker and registry
    /// key made, and `active` proves what it cannot reach and what it can write, and starts both CLIs.
    fn round(&self, active: Purpose) -> BoxFuture<'_, Result<Round, Failure>>;
    /// With both receipts written: the daemon's own readiness check must accept the pair.
    fn confirm(&self) -> BoxFuture<'_, Result<(), Failure>>;
    /// Always, last: removes the synthetic credential and marker and the scratch folders, unless a tree's stop was not
    /// confirmed (`keep_scratch`), and lets both identities go.
    fn cleanup(&self, keep_scratch: bool) -> BoxFuture<'_, ()>;
}

/// The whole check. Fails closed: the receipts are removed first, written only after every check of both rounds held,
/// taken back if the final pair verification refuses them, and removed again on any failure. An old receipt that
/// cannot be removed stops the run before any check, so one can never stand beside checks it did not pass.
pub async fn run(
    steps: &dyn Steps,
    provider: &WorkerConfiguration,
    review: &WorkerConfiguration,
) -> Result<(), Failure> {
    let paths = receipt_paths(&provider.owner_data_root);
    clear_earlier(&paths)?;
    let outcome = async {
        steps.prepare().await?;
        let first = steps.round(Purpose::Provider).await?;
        first.require(Purpose::Provider)?;
        let second = steps.round(Purpose::Review).await?;
        let [provider_receipt, review_receipt] = receipts(provider, review, &first, &second)?;
        write_receipt(&paths[0], &provider_receipt)?;
        write_receipt(&paths[1], &review_receipt)?;
        steps.confirm().await
    }
    .await;
    let cleared = if outcome.is_err() {
        clear(&paths)
    } else {
        Ok(())
    };
    steps.cleanup(outcome == Err(Failure::Termination)).await;
    match (outcome, cleared) {
        (Ok(()), _) => Ok(()),
        (Err(failure), Ok(())) => Err(failure),
        (Err(failure), Err(stayed)) => Err(check(format!(
            "{failure} In addition an acceptance receipt could not be removed ({stayed}); delete it by hand before relying on this machine."
        ))),
    }
}

/// `canonical.startsWith(base + "\\") && canonical.endsWith("acceptance-" + id)`, tightened to a direct child: only this
/// run's own scratch folder inside a worker root may be deleted, however the path is spelled.
pub fn removable_scratch(base: &Path, folder: &Path, id: &str) -> bool {
    let (Ok(folder), Ok(base)) = (std::fs::canonicalize(folder), std::fs::canonicalize(base))
    else {
        return false;
    };
    folder.parent() == Some(base.as_path())
        && folder
            .file_name()
            .is_some_and(|name| name == format!("acceptance-{id}").as_str())
}

static VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[0-9]+\.[0-9]+\.[0-9]+").expect("valid"));

/// A CLI started if `--version` exited 0 and printed a version (`checked.code === 0 && /\d+\.\d+\.\d+/`).
pub fn version_started(output: &CliOutput) -> bool {
    output.code == Some(0) && VERSION.is_match(&output.stdout)
}

// ---- the fixed programs a worker runs --------------------------------------------------------------------------

/// `powershellProbe`, byte for byte: reads the payload from stdin, tries everything the worker must be refused and the
/// one write it must be allowed, and prints the nine results as JSON. A missing target fails the script (an error is
/// not a denial), so a peer fixture that is not really there cannot be mistaken for a denied one.
const POWERSHELL_PROBE: &str = r#"
$ErrorActionPreference='Stop'
[Console]::InputEncoding=New-Object Text.UTF8Encoding($false)
[Console]::OutputEncoding=New-Object Text.UTF8Encoding($false)
$p=[Console]::In.ReadToEnd()|ConvertFrom-Json
Add-Type -TypeDefinition @'
using System;using System.IO;using System.Security;using System.Runtime.InteropServices;using Microsoft.Win32;
public static class TjWorkerProbe {
 [DllImport("advapi32.dll",EntryPoint="CredReadW",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool Read(string target,uint type,uint flags,out IntPtr value);
 [DllImport("advapi32.dll")] static extern void CredFree(IntPtr value);
 [DllImport("kernel32.dll",SetLastError=true)] static extern IntPtr OpenProcess(uint access,bool inherit,uint pid);
 [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr value);
 [DllImport("user32.dll",SetLastError=true)] static extern IntPtr OpenInputDesktop(uint flags,bool inherit,uint access);
 [DllImport("user32.dll")] static extern bool CloseDesktop(IntPtr value);
 public static bool CredentialDenied(string target) { IntPtr value; if(!Read(target,1,0,out value)) return Marshal.GetLastWin32Error()==1168; CredFree(value); return false; }
 public static bool ProcessDenied(uint pid) { var value=OpenProcess(0x0038,false,pid); if(value==IntPtr.Zero) return Marshal.GetLastWin32Error()==5; CloseHandle(value); return false; }
 public static bool DesktopDenied() { var value=OpenInputDesktop(0,false,0x0108); if(value==IntPtr.Zero) return Marshal.GetLastWin32Error()==5; CloseDesktop(value); return false; }
 public static bool FileDenied(string path,bool write) { try { using(var file=File.Open(path,FileMode.Open,write?FileAccess.ReadWrite:FileAccess.Read,FileShare.ReadWrite)){} return false; } catch(UnauthorizedAccessException){return true;} }
 public static bool RegistryDenied(string path) { try { using(var key=Registry.Users.OpenSubKey(path,true)){return false;} } catch(UnauthorizedAccessException){return true;} catch(SecurityException){return true;} }
}
'@
$writeAllowed=$false
try { [IO.File]::WriteAllText($p.write,'synthetic');$writeAllowed=$true } catch {}
@{ownerFileDenied=[TjWorkerProbe]::FileDenied($p.marker,$false);ownerCredentialDenied=[TjWorkerProbe]::CredentialDenied($p.target);ownerProcessDenied=[TjWorkerProbe]::ProcessDenied($p.ownerPid);ownerDesktopDenied=[TjWorkerProbe]::DesktopDenied();workerWriteAllowed=$writeAllowed;peerFileDenied=[TjWorkerProbe]::FileDenied($p.peerMarker,$false);peerWriteDenied=[TjWorkerProbe]::FileDenied($p.peerMarker,$true);peerRegistryDenied=[TjWorkerProbe]::RegistryDenied($p.peerRegistry);peerProcessDenied=[TjWorkerProbe]::ProcessDenied($p.peerPid)}|ConvertTo-Json -Compress
"#;

/// `powershellHolder`, byte for byte: makes the peer's marker file and registry key, says `ready`, then holds them until
/// its stdin closes and removes them.
const POWERSHELL_HOLDER: &str = r#"
$ErrorActionPreference='Stop'
[Console]::InputEncoding=New-Object Text.UTF8Encoding($false)
[Console]::OutputEncoding=New-Object Text.UTF8Encoding($false)
$p=[Console]::In.ReadLine()|ConvertFrom-Json
$key='HKCU:\'+$p.registry
try { [void](New-Item -Path $key -Force);[void](New-ItemProperty -LiteralPath $key -Name 'Synthetic' -Value 'present' -PropertyType String -Force);[IO.File]::WriteAllText($p.marker,'synthetic');[Console]::Out.WriteLine('ready');[void][Console]::In.ReadToEnd() }
finally { Remove-Item -LiteralPath $key -Recurse -Force -ErrorAction SilentlyContinue;Remove-Item -LiteralPath $p.marker -Force -ErrorAction SilentlyContinue }
"#;

/// `nativeNode`: the `node -e` program that runs one PowerShell script. `hold` keeps it running with its stdin piped
/// through (the holder); otherwise it runs once with the payload as input and prints the result (the probe).
fn native_node(script: &str, hold: bool) -> String {
    let head = format!(
        "const{{spawn,spawnSync}}=require('node:child_process');const{{join}}=require('node:path');const file=join(process.env.SystemRoot,'System32','WindowsPowerShell','v1.0','powershell.exe');const args=['-NoLogo','-NoProfile','-NonInteractive','-Command',{}];",
        serde_json::to_string(script).expect("a string")
    );
    let tail = if hold {
        "const p=spawn(file,args,{windowsHide:true,stdio:['pipe','pipe','pipe']});p.stdin.write(process.argv[1]+'\\n');process.stdin.pipe(p.stdin);p.stdout.pipe(process.stdout);p.stderr.pipe(process.stderr);p.once('error',()=>process.exit(1));p.once('close',code=>process.exit(code??1));"
    } else {
        "const p=spawnSync(file,args,{windowsHide:true,input:process.argv[1],encoding:'utf8',timeout:15000,maxBuffer:4096});if(p.status!==0)process.exit(1);process.stdout.write(p.stdout);"
    };
    head + tail
}

/// The arguments `runtime\node.exe` is started with for the denial probe: `-e <program> <payload JSON>`.
pub fn probe_args(payload: &str) -> Vec<String> {
    vec![
        "-e".into(),
        native_node(POWERSHELL_PROBE, false),
        payload.into(),
    ]
}

/// The arguments for the peer holder: `-e <program> <payload JSON>`.
pub fn holder_args(payload: &str) -> Vec<String> {
    vec![
        "-e".into(),
        native_node(POWERSHELL_HOLDER, true),
        payload.into(),
    ]
}

// ---- the real check --------------------------------------------------------------------------------------------

/// The owner's Credential Manager, as far as the check uses it: one synthetic entry the denial probe tries to read as the
/// worker. A seam so tests never touch the real vault (parallel writes and deletes there can resurrect a deleted entry).
pub trait Credentials: Send + Sync {
    fn write(&self, target: &str, value: &str) -> std::io::Result<()>;
    /// A missing target is not an error.
    fn delete(&self, target: &str) -> std::io::Result<()>;
}

struct CredentialManager;

impl Credentials for CredentialManager {
    fn write(&self, target: &str, value: &str) -> std::io::Result<()> {
        cred::write(target, value)
    }

    fn delete(&self, target: &str) -> std::io::Result<()> {
        cred::delete(target)
    }
}

/// What a test replaces; `Default` is the real thing.
#[derive(Default)]
pub struct Seams {
    /// "Both CLIs start": the real one needs both CLIs installed under the protected runtime.
    pub cli: Option<Arc<dyn CliProbe>>,
    pub credentials: Option<Arc<dyn Credentials>>,
}

/// "Both CLIs start" (`resolveCli` and `runCli(.., ["--version"])` under the identity). A seam so tests need no
/// installed CLI, no sign-in and no model turn; the real one runs `--version` only, which never makes a model call.
pub trait CliProbe: Send + Sync {
    /// Starts `cli --version` as `active`'s identity with `account` as its profile folder. Ok means it exited 0 and
    /// printed a version.
    fn started<'a>(
        &'a self,
        active: Purpose,
        cli: Provider,
        account: &'a Path,
    ) -> BoxFuture<'a, Result<(), Failure>>;
}

/// The real [`CliProbe`]: each identity's `SystemCli`, which launches only through that identity's worker boundary
/// and finds CLIs only in the protected runtime folder.
struct InstalledClis {
    provider: SystemCli,
    review: SystemCli,
}

impl CliProbe for InstalledClis {
    fn started<'a>(
        &'a self,
        active: Purpose,
        cli: Provider,
        account: &'a Path,
    ) -> BoxFuture<'a, Result<(), Failure>> {
        Box::pin(async move {
            let system = match active {
                Purpose::Provider => &self.provider,
                Purpose::Review => &self.review,
            };
            let (name, home) = match cli {
                Provider::Claude => ("claude", "CLAUDE_CONFIG_DIR"),
                Provider::Codex => ("codex", "CODEX_HOME"),
            };
            let command = system
                .resolve_cli(cli, &process_env(), Runtime::current())
                .ok_or_else(|| check(NO_CLI))?;
            let env: Env = [(home.to_string(), account.to_string_lossy().into_owned())].into();
            let output = system
                .run_cli(
                    &command,
                    &["--version".into()],
                    &env,
                    Duration::from_secs(30),
                )
                .await
                .map_err(|error| {
                    if is_worker_termination(&error) {
                        Failure::Termination
                    } else {
                        check(error.to_string())
                    }
                })?;
            if version_started(&output) {
                Ok(())
            } else {
                Err(check(format!(
                    "{name} could not start in the {} identity: its --version did not exit 0 with a version.",
                    active.as_str()
                )))
            }
        })
    }
}

/// A live peer fixture: its stdin (closing it ends the fixture) and what is needed to confirm it stopped.
struct Held {
    stdin: Option<Box<dyn AsyncWrite + Send + Unpin>>,
    retirement: Option<Retirement>,
    kill_tree: KillTree,
    pid: u32,
    marker: PathBuf,
    drain: tokio::task::JoinHandle<()>,
}

impl Held {
    /// `finally { child.stdin.end(); await retired(child); }`.
    async fn release(mut self) -> Result<(), Failure> {
        drop(self.stdin.take());
        let stopped = retired(self.retirement.take(), &self.kill_tree).await;
        self.drain.abort();
        stopped
    }
}

/// `retired`: the tree's stop must be confirmed, in time, or it is a `Termination`.
async fn retired(retirement: Option<Retirement>, kill_tree: &KillTree) -> Result<(), Failure> {
    let Some(proof) = retirement else {
        return Err(Failure::Termination);
    };
    match tokio::time::timeout(RETIREMENT, proof).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(_)) => Err(Failure::Termination),
        Err(_) => {
            let _ = kill_tree().await;
            Err(Failure::Termination)
        }
    }
}

/// Reads the fixture's `ready` line.
async fn read_ready(stdout: &mut (dyn AsyncRead + Send + Unpin)) -> Result<(), Failure> {
    let (mut seen, mut chunk) = (Vec::new(), [0u8; 64]);
    loop {
        let count = stdout.read(&mut chunk).await.unwrap_or(0);
        if count == 0 {
            return Err(check("The peer fixture closed before ready."));
        }
        seen.extend_from_slice(&chunk[..count]);
        if String::from_utf8_lossy(&seen).trim() == "ready" {
            return Ok(());
        }
        if seen.len() > 100 {
            return Err(check("The peer fixture did not initialize."));
        }
    }
}

/// `collect`: the probe's stdout (at most 4096 bytes, within 30 seconds), then proof that its tree stopped. It must
/// exit 0. Output that is too long, or an exit that fails, is a failed probe; an unconfirmed stop is a `Termination`.
async fn collect(child: CliChild) -> Result<String, Failure> {
    let CliChild {
        stdin,
        mut stdout,
        mut stderr,
        exit,
        kill_tree,
        retirement,
        ..
    } = child;
    drop(stdin);
    let drain = tokio::spawn(async move {
        let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
    });
    let reading = tokio::time::timeout(PROBE, async {
        let (mut bytes, mut chunk) = (Vec::new(), [0u8; 1024]);
        loop {
            let count = stdout.read(&mut chunk).await.unwrap_or(0);
            if count == 0 {
                return Some((bytes, exit.wait().await));
            }
            if bytes.len() + count > PROBE_OUTPUT_LIMIT {
                return None;
            }
            bytes.extend_from_slice(&chunk[..count]);
        }
    })
    .await;
    let result = match reading {
        Err(_) => {
            let _ = kill_tree().await;
            Err(Failure::Termination)
        }
        Ok(None) => match kill_tree().await {
            Ok(()) => Err(check(PROBE_FAILED)),
            Err(_) => Err(Failure::Termination),
        },
        Ok(Some((bytes, status))) => match retired(retirement, &kill_tree).await {
            Err(failure) => Err(failure),
            Ok(()) => match status {
                CliExit::Exited { code: Some(0), .. } => {
                    Ok(String::from_utf8_lossy(&bytes).into_owned())
                }
                _ => Err(check(PROBE_FAILED)),
            },
        },
    };
    drain.abort();
    result
}

/// The real [`Steps`]: the provider and review boundaries the daemon would install, a synthetic owner marker and
/// credential, and one scratch folder per identity.
pub struct WindowsSteps {
    id: String,
    provider: WorkerBoundary,
    review: WorkerBoundary,
    provider_config: WorkerConfiguration,
    review_config: WorkerConfiguration,
    native: Arc<dyn WorkerNative>,
    cli: Arc<dyn CliProbe>,
    credentials: Arc<dyn Credentials>,
    installed: Mutex<Option<InstalledWorkers>>,
    marker: PathBuf,
    credential: String,
    /// Provider's, then review's.
    scratch: [PathBuf; 2],
}

impl WindowsSteps {
    fn boundary(&self, purpose: Purpose) -> &WorkerBoundary {
        match purpose {
            Purpose::Provider => &self.provider,
            Purpose::Review => &self.review,
        }
    }

    fn scratch(&self, purpose: Purpose) -> &Path {
        match purpose {
            Purpose::Provider => &self.scratch[0],
            Purpose::Review => &self.scratch[1],
        }
    }

    fn registry_key(&self) -> String {
        format!(r"Software\Tjommie\Acceptance\{}", self.id)
    }

    /// `scope`: the review identity only ever runs inside its policy-observed lease (`withReviewPolicy`), so work as the
    /// review identity goes through it, and a completed one proves its policy was the same before and after.
    async fn scoped<T>(
        &self,
        purpose: Purpose,
        verified: &AtomicBool,
        action: impl Future<Output = Result<T, Failure>>,
    ) -> Result<T, Failure> {
        if purpose != Purpose::Review {
            return action.await;
        }
        let result = self
            .review
            .with_review_policy(|_before| async move { action.await.map_err(BoundaryError::from) })
            .await;
        if result.is_ok() {
            verified.store(true, Ordering::SeqCst);
        }
        result.map_err(Failure::from)
    }

    /// `holdPeer`'s first half: starts the peer fixture as `peer` and waits until it has made its marker and key.
    async fn hold(&self, peer: Purpose) -> Result<Held, Failure> {
        let boundary = self.boundary(peer);
        let config = boundary.configuration()?;
        let scratch = self.scratch(peer);
        let marker = scratch.join("peer-marker.txt");
        let payload = serde_json::json!({ "marker": marker.to_string_lossy(), "registry": self.registry_key() });
        let node = config.runtime_root.join("node.exe");
        let child = boundary
            .spawn(
                &node.to_string_lossy(),
                &holder_args(&payload.to_string()),
                scratch,
                &[],
            )
            .await?;
        let CliChild {
            pid,
            stdin,
            mut stdout,
            mut stderr,
            kill_tree,
            retirement,
            ..
        } = child;
        // Nothing the fixture prints after `ready` is wanted, but an unread pipe must not stall it.
        let (ready, rest) = tokio::sync::oneshot::channel();
        let drain = tokio::spawn(async move {
            let _ = ready.send(read_ready(&mut stdout).await);
            let (mut sink_out, mut sink_err) = (tokio::io::sink(), tokio::io::sink());
            let _ = tokio::join!(
                tokio::io::copy(&mut stdout, &mut sink_out),
                tokio::io::copy(&mut stderr, &mut sink_err)
            );
        });
        let waited = match tokio::time::timeout(READY, rest).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(check("The peer fixture closed before ready.")),
            Err(_) => {
                let _ = kill_tree().await;
                Err(Failure::Termination)
            }
        };
        // `peer.receipts().find(wrapperPid === child.pid)`: a process the boundary has no receipt for is one it cannot
        // account for.
        let accounted = pid.filter(|pid| {
            boundary
                .receipts()
                .iter()
                .any(|receipt| receipt.worker_pid == *pid)
        });
        let held = Held {
            stdin: Some(stdin),
            retirement,
            kill_tree,
            pid: accounted.unwrap_or(0),
            marker,
            drain,
        };
        let problem = match (waited, accounted) {
            (Ok(()), Some(_)) => return Ok(held),
            (Err(failure), _) => failure,
            (Ok(()), None) => Failure::Termination,
        };
        // Whatever went wrong, the fixture is stopped and confirmed gone before anything else happens.
        held.release().await?;
        Err(problem)
    }

    /// What `active` proves while the peer's fixture is live: the probe's nine results, then both CLIs.
    async fn prove(
        &self,
        active: Purpose,
        peer_pid: u32,
        peer_marker: &Path,
    ) -> Result<Denials, Failure> {
        let boundary = self.boundary(active);
        let config = boundary.configuration()?;
        let scratch = self.scratch(active);
        let payload = serde_json::json!({
            "marker": self.marker.to_string_lossy(),
            "target": self.credential,
            "ownerPid": std::process::id(),
            "peerPid": peer_pid,
            "peerMarker": peer_marker.to_string_lossy(),
            "peerRegistry": format!(r"{}\{}", config.peer_sid, self.registry_key()),
            "write": scratch.join("write.txt").to_string_lossy(),
        });
        let node = config.runtime_root.join("node.exe");
        let child = boundary
            .spawn(
                &node.to_string_lossy(),
                &probe_args(&payload.to_string()),
                scratch,
                &[],
            )
            .await?;
        let denials = Denials::parse(&collect(child).await?)?;
        // As in 0.4.2, where a false denial threw inside the round: an identity that has just shown it is not isolated
        // is not asked to start either CLI.
        not_held(active, &denials.unproven())?;
        let account = scratch.join("accounts");
        std::fs::create_dir(&account)
            .map_err(|error| check(format!("Could not make {}: {error}", account.display())))?;
        for cli in [Provider::Claude, Provider::Codex] {
            self.cli.started(active, cli, &account).await?;
        }
        Ok(denials)
    }
}

impl Steps for WindowsSteps {
    fn prepare(&self) -> BoxFuture<'_, Result<(), Failure>> {
        Box::pin(async move {
            // The daemon's own readiness check, run here so a refusal says why (a restart is due, a provider CLI is
            // running, a runtime file is writable) instead of the boundary's one fixed sentence.
            for config in [&self.provider_config, &self.review_config] {
                self.native.verify(config, &[]).await.map_err(|reason| {
                    check(format!(
                        "The {} worker is not ready to be checked: {reason}",
                        config.purpose.as_str()
                    ))
                })?;
            }
            for folder in &self.scratch {
                std::fs::create_dir(folder).map_err(|error| {
                    check(format!("Could not make {}: {error}", folder.display()))
                })?;
            }
            std::fs::write(&self.marker, "synthetic").map_err(|error| {
                check(format!("Could not make {}: {error}", self.marker.display()))
            })?;
            let key = logon::random_uuid().map_err(check)?;
            self.credentials
                .write(&self.credential, &format!("synthetic-{key}"))
                .map_err(|error| check(format!("Could not save the synthetic credential: {error}")))
        })
    }

    fn round(&self, active: Purpose) -> BoxFuture<'_, Result<Round, Failure>> {
        Box::pin(async move {
            let verified = AtomicBool::new(false);
            let peer = active.peer();
            let denials = self
                .scoped(peer, &verified, async {
                    let held = self.hold(peer).await?;
                    let proved = self
                        .scoped(
                            active,
                            &verified,
                            self.prove(active, held.pid, &held.marker),
                        )
                        .await;
                    // A tree that cannot be confirmed stopped outranks whatever the check found.
                    held.release().await.and(proved)
                })
                .await?;
            Ok(Round {
                denials,
                claude_started: true,
                codex_started: true,
                review_policy_verified: verified.load(Ordering::SeqCst),
            })
        })
    }

    fn confirm(&self) -> BoxFuture<'_, Result<(), Failure>> {
        Box::pin(async move {
            if self.provider.unavailable_reason().await.is_some()
                || self.review.unavailable_reason().await.is_some()
            {
                return Err(check(NOT_ACCEPTED));
            }
            Ok(())
        })
    }

    fn cleanup(&self, keep_scratch: bool) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            if let Err(error) = self.credentials.delete(&self.credential) {
                eprintln!(
                    "The synthetic credential {} could not be removed ({error}); delete it from Credential Manager.",
                    self.credential
                );
            }
            let _ = std::fs::remove_file(&self.marker);
            if !keep_scratch {
                for (config, folder) in [
                    (&self.provider_config, &self.scratch[0]),
                    (&self.review_config, &self.scratch[1]),
                ] {
                    if removable_scratch(&config.worker_root, folder, &self.id) {
                        let _ = std::fs::remove_dir_all(folder);
                    }
                }
            }
            self.provider.close();
            self.review.close();
            self.installed
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take();
        })
    }
}

/// Runs the check for the install at `root` with the real Windows layer and CLIs. Returns what to tell the owner.
pub async fn accept_worker(root: &Path) -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let daemon_dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    accept_with(root, &daemon_dir, Arc::new(WindowsNative), Seams::default()).await
}

/// [`accept_worker`] with the Windows layer and, optionally, "both CLIs start" and the credential vault supplied.
pub async fn accept_with(
    root: &Path,
    daemon_dir: &Path,
    native: Arc<dyn WorkerNative>,
    seams: Seams,
) -> Result<String, String> {
    // First of all, before the configuration is even read: every way out of this function, an early failure included,
    // starts from "no receipts" (`run` clears them again for the checks it makes).
    clear_earlier(&receipt_paths(root)).map_err(|failure| failure.to_string())?;
    for purpose in [Purpose::Provider, Purpose::Review] {
        if !root.join(purpose.configuration_file()).is_file() {
            return Err(format!(
                "There is no worker configuration in {}. Run `tjommie-daemon provision` first (from an elevated window), then restart Windows.",
                root.display()
            ));
        }
    }
    let load = |purpose: Purpose| {
        WorkerConfiguration::load(&root.join(purpose.configuration_file()), purpose).map_err(|_| {
            format!(
                "The worker configuration in {} is not a consistent provider and review pair. Nothing was checked.",
                root.display()
            )
        })
    };
    let (provider_config, review_config) = (load(Purpose::Provider)?, load(Purpose::Review)?);
    let install = Arc::new(WorkerInstall::default());
    let (provider, review, installed) = install_workers(root, daemon_dir, &install, &native)
        .map_err(|error| match error {
            StartupError::Plain(message) => message,
            StartupError::Unexpected(error) => error.to_string(),
        })?;
    let review = (*review).clone();
    let credentials = seams
        .credentials
        .unwrap_or_else(|| Arc::new(CredentialManager));
    let cli = seams.cli.unwrap_or_else(|| {
        Arc::new(InstalledClis {
            provider: SystemCli::with_worker(Arc::new(provider.clone()))
                .reporting_to(install.clone()),
            review: SystemCli::with_worker(Arc::new(review.clone())).reporting_to(install.clone()),
        })
    });
    let id = logon::random_uuid()?;
    let credential = provider_config
        .credential_target
        .strip_suffix("isolated-worker")
        .map(|prefix| format!("{prefix}acceptance/{id}"))
        .ok_or("The worker configuration has an unexpected credential target.")?;
    let scratch =
        |config: &WorkerConfiguration| config.worker_root.join(format!("acceptance-{id}"));
    let steps = WindowsSteps {
        marker: provider_config
            .owner_data_root
            .join(format!("acceptance-{id}.txt")),
        scratch: [scratch(&provider_config), scratch(&review_config)],
        credential,
        id,
        provider,
        review,
        provider_config: provider_config.clone(),
        review_config: review_config.clone(),
        native,
        cli,
        credentials,
        installed: Mutex::new(installed),
    };
    run(&steps, &provider_config, &review_config)
        .await
        .map_err(|failure| failure.to_string())?;
    Ok(format!(
        "Acceptance passed for the separate provider and review identities: denied owner and peer file, registry, process, credential and desktop access, permitted writes to their own folders, both CLI versions under both identities, and an unchanged review policy before and after. Wrote {} and {}. No model turn was run and no owner credential was read.",
        receipt_path(root, Purpose::Provider).display(),
        receipt_path(root, Purpose::Review).display()
    ))
}
