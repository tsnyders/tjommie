//! Isolated Windows worker identities: the checks from `windows-worker.ts` and its helper `windows-worker.ps1` that
//! need no Windows object. The native calls (accounts, jobs, logon launches, grants, verification) are in `logon.rs`.
//! The TypeScript daemon ran the helper as a wrapper process and exchanged receipt files with it; the Rust daemon
//! makes the calls itself and holds the job handles, so that file protocol is gone and its checks stay.

use crate::win::acl::{below, same_path};
use crate::win::cred::Namespace;
use crate::win::logon::{Ace, Descriptor};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::{fs, io::Write};

pub(crate) const FAILURE: &str =
    "The isolated Windows provider could not start. No owner credential was sent to it.";
const INVALID_INPUT: &str = "Invalid Windows worker launch input.";
const LAUNCH_LIMIT: &str = "The Windows worker command exceeds its supported launch limit.";
pub(crate) const SYSTEM: &str = "S-1-5-18";
pub(crate) const ADMINISTRATORS: &str = "S-1-5-32-544";
/// `FileSystemRights.ReadAndExecute | Synchronize`: all a runtime may grant anyone else.
const READ_ONLY: u32 = 0x0012_00a9;

/// `WINDOWS_WORKER_BOOTSTRAP` (`windows-worker.ts:25`), byte for byte: the fixed logon command reads one private
/// frame from stdin and runs it, so no target argument or credential enters the logon command line.
pub const BOOTSTRAP: &str = "try{const f=require('node:fs'),r=n=>{if(n<1||n>262144)throw 0;const b=Buffer.alloc(n);for(let i=0;i<n;){const k=f.readSync(0,b,i,n-i,null);if(!k)throw 0;i+=k}return b},p=JSON.parse(r(r(4).readUInt32LE())),v=require('node:child_process').spawnSync(p.file,p.args,{stdio:'inherit',windowsHide:true});process.exitCode=v.status??71}catch{process.exitCode=71}";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Purpose {
    Provider,
    Review,
}

impl Purpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Review => "review",
        }
    }
    pub fn peer(self) -> Self {
        match self {
            Self::Provider => Self::Review,
            Self::Review => Self::Provider,
        }
    }
    pub fn configuration_file(self) -> &'static str {
        match self {
            Self::Provider => "worker.json",
            Self::Review => "review-worker.json",
        }
    }
    fn acceptance_file(self) -> &'static str {
        match self {
            Self::Provider => "worker-acceptance.json",
            Self::Review => "review-worker-acceptance.json",
        }
    }
    fn account_prefix(self) -> &'static str {
        match self {
            Self::Provider => "TjWorker-",
            Self::Review => "TjReview-",
        }
    }
    fn target_suffix(self) -> &'static str {
        match self {
            Self::Provider => "/isolated-worker",
            Self::Review => "/isolated-review",
        }
    }
}

/// `TjWorker-<ns8>` or `TjReview-<ns8>` (`provision-worker.ps1:14-15`).
pub fn worker_name(namespace: &Namespace, purpose: Purpose) -> String {
    format!("{}{}", purpose.account_prefix(), &namespace.short()[..8])
}

/// `WindowsWorkerConfiguration` (`windows-worker.ts:12-17`). Field order is the TypeScript schema's, so the hash
/// over its JSON matches `workerConfigurationHash`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerConfiguration {
    pub version: u8,
    pub purpose: Purpose,
    pub owner_sid: String,
    pub worker_sid: String,
    pub peer_sid: String,
    pub worker_name: String,
    pub worker_root: PathBuf,
    pub peer_root: PathBuf,
    pub runtime_root: PathBuf,
    pub owner_data_root: PathBuf,
    pub provisioned_at: String,
    pub credential_target: String,
}

