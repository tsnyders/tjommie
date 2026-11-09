//! Codex data ACLs: a native port of `windows-acl.ts`. The plan (which folders and files get which ACE) is the same.
//! The PowerShell + icacls pass becomes GetNamedSecurityInfoW, SetEntriesInAclW and SetNamedSecurityInfoW, writing
//! what `icacls … (RX,W)` writes. `CodexSandboxUsers` is looked up on every pass (`windows-acl.ts:54`).

pub use protocol::model::Provider;
use protocol::model::SETUP_CODEX_SANDBOX_REASON;
use std::path::{Path, PathBuf};
use std::{fs, io, ptr};
use windows::Win32::Foundation::{ERROR_NONE_MAPPED, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSidToSidW, DENY_ACCESS, EXPLICIT_ACCESS_W, GRANT_ACCESS,
    GetNamedSecurityInfoW, SE_FILE_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW, TRUSTEE_IS_SID,
    TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION,
    DeleteAce, EqualSid, GetAce, GetSecurityDescriptorControl, INHERITED_ACE, LookupAccountNameW,
    NO_INHERITANCE, OBJECT_INHERIT_ACE, PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    PSID, SE_DACL_PROTECTED, SID_NAME_USE, SUB_CONTAINERS_AND_OBJECTS_INHERIT,
    UNPROTECTED_DACL_SECURITY_INFORMATION,
};
use windows::core::{HSTRING, PCWSTR, PWSTR};

const FAILURE_REASON: &str =
    "Tjommie couldn't check Codex's sandbox. If Codex was updated, run its sandbox setup again.";
/// What `icacls … (RX,W)` writes, measured: read, execute and write, plus READ_CONTROL and SYNCHRONIZE.
const RX_W: u32 = 0x0012_01bf;
/// `FileSystemRights.ReadAndExecute` and `.Write`, which the TypeScript check tests for.
const READ_AND_EXECUTE: u32 = 0x0002_00a9;
const WRITE: u32 = 0x0000_0116;
const SYNCHRONIZE: u32 = 0x0010_0000;
const ACCESS_ALLOWED: u8 = 0;
const ACCESS_DENIED: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Deny,
    Allow,
    RemoveDeny,
    VerifyBin,
    CheckDeny,
    CheckAllow,
    CheckNoDeny,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AclTarget {
    pub path: PathBuf,
    pub folder: bool,
    pub mode: Mode,
    pub codex_dependent: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PassResult {
    pub sid: Option<String>,
    pub group_missing: bool,
    pub failures: Vec<String>,
    pub applied: usize,
}

#[derive(Clone, Debug)]
pub struct ProtectedAccount {
    pub id: String,
    pub provider: Provider,
    pub folder: PathBuf,
}

#[derive(Clone, Debug)]
pub struct BotHome {
    pub id: String,
    pub home_folder: PathBuf,
    pub provider: Provider,
}

/// The `Paths` fields the pass needs (`paths.ts:8`). ponytail: a local copy so the lanes don't collide; replace it
/// with `paths::Paths` once T1.1 merges.
#[derive(Clone, Debug)]
pub struct ProtectedPaths {
    pub root: PathBuf,
    pub data: PathBuf,
    pub token: PathBuf,
    pub bots: PathBuf,
    pub accounts: PathBuf,
    pub logs: PathBuf,
    pub shared_files: PathBuf,
}

impl ProtectedPaths {
    /// `dataPaths(root)`, without creating the folders.
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            data: root.join("data"),
            token: root.join("daemon-token"),
            bots: root.join("bots"),
            accounts: root.join("accounts"),
            logs: root.join("logs"),
            shared_files: root.join("shared-files"),
        }
    }
}

#[derive(Debug, Default)]
pub struct Plan {
    pub targets: Vec<AclTarget>,
    pub failures: Vec<String>,
    pub expected: Vec<String>,
    pub skipped: Vec<String>,
    pub setup_missing: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protection {
    Ready,
    NotConfigured,
    CheckFailed,
}

impl Protection {
    pub fn reason(self) -> Option<&'static str> {
        match self {
            Self::Ready => None,
            Self::NotConfigured => Some(SETUP_CODEX_SANDBOX_REASON),
            Self::CheckFailed => Some(FAILURE_REASON),
        }
    }
}