impl WorkerConfiguration {
    pub fn parse(json: &str) -> Result<Self, String> {
        let config: Self = serde_json::from_str(json).map_err(|error| error.to_string())?;
        let valid = config.version == 2
            && [&config.owner_sid, &config.worker_sid, &config.peer_sid]
                .iter()
                .all(|sid| user_sid(sid))
            && worker_name_shape(&config.worker_name)
            && [
                &config.worker_root,
                &config.peer_root,
                &config.runtime_root,
                &config.owner_data_root,
            ]
            .iter()
            .all(|p| !p.as_os_str().is_empty())
            && config.provisioned_at.ends_with('Z')
            && config.provisioned_at.parse::<jiff::Timestamp>().is_ok()
            && credential_target_shape(&config.credential_target);
        if valid {
            Ok(config)
        } else {
            Err("invalid worker configuration".into())
        }
    }

    /// `workerConfigurationHash`: sha256 of the configuration's JSON.
    pub fn hash(&self) -> String {
        format!(
            "{:x}",
            Sha256::digest(serde_json::to_string(self).expect("plain data"))
        )
    }

    /// `WindowsWorkerBoundary.configuration` (`windows-worker.ts:202-211`): this purpose's file, paired with its
    /// peer, as written and with every root resolved.
    pub fn load(path: &Path, purpose: Purpose) -> Result<Self, String> {
        let read = |path: &Path| {
            fs::read_to_string(path)
                .map_err(|_| FAILURE.to_string())
                .and_then(|text| Self::parse(&text))
        };
        let config = read(path)?;
        if config.purpose != purpose
            || !same_path(
                path,
                &config.owner_data_root.join(purpose.configuration_file()),
            )
        {
            return Err(FAILURE.into());
        }
        let peer = read(
            &config
                .owner_data_root
                .join(purpose.peer().configuration_file()),
        )?;
        validate_pair(&config, &peer)?;
        validate_pair(&peer, &config)?;
        let canonical = |value: &Self| -> Result<Self, String> {
            let real = |path: &Path| fs::canonicalize(path).map_err(|_| FAILURE.to_string());
            Ok(Self {
                worker_root: real(&value.worker_root)?,
                peer_root: real(&value.peer_root)?,
                runtime_root: real(&value.runtime_root)?,
                owner_data_root: real(&value.owner_data_root)?,
                ..value.clone()
            })
        };
        validate_pair(&canonical(&config)?, &canonical(&peer)?)?;
        Ok(config)
    }
}

/// `^S-1-5-21-\d+-\d+-\d+-\d+$`
fn user_sid(sid: &str) -> bool {
    sid.strip_prefix("S-1-5-21-").is_some_and(|rest| {
        let parts: Vec<&str> = rest.split('-').collect();
        parts.len() == 4
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
    })
}

fn lower_hex(text: &str, length: usize) -> bool {
    text.len() == length && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `^Tj(?:Worker|Review)-[a-f0-9]{8}$`
fn worker_name_shape(name: &str) -> bool {
    ["TjWorker-", "TjReview-"].iter().any(|prefix| {
        name.strip_prefix(prefix)
            .is_some_and(|rest| lower_hex(rest, 8))
    })
}

/// `^Tjommie\/[a-f0-9]{24}\/isolated-(?:worker|review)$`
fn credential_target_shape(target: &str) -> bool {
    target.strip_prefix("Tjommie/").is_some_and(|rest| {
        rest.len() > 24
            && lower_hex(&rest[..24], 24)
            && matches!(&rest[24..], "/isolated-worker" | "/isolated-review")
    })
}

/// `a` contains `b`, or they are the same path. Paths are compared without resolving links, like Node's `relative`.
fn overlap(a: &Path, b: &Path) -> bool {
    same_path(a, b) || below(a, b).is_some()
}

/// `validateWorkerPair` (`windows-worker.ts:162-171`).
pub fn validate_pair(
    config: &WorkerConfiguration,
    peer: &WorkerConfiguration,
) -> Result<(), String> {
    let sids = [&config.owner_sid, &config.worker_sid, &peer.worker_sid];
    let distinct = sids[0] != sids[1] && sids[0] != sids[2] && sids[1] != sids[2];
    let roots = [
        &config.worker_root,
        &peer.worker_root,
        &config.runtime_root,
        &config.owner_data_root,
    ];
    let disjoint = roots.iter().enumerate().all(|(i, a)| {
        roots
            .iter()
            .enumerate()
            .all(|(j, b)| i == j || (!overlap(a, b) && !overlap(b, a)))
    });
    let valid = config.purpose != peer.purpose
        && distinct
        && config.owner_sid == peer.owner_sid
        && config.peer_sid == peer.worker_sid
        && peer.peer_sid == config.worker_sid
        && config.provisioned_at == peer.provisioned_at
        && [
            &config.worker_root,
            &config.peer_root,
            &config.runtime_root,
            &config.owner_data_root,
            &peer.worker_root,
        ]
        .iter()
        .all(|p| p.is_absolute())
        && same_path(&config.peer_root, &peer.worker_root)
        && same_path(&peer.peer_root, &config.worker_root)
        && same_path(&config.runtime_root, &peer.runtime_root)
        && same_path(&config.owner_data_root, &peer.owner_data_root)
        && config
            .worker_name
            .starts_with(config.purpose.account_prefix())
        && config
            .credential_target
            .ends_with(config.purpose.target_suffix())
        && disjoint;
    if valid { Ok(()) } else { Err(FAILURE.into()) }
}

/// The acceptance receipt (`windows-worker.ts:20`): every denial was observed on this machine.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Acceptance {
    version: u8,
    purpose: Purpose,
    worker_sid: String,
    peer_sid: String,
    configuration_hash: String,
    peer_configuration_hash: String,
    owner_file_denied: bool,
    owner_credential_denied: bool,
    owner_process_denied: bool,
    owner_desktop_denied: bool,
    worker_write_allowed: bool,
    peer_file_denied: bool,
    peer_write_denied: bool,
    peer_registry_denied: bool,
    peer_process_denied: bool,
    codex_started: bool,
    claude_started: bool,
    review_policy_verified: bool,
}