/// A protection status and the lines the TypeScript daemon logs with `console.info` and `console.error`.
#[derive(Debug)]
pub struct Report {
    pub protection: Protection,
    pub info: Vec<String>,
    pub errors: Vec<String>,
}

/// `protectCodexData`: plan, run the native pass, log and return the status. Blocking; call it off the runtime.
pub fn protect_codex_data(
    paths: &ProtectedPaths,
    bot_homes: &[BotHome],
    accounts: Option<&[ProtectedAccount]>,
) -> Protection {
    let plan = plan_targets(paths, bot_homes, accounts);
    let report = finish(&plan, run_pass(&plan.targets));
    report.info.iter().for_each(|line| println!("{line}"));
    report.errors.iter().for_each(|line| eprintln!("{line}"));
    report.protection
}

/// `planTargets` (`windows-acl.ts:201-268`). With `accounts` unset, `claude-main` and `codex-main` are assumed and
/// a missing Codex sandbox means setup hasn't run yet.
pub fn plan_targets(
    paths: &ProtectedPaths,
    bot_homes: &[BotHome],
    accounts: Option<&[ProtectedAccount]>,
) -> Plan {
    let mut planner = Planner {
        plan: Plan::default(),
        paths,
    };
    let defaults;
    let owned = match accounts {
        Some(accounts) => accounts,
        None => {
            defaults = [
                ("claude-main", Provider::Claude),
                ("codex-main", Provider::Codex),
            ]
            .map(|(id, provider)| ProtectedAccount {
                id: id.into(),
                provider,
                folder: paths.accounts.join(id),
            });
            &defaults
        }
    };
    let mut valid = vec![];
    for account in owned {
        let id = account.id.as_str();
        if same_path(&account.folder, &paths.accounts.join(id))
            && !id.contains(['/', '\\'])
            && id != "."
            && id != ".."
        {
            valid.push(account);
        } else {
            planner
                .plan
                .failures
                .push(format!("{id}: account folder does not match its id"));
        }
    }
    for account in valid
        .iter()
        .filter(|account| account.provider == Provider::Claude)
    {
        planner.account(&account.folder, Mode::Deny, true, false);
    }
    for folder in [&paths.logs, &paths.data, &paths.shared_files] {
        planner.add(
            &paths.root,
            target(folder, true, Mode::Deny, false),
            true,
            None,
        );
    }
    planner.add(
        &paths.root,
        target(&paths.token, false, Mode::Deny, false),
        false,
        None,
    );
    match fs::read_dir(&paths.data) {
        Ok(entries) => {
            for entry in entries {
                let name = match entry {
                    Ok(entry) => entry.file_name(),
                    Err(error) => {
                        planner
                            .plan
                            .failures
                            .push(format!("database files: {error}"));
                        continue;
                    }
                };
                let is_database = name.to_str().is_some_and(|name| {
                    name.strip_prefix("tjommie.db")
                        .is_some_and(|rest| rest.is_empty() || rest.starts_with(['-', '.']))
                });
                if is_database {
                    planner.add(
                        &paths.root,
                        target(&paths.data.join(name), false, Mode::Deny, false),
                        false,
                        None,
                    );
                }
            }
        }
        Err(error) => planner
            .plan
            .failures
            .push(format!("database files: {error}")),
    }
    for bot in bot_homes {
        if !same_path(&bot.home_folder, &paths.bots.join(&bot.id)) {
            planner.plan.failures.push(format!(
                "{}: bot folder does not match its id",
                bot.home_folder.display()
            ));
            continue;
        }
        let (mode, name) = match bot.provider {
            Provider::Claude => (Mode::Deny, "Claude"),
            Provider::Codex => (Mode::RemoveDeny, "Codex"),
        };
        let missing = format!("{name} bot folder is missing; nothing to protect.");
        let present = planner.add(
            &paths.root,
            target(&bot.home_folder, true, mode, false),
            false,
            Some(&missing),
        );
        if bot.provider == Provider::Codex && present {
            planner.add(
                &paths.root,
                target(
                    &bot.home_folder.join("notes.md"),
                    false,
                    Mode::CheckNoDeny,
                    false,
                ),
                false,
                None,
            );
            planner.add(
                &paths.root,
                target(
                    &bot.home_folder.join("inbox"),
                    true,
                    Mode::CheckNoDeny,
                    false,
                ),
                false,
                None,
            );
        }
    }
    for account in valid
        .iter()
        .filter(|account| account.provider == Provider::Codex)
    {
        let home = &account.folder;
        let (sandbox, sandbox_bin) = (home.join(".sandbox"), home.join(".sandbox-bin"));
        // Presence probes: the verifyBin targets are dropped again unless the whole sandbox is there.
        let home_present = planner.account(home, Mode::VerifyBin, false, false);
        if home_present {
            planner.plan.targets.pop();
        }
        let sandbox_present =
            home_present && planner.account(&sandbox, Mode::VerifyBin, false, false);
        if sandbox_present {
            planner.plan.targets.pop();
        }
        if !home_present
            || !sandbox_present
            || !planner.account(&sandbox_bin, Mode::VerifyBin, false, false)
        {
            // A new, unconfigured account is still protected without blocking a configured one.
            if home_present {
                planner.account(home, Mode::Deny, true, false);
            }
            if accounts.is_none() {
                planner.plan.setup_missing = true;
            }
        } else {
            planner.account(home, Mode::Deny, true, true);
            planner.account(&sandbox, Mode::Allow, true, true);
        }
    }
    planner.plan
}