/// `validateWorkerAcceptance` (`windows-worker.ts:96-99`).
pub fn validate_acceptance(
    config: &WorkerConfiguration,
    peer: &WorkerConfiguration,
    raw: &str,
) -> Result<(), String> {
    let a: Acceptance = serde_json::from_str(raw).map_err(|_| FAILURE.to_string())?;
    let observed = [
        a.owner_file_denied,
        a.owner_credential_denied,
        a.owner_process_denied,
        a.owner_desktop_denied,
        a.worker_write_allowed,
        a.peer_file_denied,
        a.peer_write_denied,
        a.peer_registry_denied,
        a.peer_process_denied,
        a.codex_started,
        a.claude_started,
        a.review_policy_verified,
    ];
    let valid = a.version == 2
        && observed.iter().all(|value| *value)
        && user_sid(&a.worker_sid)
        && user_sid(&a.peer_sid)
        && lower_hex(&a.configuration_hash, 64)
        && lower_hex(&a.peer_configuration_hash, 64)
        && a.purpose == config.purpose
        && a.worker_sid == config.worker_sid
        && a.peer_sid == config.peer_sid
        && a.configuration_hash == config.hash()
        && a.peer_configuration_hash == peer.hash();
    if valid { Ok(()) } else { Err(FAILURE.into()) }
}

/// `required` (`windows-worker.ts:201`): configured, or still marked as needing protection.
pub fn required(configuration_path: &Path) -> bool {
    configuration_path.exists()
        || configuration_path
            .parent()
            .is_some_and(|dir| dir.join("vault-protection-required").exists())
}

/// The file checks of `unavailableReason` (`windows-worker.ts:212-226`): a paired configuration with an acceptance
/// receipt for both identities. The caller adds its exclusivity check and `logon::verify`.
pub fn ready(configuration_path: &Path, purpose: Purpose) -> Result<WorkerConfiguration, String> {
    let config = WorkerConfiguration::load(configuration_path, purpose)?;
    let peer_path = config
        .owner_data_root
        .join(purpose.peer().configuration_file());
    let peer = fs::read_to_string(peer_path)
        .map_err(|_| FAILURE.to_string())
        .and_then(|text| WorkerConfiguration::parse(&text))?;
    for (current, other) in [(&config, &peer), (&peer, &config)] {
        let raw = fs::read_to_string(
            config
                .owner_data_root
                .join(current.purpose.acceptance_file()),
        )
        .map_err(|_| FAILURE.to_string())?;
        validate_acceptance(current, other, &raw)?;
    }
    Ok(config)
}