/// `finish` and `failedPass` (`windows-acl.ts:270-284`).
pub fn finish(plan: &Plan, result: Result<PassResult, String>) -> Report {
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            return Report {
                protection: Protection::CheckFailed,
                info: vec![],
                errors: vec![format!("Codex data ACLs: {error}")],
            };
        }
    };
    let mut info = vec![];
    if result.group_missing {
        info.push(format!("Codex data ACLs: CodexSandboxUsers group does not exist yet. {SETUP_CODEX_SANDBOX_REASON}."));
    }
    info.extend(
        plan.expected
            .iter()
            .chain(&plan.skipped)
            .map(|line| format!("Codex data ACLs: {line}")),
    );
    if plan.setup_missing {
        info.push("Codex data ACLs: Codex setup marker is not present yet.".into());
    }
    let errors: Vec<String> = plan
        .failures
        .iter()
        .chain(&result.failures)
        .map(|line| format!("Codex data ACLs: {line}"))
        .collect();
    let protection = if !errors.is_empty() || !plan.expected.is_empty() {
        Protection::CheckFailed
    } else if result.group_missing || plan.setup_missing {
        Protection::NotConfigured
    } else {
        Protection::Ready
    };
    Report {
        protection,
        info,
        errors,
    }
}

/// The ACL pass for `CodexSandboxUsers`, or `group_missing` when that group doesn't exist yet.
pub fn run_pass(targets: &[AclTarget]) -> Result<PassResult, String> {
    match sandbox_group()? {
        Some(sid) => run_pass_for(&sid, targets),
        None => Ok(PassResult {
            sid: None,
            group_missing: true,
            failures: vec![],
            applied: 0,
        }),
    }
}

/// The ACL pass for any group SID (`windows-acl.ts:93-121`): checks each target, applies a missing ACE and checks
/// again. Targets marked `codex_dependent` are skipped once a `.sandbox-bin` check has failed.
pub fn run_pass_for(sid: &str, targets: &[AclTarget]) -> Result<PassResult, String> {
    if !valid_sid(sid) {
        return Err("Invalid Codex sandbox group SID".into());
    }
    let group = Sid::parse(sid)?;
    let (mut failures, mut applied, mut bin_ready) = (vec![], 0, true);
    for target in targets {
        if target.codex_dependent && !bin_ready {
            continue;
        }
        let path = target.path.display();
        let mut step = || -> Result<Option<String>, String> {
            let present = test_target(target, &group)?;
            match target.mode {
                Mode::VerifyBin if !present => {
                    return Ok(Some(format!(
                        "{path}: .sandbox-bin needs a protected CodexSandboxUsers ReadAndExecute allow"
                    )));
                }
                Mode::CheckDeny | Mode::CheckAllow | Mode::CheckNoDeny if !present => {
                    return Ok(Some(format!("{path}: expected ACL state was not found")));
                }
                Mode::VerifyBin | Mode::CheckDeny | Mode::CheckAllow | Mode::CheckNoDeny => {
                    return Ok(None);
                }
                Mode::Deny | Mode::Allow | Mode::RemoveDeny if present => return Ok(None),
                Mode::Deny | Mode::Allow | Mode::RemoveDeny => apply(target, &group)?,
            }
            applied += 1;
            if !test_target(target, &group)? {
                return Err("ACE was not verified after applying".into());
            }
            Ok(None)
        };
        let failure = step().unwrap_or_else(|error| Some(format!("{path}: {error}")));
        if let Some(failure) = failure {
            if target.mode == Mode::VerifyBin {
                bin_ready = false;
            }
            failures.push(failure);
        }
    }
    Ok(PassResult {
        sid: Some(sid.into()),
        group_missing: false,
        failures,
        applied,
    })
}

fn target(path: &Path, folder: bool, mode: Mode, codex_dependent: bool) -> AclTarget {
    AclTarget {
        path: path.to_path_buf(),
        folder,
        mode,
        codex_dependent,
    }
}

struct Planner<'a> {
    plan: Plan,
    paths: &'a ProtectedPaths,
}

impl Planner<'_> {
    /// `add` (`windows-acl.ts:203-217`): queues the target if it is present and safe, under `anchor`.
    fn add(
        &mut self,
        anchor: &Path,
        target: AclTarget,
        required: bool,
        missing: Option<&str>,
    ) -> bool {
        match present(anchor, &target.path, target.folder) {
            Ok(true) => {
                self.plan.targets.push(target);
                true
            }
            Ok(false) => {
                if required {
                    self.plan.expected.push(format!(
                        "{}: required folder is missing",
                        target.path.display()
                    ));
                } else if let Some(missing) = missing {
                    self.plan
                        .skipped
                        .push(format!("{}: {missing}", target.path.display()));
                }
                false
            }
            Err(error) => {
                self.plan
                    .failures
                    .push(format!("{}: {error}", target.path.display()));
                false
            }
        }
    }

    /// Account folders may live outside the data root, so they are contained by the account root instead.
    fn account(&mut self, path: &Path, mode: Mode, required: bool, codex_dependent: bool) -> bool {
        let accounts = self.paths.accounts.clone();
        self.add(
            &accounts,
            target(path, true, mode, codex_dependent),
            required,
            None,
        )
    }
}