/// libuv's Windows argv quoting (`windows-worker.ts:27-39`).
fn quote(value: &str) -> Result<String, String> {
    if value.contains('\0') {
        return Err(INVALID_INPUT.into());
    }
    if !value.is_empty() && !value.contains([' ', '\t', '"']) {
        return Ok(value.into());
    }
    let (mut result, mut slashes) = (String::from('"'), 0);
    for character in value.chars() {
        if character == '\\' {
            slashes += 1;
            continue;
        }
        let escapes = if character == '"' {
            slashes * 2 + 1
        } else {
            slashes
        };
        result.extend(std::iter::repeat_n('\\', escapes));
        result.push(character);
        slashes = 0;
    }
    result.extend(std::iter::repeat_n('\\', slashes * 2));
    result.push('"');
    Ok(result)
}

/// `windowsWorkerCommandLine`: at most 32767 UTF-16 units with the terminator.
pub fn command_line(file: &str, args: &[String]) -> Result<String, String> {
    if file.is_empty() || args.len() > 32766 {
        return Err(INVALID_INPUT.into());
    }
    let mut parts = vec![quote(file)?];
    let mut length = parts[0].encode_utf16().count();
    for arg in args {
        let value = quote(arg)?;
        length += value.encode_utf16().count() + 1;
        if length + 1 > 32767 {
            return Err(LAUNCH_LIMIT.into());
        }
        parts.push(value);
    }
    Ok(parts.join(" "))
}

/// `workerLaunchFrame`: a little-endian length, then `{"file","args"}` as JSON, for the bootstrap to read.
pub fn launch_frame(file: &str, args: &[String]) -> Result<Vec<u8>, String> {
    #[derive(Serialize)]
    struct Frame<'a> {
        file: &'a str,
        args: &'a [String],
    }
    command_line(file, args)?;
    let body = serde_json::to_vec(&Frame { file, args }).map_err(|_| INVALID_INPUT.to_string())?;
    if body.is_empty() || body.len() > 262_144 {
        return Err(INVALID_INPUT.into());
    }
    let mut frame = (body.len() as u32).to_le_bytes().to_vec();
    frame.extend(body);
    Ok(frame)
}

/// `workerLaunchEnvironment`: refuses malformed entries and drops what Node reads before the bootstrap runs.
pub fn launch_environment(input: &[(String, String)]) -> Result<Vec<(String, String)>, String> {
    let mut kept = vec![];
    for (key, value) in input {
        if key.is_empty() || key.contains(['=', '\0']) || value.contains('\0') {
            return Err(INVALID_INPUT.into());
        }
        let upper = key.to_ascii_uppercase();
        if !upper.starts_with("NODE_") && upper != "ELECTRON_RUN_AS_NODE" {
            kept.push((key.clone(), value.clone()));
        }
    }
    Ok(kept)
}

/// The worker's environment (`windows-worker.ps1:315-331`): the filtered launch values, then the fixed ones, which
/// win regardless of case. Sorted without case, as Windows expects.
pub fn worker_environment(
    config: &WorkerConfiguration,
    input: &[(String, String)],
) -> Result<Vec<(String, String)>, String> {
    let system = std::env::var("SystemRoot").map_err(|_| FAILURE.to_string())?;
    let root = config.worker_root.to_string_lossy().into_owned();
    let under = |parts: &[&str]| {
        parts
            .iter()
            .fold(config.worker_root.clone(), |path, part| path.join(part))
            .to_string_lossy()
            .into_owned()
    };
    let mut values = launch_environment(input)?;
    let fixed = [
        ("SystemRoot", system.clone()),
        ("WINDIR", system),
        (
            "ProgramFiles",
            std::env::var("ProgramFiles").unwrap_or_default(),
        ),
        (
            "ProgramData",
            std::env::var("ProgramData").unwrap_or_default(),
        ),
        ("USERPROFILE", root.clone()),
        ("HOME", root),
        ("APPDATA", under(&["AppData", "Roaming"])),
        ("LOCALAPPDATA", under(&["AppData", "Local"])),
        ("TEMP", under(&["tmp"])),
        ("TMP", under(&["tmp"])),
    ];
    for (key, value) in fixed {
        values.retain(|(existing, _)| !existing.eq_ignore_ascii_case(key));
        values.push((key.into(), value));
    }
    values.sort_by_key(|(key, _)| key.to_uppercase());
    Ok(values)
}