/// `targetIfPresent` (`windows-acl.ts:179-198`): `false` if anything on the way is missing; an error if the path
/// escapes `root`, runs through a junction or symlink, or has the wrong type.
fn present(root: &Path, path: &Path, folder: bool) -> Result<bool, String> {
    let check = || -> io::Result<()> {
        let parts = below(root, path)
            .ok_or_else(|| io::Error::other("protected path is outside its protection root"))?;
        let mut ancestor = std::path::absolute(root)?;
        if fs::symlink_metadata(&ancestor)?.is_symlink() {
            return Err(io::Error::other("protection root is a reparse point"));
        }
        for part in &parts[..parts.len() - 1] {
            ancestor.push(part);
            if fs::symlink_metadata(&ancestor)?.is_symlink() {
                return Err(io::Error::other(
                    "protected path has a reparse-point parent",
                ));
            }
        }
        let stat = fs::symlink_metadata(path)?;
        if stat.is_symlink()
            || if folder {
                !stat.is_dir()
            } else {
                !stat.is_file()
            }
        {
            return Err(io::Error::other("unexpected path type"));
        }
        Ok(())
    };
    match check() {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

/// The parts of `path` below `root`, compared without case like Node's `path.relative` on Windows. `None` when the
/// path is the root itself or outside it.
fn below(root: &Path, path: &Path) -> Option<Vec<PathBuf>> {
    let (root, path) = (parts(root)?, parts(path)?);
    let inside = path.len() > root.len()
        && root
            .iter()
            .zip(&path)
            .all(|(a, b)| a.to_lowercase() == b.to_lowercase());
    inside.then(|| path[root.len()..].iter().map(PathBuf::from).collect())
}

fn parts(path: &Path) -> Option<Vec<String>> {
    let absolute = std::path::absolute(path).ok()?;
    absolute
        .components()
        .map(|part| part.as_os_str().to_str().map(str::to_owned))
        .collect()
}

/// `resolve(a).toLowerCase() === resolve(b).toLowerCase()`.
fn same_path(a: &Path, b: &Path) -> bool {
    match (parts(a), parts(b)) {
        (Some(a), Some(b)) => {
            a.len() == b.len()
                && a.iter()
                    .zip(&b)
                    .all(|(a, b)| a.to_lowercase() == b.to_lowercase())
        }
        _ => false,
    }
}

/// `^S-1-\d+(?:-\d+)+$`
fn valid_sid(sid: &str) -> bool {
    sid.strip_prefix("S-1-").is_some_and(|rest| {
        let parts: Vec<&str> = rest.split('-').collect();
        parts.len() >= 2
            && parts
                .iter()
                .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
    })
}

/// `<COMPUTERNAME>\CodexSandboxUsers`, like the TypeScript pass's NTAccount lookup. `None` if it doesn't exist.
fn sandbox_group() -> Result<Option<String>, String> {
    let computer =
        std::env::var("COMPUTERNAME").map_err(|_| "COMPUTERNAME is not set".to_string())?;
    let name = HSTRING::from(format!("{computer}\\CodexSandboxUsers"));
    let mut sid = [0u8; 68]; // SECURITY_MAX_SID_SIZE
    let mut domain = [0u16; 256];
    let (mut sid_size, mut domain_size, mut kind) = (
        sid.len() as u32,
        domain.len() as u32,
        SID_NAME_USE::default(),
    );
    // SAFETY: both buffers are local and their sizes are passed alongside them.
    let found = unsafe {
        LookupAccountNameW(
            PCWSTR::null(),
            &name,
            Some(PSID(sid.as_mut_ptr().cast())),
            &mut sid_size,
            Some(PWSTR(domain.as_mut_ptr())),
            &mut domain_size,
            &mut kind,
        )
    };
    match found {
        Err(error) if error.code() == ERROR_NONE_MAPPED.to_hresult() => Ok(None),
        Err(error) => Err(format!(
            "the Codex sandbox group could not be looked up: {error}"
        )),
        Ok(()) => {
            let mut text = PWSTR::null();
            // SAFETY: `sid` holds the SID LookupAccountNameW wrote; the string is freed with LocalFree.
            unsafe {
                ConvertSidToStringSidW(PSID(sid.as_mut_ptr().cast()), &mut text)
                    .map_err(|error| error.to_string())?;
                let value = text.to_string().map_err(|error| error.to_string());
                LocalFree(Some(HLOCAL(text.0.cast())));
                let value = value?;
                if valid_sid(&value) {
                    Ok(Some(value))
                } else {
                    Err("Invalid Codex sandbox group SID".into())
                }
            }
        }
    }
}

/// A SID parsed from its string form, freed on drop.
struct Sid(PSID);

impl Sid {
    fn parse(sid: &str) -> Result<Self, String> {
        let mut parsed = PSID::default();
        // SAFETY: on success the system allocates the SID; Drop frees it.
        unsafe { ConvertStringSidToSidW(&HSTRING::from(sid), &mut parsed) }
            .map_err(|error| error.to_string())?;
        Ok(Self(parsed))
    }

    fn equals(&self, other: PSID) -> bool {
        // SAFETY: both point at valid SIDs for the duration of the call.
        unsafe { EqualSid(self.0, other) }.is_ok()
    }
}

impl Drop for Sid {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSidToSidW and freed once.
        unsafe { LocalFree(Some(HLOCAL(self.0.0))) };
    }
}

/// An allow or deny ACE; other ACE types are ignored, like the TypeScript pass's access rules.
struct Ace {
    index: u32,
    denied: bool,
    flags: u32,
    mask: u32,
    sid: PSID,
    /// The ACE inside the DACL, for editing its mask in place.
    raw: *mut ACCESS_ALLOWED_ACE,
}

/// A file's security descriptor with its DACL, freed on drop.
struct Descriptor {
    descriptor: PSECURITY_DESCRIPTOR,
    dacl: *mut ACL,
}

impl Descriptor {
    fn read(path: &Path) -> Result<Self, String> {
        let (mut descriptor, mut dacl) = (PSECURITY_DESCRIPTOR::default(), ptr::null_mut());
        // SAFETY: on success the system allocates one buffer holding the descriptor and the DACL; Drop frees it.
        let error = unsafe {
            GetNamedSecurityInfoW(
                &HSTRING::from(path),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(&mut dacl),
                None,
                &mut descriptor,
            )
        };
        error.ok().map_err(|error| error.to_string())?;
        Ok(Self { descriptor, dacl })
    }

    fn protected(&self) -> Result<bool, String> {
        let (mut control, mut revision) = (0u16, 0u32);
        // SAFETY: the descriptor is valid while `self` lives.
        unsafe { GetSecurityDescriptorControl(self.descriptor, &mut control, &mut revision) }
            .map_err(|error| error.to_string())?;
        Ok(control & SE_DACL_PROTECTED.0 != 0)
    }

    fn aces(&self) -> Vec<Ace> {
        if self.dacl.is_null() {
            return vec![];
        }
        // SAFETY: the DACL is valid while `self` lives; GetAce returns pointers into it.
        unsafe {
            (0..u32::from((*self.dacl).AceCount))
                .filter_map(|index| {
                    let mut ace = ptr::null_mut();
                    GetAce(self.dacl, index, &mut ace).ok()?;
                    let header = &*(ace as *const ACE_HEADER);
                    if header.AceType != ACCESS_ALLOWED && header.AceType != ACCESS_DENIED {
                        return None;
                    }
                    // Allowed and denied ACEs share this layout.
                    let raw = ace.cast::<ACCESS_ALLOWED_ACE>();
                    Some(Ace {
                        index,
                        denied: header.AceType == ACCESS_DENIED,
                        flags: u32::from(header.AceFlags),
                        mask: (*raw).Mask,
                        sid: PSID(ptr::addr_of_mut!((*raw).SidStart).cast()),
                        raw,
                    })
                })
                .collect()
        }
    }

    /// Writes `dacl`, keeping the file's inheritance protection as it is (like Set-Acl and icacls).
    fn write(&self, path: &Path, dacl: *const ACL) -> Result<(), String> {
        let protection = if self.protected()? {
            PROTECTED_DACL_SECURITY_INFORMATION
        } else {
            UNPROTECTED_DACL_SECURITY_INFORMATION
        };
        // SAFETY: `dacl` is a valid ACL for the duration of the call.
        let error = unsafe {
            SetNamedSecurityInfoW(
                &HSTRING::from(path),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | protection,
                None,
                None,
                Some(dacl),
                None,
            )
        };
        error.ok().map_err(|error| error.to_string())
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: allocated by GetNamedSecurityInfoW and freed once.
        unsafe { LocalFree(Some(HLOCAL(self.descriptor.0))) };
    }
}

/// `Test-Target` (`windows-acl.ts:67-92`).
fn test_target(target: &AclTarget, group: &Sid) -> Result<bool, String> {
    let descriptor = Descriptor::read(&target.path)?;
    if target.mode == Mode::VerifyBin && !descriptor.protected()? {
        return Ok(false);
    }
    let removing = matches!(target.mode, Mode::RemoveDeny | Mode::CheckNoDeny);
    for ace in descriptor.aces().iter().filter(|ace| group.equals(ace.sid)) {
        if removing {
            if ace.denied {
                return Ok(false);
            }
            continue;
        }
        if ace.flags & INHERITED_ACE.0 != 0 {
            continue;
        }
        if target.mode == Mode::VerifyBin {
            if !ace.denied && ace.mask & READ_AND_EXECUTE == READ_AND_EXECUTE {
                return Ok(true);
            }
            continue;
        }
        let want_denied = matches!(target.mode, Mode::Deny | Mode::CheckDeny);
        if ace.denied != want_denied
            || ace.mask & READ_AND_EXECUTE != READ_AND_EXECUTE
            || ace.mask & WRITE != WRITE
        {
            continue;
        }
        let inheritable = OBJECT_INHERIT_ACE.0 | CONTAINER_INHERIT_ACE.0;
        if target.folder && ace.flags & inheritable != inheritable {
            continue;
        }
        return Ok(true);
    }
    Ok(removing)
}

/// What icacls does for the pass: `/deny` and `/grant` with `(OI)(CI)(RX,W)` (or `(RX,W)` on a file), and
/// `/remove:d`. Setting the DACL through SetNamedSecurityInfoW also updates inherited ACEs below a folder.
fn apply(target: &AclTarget, group: &Sid) -> Result<(), String> {
    let descriptor = Descriptor::read(&target.path)?;
    if target.mode == Mode::RemoveDeny {
        for ace in descriptor.aces().iter().rev() {
            if ace.denied && ace.flags & INHERITED_ACE.0 == 0 && group.equals(ace.sid) {
                // SAFETY: the index came from this DACL; deleting from the end keeps the others valid.
                unsafe { DeleteAce(descriptor.dacl, ace.index) }
                    .map_err(|error| error.to_string())?;
            }
        }
        return descriptor.write(&target.path, descriptor.dacl);
    }
    if target.mode == Mode::Deny {
        // icacls /deny also takes the denied rights out of this SID's explicit allows, keeping SYNCHRONIZE, and drops
        // an allow left with nothing else (measured; SetEntriesInAclW alone keeps them). /grant's mirror image is
        // what SetEntriesInAclW's GRANT_ACCESS already does.
        for ace in descriptor.aces().iter().rev() {
            if ace.denied || ace.flags & INHERITED_ACE.0 != 0 || !group.equals(ace.sid) {
                continue;
            }
            let rest = ace.mask & !(RX_W & !SYNCHRONIZE);
            // SAFETY: the ACE and index came from this DACL; editing from the end keeps the other indexes valid.
            unsafe {
                if rest & !SYNCHRONIZE == 0 {
                    DeleteAce(descriptor.dacl, ace.index).map_err(|error| error.to_string())?;
                } else {
                    (*ace.raw).Mask = rest;
                }
            }
        }
    }
    let access = EXPLICIT_ACCESS_W {
        grfAccessPermissions: RX_W,
        grfAccessMode: if target.mode == Mode::Deny {
            DENY_ACCESS
        } else {
            GRANT_ACCESS
        },
        grfInheritance: if target.folder {
            SUB_CONTAINERS_AND_OBJECTS_INHERIT
        } else {
            NO_INHERITANCE
        },
        Trustee: TRUSTEE_W {
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: PWSTR(group.0.0.cast()),
            ..Default::default()
        },
    };
    let mut updated = ptr::null_mut();
    // SAFETY: the old DACL is valid while `descriptor` lives; the new one is freed below.
    unsafe { SetEntriesInAclW(Some(&[access]), Some(descriptor.dacl), &mut updated) }
        .ok()
        .map_err(|error| error.to_string())?;
    let written = descriptor.write(&target.path, updated);
    // SAFETY: allocated by SetEntriesInAclW and freed once.
    unsafe { LocalFree(Some(HLOCAL(updated.cast()))) };
    written
}