fn trusted(sid: &str, owner_sid: &str) -> bool {
    sid == owner_sid || sid == SYSTEM || sid == ADMINISTRATORS
}

/// `AssertJobDescriptor` (`windows-worker.ps1:152-160`): owned by the owner, and only plain allows for the owner,
/// SYSTEM and Administrators.
pub fn assert_job_descriptor(descriptor: &Descriptor, owner_sid: &str) -> Result<(), String> {
    let untrusted = || "Untrusted job descriptor.".to_string();
    if descriptor.owner()?.as_deref() != Some(owner_sid) {
        return Err(untrusted());
    }
    let aces = descriptor.aces()?.ok_or_else(untrusted)?;
    if aces
        .iter()
        .all(|ace| ace.kind == Ace::ALLOWED && ace.flags == 0 && trusted(&ace.sid, owner_sid))
    {
        Ok(())
    } else {
        Err(untrusted())
    }
}

/// `Assert-TrustedRuntimeDescriptor` (`windows-worker.ps1:14-24`): a trusted owner, a real DACL, and nobody else
/// may do more than read and execute.
pub fn assert_runtime_descriptor(descriptor: &Descriptor, owner_sid: &str) -> Result<(), String> {
    if !descriptor
        .owner()?
        .is_some_and(|owner| trusted(&owner, owner_sid))
    {
        return Err("untrusted runtime owner".into());
    }
    let aces = descriptor.aces()?.ok_or("null runtime DACL")?;
    for ace in aces
        .iter()
        .filter(|ace| ace.kind == Ace::ALLOWED && !trusted(&ace.sid, owner_sid))
    {
        if ace.mask & !READ_ONLY != 0 {
            return Err("writable runtime descendant".into());
        }
    }
    Ok(())
}

/// `Assert-TrustedRuntimeTree` (`windows-worker.ps1:25-35`): every file and folder below the runtime, with no links.
pub fn assert_runtime_tree(root: &Path, owner_sid: &str) -> Result<(), String> {
    use std::os::windows::fs::MetadataExt;
    const REPARSE_POINT: u32 = 0x400;
    let started = std::time::Instant::now();
    let (mut pending, mut count) = (vec![root.to_path_buf()], 0);
    while let Some(path) = pending.pop() {
        count += 1;
        if count > 100_000 || started.elapsed().as_secs() > 20 {
            return Err("runtime verification limit".into());
        }
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.file_attributes() & REPARSE_POINT != 0 {
            return Err("runtime reparse target".into());
        }
        assert_runtime_descriptor(&Descriptor::of_path(&path)?, owner_sid)?;
        if metadata.is_dir() {
            for entry in fs::read_dir(&path).map_err(|error| error.to_string())? {
                pending.push(entry.map_err(|error| error.to_string())?.path());
            }
        }
    }
    Ok(())
}

/// `Assert-TrustedReviewMutexDescriptor` (`windows-worker.ps1:36-42`).
pub fn assert_mutex_descriptor(descriptor: &Descriptor, owner_sid: &str) -> Result<(), String> {
    if !descriptor
        .owner()?
        .is_some_and(|owner| trusted(&owner, owner_sid))
    {
        return Err("untrusted review mutex owner".into());
    }
    let aces = descriptor.aces()?.ok_or("null review mutex DACL")?;
    if aces.iter().all(|ace| trusted(&ace.sid, owner_sid)) {
        Ok(())
    } else {
        Err("untrusted review mutex access".into())
    }
}

/// `Test-UnreceiptedProviderCandidate` (`windows-worker.ps1:75-80`). Job membership is checked first, because
/// Windows hides another account's command line.
pub fn unreceipted_provider(
    name: &str,
    command_line: Option<&str>,
    in_owned_job: bool,
) -> Result<bool, String> {
    if in_owned_job {
        return Ok(false);
    }
    let name = name.to_ascii_lowercase();
    let node = name == "node" || name == "node.exe";
    let line = command_line.map(|line| line.to_lowercase().replace('/', "\\"));
    if node && line.as_deref().is_none_or(|line| line.trim().is_empty()) {
        return Err("An existing Node process could not be identified safely.".into());
    }
    let provider_script = line.is_some_and(|line| {
        line.contains(r"\@anthropic-ai\claude-code\") || line.contains(r"\@openai\codex\")
    });
    Ok(name == "claude.exe" || name == "codex.exe" || (node && provider_script))
}

/// What waiting on the review mutex returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MutexWait {
    Acquired,
    Busy,
    Abandoned,
}

/// `New-ReviewMarker`: created once, never overwritten.
pub(crate) fn write_review_marker(marker: &Path, worker_sid: &str) -> Result<(), String> {
    let body = serde_json::json!({ "version": 2, "purpose": "review", "workerSid": worker_sid, "wrapperPid": std::process::id() });
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(marker)
        .map_err(|error| error.to_string())?;
    file.write_all(body.to_string().as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| error.to_string())
}

/// `Enter-TrustedReviewMutex` (`windows-worker.ps1:48-60`): one host at a time, and never past a review whose
/// process tree wasn't confirmed stopped. An abandoned mutex leaves a durable marker.
pub fn enter_review_mutex(
    wait: MutexWait,
    marker: &Path,
    worker_sid: &str,
    release: impl FnOnce(),
) -> Result<(), String> {
    match wait {
        MutexWait::Busy => Err("Another host owns the review identity.".into()),
        MutexWait::Abandoned => {
            let written = if marker.exists() {
                Ok(())
            } else {
                write_review_marker(marker, worker_sid)
            };
            release();
            written?;
            Err("Abandoned review ownership requires native job reconciliation.".into())
        }
        MutexWait::Acquired if marker.exists() => {
            release();
            Err("Review process-tree retirement was not confirmed.".into())
        }
        MutexWait::Acquired => Ok(()),
    }
}

/// `inside` (`windows-worker.ts:100-102`): both resolved through links; a path that can't be resolved fails closed.
/// Compared by file identity (`fsid`), so no share, junction or drive alias gets past it.
fn inside(root: &Path, path: &Path) -> Result<bool, String> {
    if !root.exists() || !path.exists() {
        return Err(FAILURE.into());
    }
    crate::fsid::is_within(path, root).map_err(|_| FAILURE.to_string())
}

/// `grantTarget` (`windows-worker.ts:142-157`): the only folders a worker may be granted. Never the runtime or the
/// daemon's own folder, the peer's root, or the owner's data, except assigned `bots/<id>` and `workspaces/<id>`.
pub fn grant_target(
    config: &WorkerConfiguration,
    daemon_dir: &Path,
    path: &Path,
) -> Result<PathBuf, String> {
    let canonical = fs::canonicalize(path).map_err(|_| FAILURE.to_string())?;
    let fail = || Err(FAILURE.to_string());
    for trusted in [config.runtime_root.as_path(), daemon_dir] {
        if inside(trusted, &canonical)? || inside(&canonical, trusted)? {
            return fail();
        }
    }
    if inside(&config.peer_root, &canonical)? || inside(&canonical, &config.peer_root)? {
        return fail();
    }
    if config.purpose == Purpose::Review && !inside(&config.worker_root, &canonical)? {
        return fail();
    }
    if inside(&canonical, &config.owner_data_root)? {
        return fail();
    }
    if inside(&config.owner_data_root, &canonical)? {
        let owner = fs::canonicalize(&config.owner_data_root).map_err(|_| FAILURE.to_string())?;
        let parts: Vec<String> = below(&owner, &canonical)
            .unwrap_or_default()
            .iter()
            .map(|part| part.to_string_lossy().into_owned())
            .collect();
        let id = |part: &str| {
            (1..=100).contains(&part.len())
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        };
        let assigned = parts.len() >= 2
            && ["bots", "workspaces"].contains(&parts[0].to_lowercase().as_str())
            && id(&parts[1]);
        if !assigned {
            return fail();
        }
    }
    Ok(canonical)
}
