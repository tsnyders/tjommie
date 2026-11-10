//! Native worker identity calls, ported from `windows-worker.ps1` (C# P/Invoke), `windows-worker.ts` (the directory
//! grant) and `scripts/vault/provision-worker.ps1`. Creating accounts, adding group members and creating `Global\`
//! objects need an elevated owner; the tests that do so run only with `TJOMMIE_RS_ADMIN_TESTS=1`.

use crate::win::cred::{self, Namespace};
use crate::win::worker::{
    ADMINISTRATORS, BOOTSTRAP, FAILURE, MutexWait, Purpose, SYSTEM, WorkerConfiguration,
    assert_job_descriptor, assert_mutex_descriptor, assert_runtime_tree, command_line,
    enter_review_mutex, unreceipted_provider, worker_environment, worker_name, write_review_marker,
};
use base64::Engine;
use std::fs::{self, File};
use std::os::windows::io::FromRawHandle;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use std::{mem, ptr};
use windows::Wdk::System::Threading::{NtQueryInformationProcess, ProcessCommandLineInformation};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_NONE_MAPPED, GetLastError, HANDLE,
    HANDLE_FLAG_INHERIT, HLOCAL, LocalFree, SetHandleInformation, SetLastError, UNICODE_STRING,
    WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT, WIN32_ERROR,
};
use windows::Win32::NetworkManagement::NetManagement::{
    LOCALGROUP_MEMBERS_INFO_0, NERR_UserNotFound, NetApiBufferFree, NetLocalGroupAddMembers,
    NetUserAdd, NetUserDel, NetUserGetInfo, UF_ACCOUNTDISABLE, UF_DONT_EXPIRE_PASSWD,
    UF_PASSWD_CANT_CHANGE, UF_SCRIPT, USER_INFO_1, USER_PRIV_USER,
};
use windows::Win32::Security::Authorization::{
    ConvertSecurityDescriptorToStringSecurityDescriptorW, ConvertSidToStringSidW,
    ConvertStringSecurityDescriptorToSecurityDescriptorW, ConvertStringSidToSidW,
    EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW, GetSecurityInfo, SDDL_REVISION_1,
    SE_FILE_OBJECT, SE_KERNEL_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW, SetSecurityInfo,
    TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows::Win32::Security::Cryptography::{BCRYPT_USE_SYSTEM_PREFERRED_RNG, BCryptGenRandom};
use windows::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION,
    GetAce, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner, GetTokenInformation,
    LookupAccountNameW, LookupAccountSidW, OWNER_SECURITY_INFORMATION,
    PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
    SID_AND_ATTRIBUTES, SID_NAME_USE, SUB_CONTAINERS_AND_OBJECTS_INHERIT, TOKEN_ELEVATION,
    TOKEN_GROUPS, TOKEN_INFORMATION_CLASS, TOKEN_PRIVILEGES, TOKEN_QUERY, TOKEN_USER,
    TokenElevation, TokenGroups, TokenPrivileges, TokenUser,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_NAME_NORMALIZED, FILE_SHARE_READ,
    FILE_SHARE_WRITE, GetFinalPathNameByHandleW, OPEN_EXISTING,
};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, IsProcessInJob, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};
use windows::Win32::System::Pipes::CreatePipe;
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, CloseWindowStation, CreateDesktopW, CreateWindowStationW, DESKTOP_CONTROL_FLAGS,
    GetProcessWindowStation, HDESK, HWINSTA, SetProcessWindowStation,
};
use windows::Win32::System::SystemInformation::GetTickCount64;
use windows::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateMutexW,
    CreateProcessWithLogonW, GetCurrentProcess, GetExitCodeProcess, INFINITE, LOGON_WITH_PROFILE,
    OpenProcess, OpenProcessToken, PROCESS_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION,
    ReleaseMutex, ResumeThread, STARTF_USESHOWWINDOW, STARTF_USESTDHANDLES, STARTUPINFOW,
    TerminateProcess, WaitForSingleObject,
};
use windows::core::{BOOL, HSTRING, PCWSTR, PWSTR};

/// Credential Manager user name for worker passwords (`scripts/vault/worker-credential.ps1`), not `Tjommie`.
const WORKER_CREDENTIAL_USER: &str = "Tjommie isolated worker";
const USERS: &str = "S-1-5-32-545";
/// Modify, as .NET writes it for an allow (with SYNCHRONIZE).
const MODIFY: u32 = 0x0013_01bf;
const READ_AND_EXECUTE: u32 = 0x0012_00a9;
const WRITE: u32 = 0x0000_0116;
const CHANGE_PERMISSIONS: u32 = 0x0004_0000;
const TAKE_OWNERSHIP: u32 = 0x0008_0000;
/// Privileges an ordinary user token holds: shutdown, change notify, undock, working set, time zone.
const ORDINARY_PRIVILEGES: [u32; 5] = [19, 23, 25, 33, 34];

fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

fn sid_string(sid: PSID) -> Result<String, String> {
    let mut out = PWSTR::null();
    // SAFETY: `sid` is valid for the call; the string is freed once with LocalFree.
    unsafe {
        ConvertSidToStringSidW(sid, &mut out).map_err(text)?;
        let value = out.to_string().map_err(text);
        LocalFree(Some(HLOCAL(out.0.cast())));
        value
    }
}

/// A SID parsed from text, freed on drop.
struct Sid(PSID);

impl Sid {
    fn parse(sid: &str) -> Result<Self, String> {
        let mut parsed = PSID::default();
        // SAFETY: on success the system allocates the SID; Drop frees it.
        unsafe { ConvertStringSidToSidW(&HSTRING::from(sid), &mut parsed) }.map_err(text)?;
        Ok(Self(parsed))
    }
}

impl Drop for Sid {
    fn drop(&mut self) {
        // SAFETY: allocated by ConvertStringSidToSidW, freed once.
        unsafe { LocalFree(Some(HLOCAL(self.0.0))) };
    }
}

/// An owned handle, closed on drop.
struct Owned(HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_invalid() && !self.0.0.is_null() {
            // SAFETY: owned and closed once.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

/// An ACE as the checks see it.
#[derive(Clone, Debug)]
pub struct Ace {
    pub kind: u8,
    pub flags: u8,
    pub mask: u32,
    pub sid: String,
}

impl Ace {
    pub const ALLOWED: u8 = 0;
    pub const DENIED: u8 = 1;
}

/// A security descriptor (owner and DACL) from SDDL, a path or a kernel object; freed on drop.
pub struct Descriptor(PSECURITY_DESCRIPTOR);

impl Descriptor {
    pub fn from_sddl(sddl: &str) -> Result<Self, String> {
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: on success the system allocates the descriptor; Drop frees it.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                &HSTRING::from(sddl),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
        }
        .map_err(text)?;
        Ok(Self(descriptor))
    }

    pub fn of_path(path: &Path) -> Result<Self, String> {
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        let info = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
        // SAFETY: on success the system allocates the descriptor; Drop frees it.
        unsafe {
            GetNamedSecurityInfoW(
                &HSTRING::from(path),
                SE_FILE_OBJECT,
                info,
                None,
                None,
                None,
                None,
                &mut descriptor,
            )
        }
        .ok()
        .map_err(text)?;
        Ok(Self(descriptor))
    }

    fn of_object(handle: HANDLE) -> Result<Self, String> {
        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        let info = OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
        // SAFETY: as above, for a kernel object handle.
        unsafe {
            GetSecurityInfo(
                handle,
                SE_KERNEL_OBJECT,
                info,
                None,
                None,
                None,
                None,
                Some(&mut descriptor),
            )
        }
        .ok()
        .map_err(text)?;
        Ok(Self(descriptor))
    }

    pub fn owner(&self) -> Result<Option<String>, String> {
        let (mut owner, mut defaulted) = (PSID::default(), BOOL::default());
        // SAFETY: the descriptor is valid while `self` lives.
        unsafe { GetSecurityDescriptorOwner(self.0, &mut owner, &mut defaulted) }.map_err(text)?;
        if owner.0.is_null() {
            Ok(None)
        } else {
            sid_string(owner).map(Some)
        }
    }

    fn dacl(&self) -> Result<*mut ACL, String> {
        let (mut present, mut dacl, mut defaulted) =
            (BOOL::default(), ptr::null_mut(), BOOL::default());
        // SAFETY: the descriptor is valid while `self` lives.
        unsafe { GetSecurityDescriptorDacl(self.0, &mut present, &mut dacl, &mut defaulted) }
            .map_err(text)?;
        Ok(if present.as_bool() {
            dacl
        } else {
            ptr::null_mut()
        })
    }

    /// The DACL's ACEs, or `None` for a missing or null DACL (which allows everyone everything).
    pub fn aces(&self) -> Result<Option<Vec<Ace>>, String> {
        let dacl = self.dacl()?;
        if dacl.is_null() {
            return Ok(None);
        }
        let mut aces = vec![];
        // SAFETY: the DACL lives inside the descriptor; GetAce returns pointers into it.
        unsafe {
            for index in 0..u32::from((*dacl).AceCount) {
                let mut ace = ptr::null_mut();
                GetAce(dacl, index, &mut ace).map_err(text)?;
                let header = &*(ace as *const ACE_HEADER);
                let known = header.AceType == Ace::ALLOWED || header.AceType == Ace::DENIED;
                // Allowed and denied ACEs share this layout; other types are reported without a SID, so no check trusts them.
                let (mask, sid) = if known {
                    let body = &*(ace as *const ACCESS_ALLOWED_ACE);
                    (
                        body.Mask,
                        sid_string(PSID(ptr::addr_of!(body.SidStart).cast_mut().cast()))?,
                    )
                } else {
                    (0, String::new())
                };
                aces.push(Ace {
                    kind: header.AceType,
                    flags: header.AceFlags,
                    mask,
                    sid,
                });
            }
        }
        Ok(Some(aces))
    }

    /// Writes this descriptor's owner and DACL to `path`, with inheritance from above turned off.
    fn apply_protected(&self, path: &Path) -> Result<(), String> {
        let (mut owner, mut defaulted) = (PSID::default(), BOOL::default());
        // SAFETY: owner and DACL point into this descriptor, valid for the call.
        unsafe {
            GetSecurityDescriptorOwner(self.0, &mut owner, &mut defaulted).map_err(text)?;
            let info = OWNER_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | PROTECTED_DACL_SECURITY_INFORMATION;
            SetNamedSecurityInfoW(
                &HSTRING::from(path),
                SE_FILE_OBJECT,
                info,
                Some(owner),
                None,
                Some(self.dacl()?),
                None,
            )
            .ok()
            .map_err(text)
        }
    }
}

impl Drop for Descriptor {
    fn drop(&mut self) {
        // SAFETY: allocated by the system call that made it, freed once.
        unsafe { LocalFree(Some(HLOCAL(self.0.0))) };
    }
}

/// A path's owner, group and DACL as SDDL.
pub fn sddl(path: &Path) -> Result<String, String> {
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    let info = OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION;
    let mut out = PWSTR::null();
    // SAFETY: both buffers come from the system and are freed once.
    unsafe {
        GetNamedSecurityInfoW(
            &HSTRING::from(path),
            SE_FILE_OBJECT,
            info,
            None,
            None,
            None,
            None,
            &mut descriptor,
        )
        .ok()
        .map_err(text)?;
        let converted = ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor,
            SDDL_REVISION_1,
            info,
            &mut out,
            None,
        );
        LocalFree(Some(HLOCAL(descriptor.0)));
        converted.map_err(text)?;
        let value = out.to_string().map_err(text);
        LocalFree(Some(HLOCAL(out.0.cast())));
        value
    }
}

/// Reads one token information class into an aligned buffer.
fn token_info(token: HANDLE, class: TOKEN_INFORMATION_CLASS) -> Result<Vec<u64>, String> {
    let mut needed = 0u32;
    // SAFETY: the first call only reports the size; the second fills a buffer of that size.
    unsafe {
        let _ = GetTokenInformation(token, class, None, 0, &mut needed);
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8).max(1)];
        GetTokenInformation(
            token,
            class,
            Some(buffer.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )
        .map_err(text)?;
        Ok(buffer)
    }
}

fn process_token(process: HANDLE) -> Result<Owned, String> {
    let mut token = HANDLE::default();
    // SAFETY: the token handle is owned by the returned guard.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.map_err(text)?;
    Ok(Owned(token))
}

fn token_user(token: HANDLE) -> Result<String, String> {
    let buffer = token_info(token, TokenUser)?;
    // SAFETY: the buffer holds a TOKEN_USER whose SID points into the same buffer.
    sid_string(unsafe { (*(buffer.as_ptr() as *const TOKEN_USER)).User.Sid })
}

fn elevated(token: HANDLE) -> Result<bool, String> {
    let buffer = token_info(token, TokenElevation)?;
    // SAFETY: the buffer holds a TOKEN_ELEVATION.
    Ok(unsafe { (*(buffer.as_ptr() as *const TOKEN_ELEVATION)).TokenIsElevated } != 0)
}

/// The SID of the account this process runs as.
pub fn current_user_sid() -> Result<String, String> {
    // SAFETY: the pseudo handle needs no closing.
    let token = process_token(unsafe { GetCurrentProcess() })?;
    token_user(token.0)
}

/// `Verify` (`windows-worker.ps1:131-151`): the process runs as the worker, unelevated, in no administrator or
/// SYSTEM group, with only ordinary user privileges. Stricter than the TypeScript helper, which skipped deny-only
/// groups: a worker whose token carries Administrators at all, even deny-only (a UAC-filtered admin), is refused.
pub fn verify_token(process: HANDLE, worker_sid: &str) -> Result<(), String> {
    let token = process_token(process)?;
    if token_user(token.0)? != worker_sid {
        return Err(FAILURE.into());
    }
    let groups = token_info(token.0, TokenGroups)?;
    // SAFETY: the buffer holds a TOKEN_GROUPS with GroupCount entries.
    let groups: Vec<SID_AND_ATTRIBUTES> = unsafe {
        let header = &*(groups.as_ptr() as *const TOKEN_GROUPS);
        std::slice::from_raw_parts(header.Groups.as_ptr(), header.GroupCount as usize).to_vec()
    };
    for group in groups {
        let sid = sid_string(group.Sid)?;
        if sid == ADMINISTRATORS || sid == SYSTEM {
            return Err(FAILURE.into());
        }
    }
    if elevated(token.0)? {
        return Err(FAILURE.into());
    }
    let privileges = token_info(token.0, TokenPrivileges)?;
    // SAFETY: the buffer holds a TOKEN_PRIVILEGES with PrivilegeCount entries.
    let privileges = unsafe {
        let header = &*(privileges.as_ptr() as *const TOKEN_PRIVILEGES);
        std::slice::from_raw_parts(header.Privileges.as_ptr(), header.PrivilegeCount as usize)
            .to_vec()
    };
    if privileges
        .iter()
        .all(|p| p.Luid.HighPart == 0 && ORDINARY_PRIVILEGES.contains(&p.Luid.LowPart))
    {
        Ok(())
    } else {
        Err(FAILURE.into())
    }
}

/// A job object with kill-on-close: closing the last handle kills every process in it.
pub struct Job(Owned);

impl Job {
    /// `BindJob` (`windows-worker.ps1:167-179`): a new job only the owner, SYSTEM and Administrators can open,
    /// limited to kill-on-close, with `process` assigned. A name that already exists is refused.
    pub fn bind(process: HANDLE, name: Option<&str>, owner_sid: &str) -> Result<Self, String> {
        let descriptor = Descriptor::from_sddl(&format!(
            "O:{owner_sid}D:P(A;;GA;;;{owner_sid})(A;;GA;;;SY)(A;;GA;;;BA)"
        ))?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0.0,
            bInheritHandle: false.into(),
        };
        let name = name.map(HSTRING::from);
        // SAFETY: the attributes and name outlive the call; the handle is owned by the returned job.
        let job = unsafe {
            // Windows only promises ERROR_ALREADY_EXISTS when the name existed; clear any stale value first.
            SetLastError(WIN32_ERROR(0));
            let handle = CreateJobObjectW(
                Some(&attributes),
                name.as_ref().map_or(PCWSTR::null(), |n| PCWSTR(n.as_ptr())),
            )
            .map_err(text)?;
            let existed = GetLastError() == ERROR_ALREADY_EXISTS;
            let job = Self(Owned(handle));
            if existed {
                return Err("A job name already exists.".into());
            }
            job
        };
        assert_job_descriptor(&Descriptor::of_object(job.0.0)?, owner_sid)?;
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `limits` is a live, correctly sized struct; both handles are valid.
        unsafe {
            SetInformationJobObject(
                job.0.0,
                JobObjectExtendedLimitInformation,
                ptr::addr_of!(limits).cast(),
                mem::size_of_val(&limits) as u32,
            )
            .map_err(text)?;
            AssignProcessToJobObject(job.0.0, process).map_err(text)?;
        }
        Ok(job)
    }

    /// How many processes are still in the job.
    pub fn active(&self) -> Result<u32, String> {
        let mut info = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        // SAFETY: `info` is a live, correctly sized struct.
        unsafe {
            QueryInformationJobObject(
                Some(self.0.0),
                JobObjectBasicAccountingInformation,
                ptr::addr_of_mut!(info).cast(),
                mem::size_of_val(&info) as u32,
                None,
            )
        }
        .map_err(text)?;
        Ok(info.ActiveProcesses)
    }

    /// `StopJob` (`windows-worker.ps1:216-221`): kill everything with exit code 71 and confirm, within 5 s, that
    /// nothing is left. `false` means the tree could not be confirmed stopped.
    pub fn stop(&self) -> bool {
        // SAFETY: the job handle is valid while `self` lives.
        if unsafe { TerminateJobObject(self.0.0, 71) }.is_err() {
            return false;
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match self.active() {
                Ok(0) => return true,
                Ok(_) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
                _ => return false,
            }
        }
    }

    /// Whether `pid` is in this job, asked of the kernel with the process pinned open.
    pub fn contains(&self, pid: u32) -> Result<bool, String> {
        // SAFETY: the process handle is owned by the guard; IsProcessInJob only reads.
        unsafe {
            let process =
                Owned(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).map_err(text)?);
            let mut member = BOOL::default();
            IsProcessInJob(process.0, Some(self.0.0), &mut member).map_err(text)?;
            Ok(member.as_bool())
        }
    }
}

/// `CreateWindowStation` and `SetProcessWindowStation` act on the whole process; one worker at a time switches.
static STATION: Mutex<()> = Mutex::new(());

/// A worker process started suspended under its own account, on a private window station and desktop, with its
/// standard handles piped to the daemon (`TjNativeWorker`, `windows-worker.ps1:232-284`).
pub struct NativeWorker {
    process: Owned,
    thread: Mutex<Option<Owned>>,
    station: HWINSTA,
    desktop: HDESK,
    stdin: Option<File>,
    stdout: Option<File>,
    stderr: Option<File>,
}

// SAFETY: the station and desktop handles are only closed in Drop; the rest are owned handles.
unsafe impl Send for NativeWorker {}
unsafe impl Sync for NativeWorker {}

impl NativeWorker {
    /// `TjNativeWorker.Start`. `command_line` already starts with the quoted `file`. Creation flags are
    /// CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED (`0x08000404`), as in the TypeScript helper.
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        user: &str,
        password: &str,
        file: &str,
        command_line: &str,
        cwd: &Path,
        environment: &[u16],
        owner_sid: &str,
        worker_sid: &str,
    ) -> Result<Self, String> {
        let descriptor = Descriptor::from_sddl(&format!(
            "D:(A;;GA;;;{owner_sid})(A;;GA;;;{worker_sid})(A;;GA;;;SY)"
        ))?;
        let secured = SECURITY_ATTRIBUTES {
            nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0.0,
            bInheritHandle: false.into(),
        };
        let station_name = format!("TjommieWorker-{}", random_hex(16)?);
        let mut worker = Self {
            process: Owned(HANDLE::default()),
            thread: Mutex::new(None),
            station: HWINSTA::default(),
            desktop: HDESK::default(),
            stdin: None,
            stdout: None,
            stderr: None,
        };
        {
            let _switch = STATION
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // SAFETY: the station is created, switched to only to create its desktop, and switched back.
            unsafe {
                worker.station =
                    CreateWindowStationW(&HSTRING::from(&station_name), 0, 0x37f, Some(&secured))
                        .map_err(text)?;
                let previous = GetProcessWindowStation().map_err(text)?;
                SetProcessWindowStation(worker.station).map_err(text)?;
                let desktop = CreateDesktopW(
                    &HSTRING::from("default"),
                    PCWSTR::null(),
                    None,
                    DESKTOP_CONTROL_FLAGS(0),
                    0x1ff,
                    Some(&secured),
                );
                SetProcessWindowStation(previous).map_err(text)?;
                worker.desktop = desktop.map_err(text)?;
            }
        }
        let inheritable = SECURITY_ATTRIBUTES {
            nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: ptr::null_mut(),
            bInheritHandle: true.into(),
        };
        let pipe = |child_reads: bool| -> Result<(Owned, Owned), String> {
            let (mut read, mut write) = (HANDLE::default(), HANDLE::default());
            // SAFETY: both ends are owned by the returned guards; the daemon's end is made non-inheritable.
            unsafe {
                CreatePipe(&mut read, &mut write, Some(&inheritable), 0).map_err(text)?;
                let (child, parent) = if child_reads {
                    (Owned(read), Owned(write))
                } else {
                    (Owned(write), Owned(read))
                };
                SetHandleInformation(parent.0, HANDLE_FLAG_INHERIT.0, Default::default())
                    .map_err(text)?;
                Ok((child, parent))
            }
        };
        let (child_in, parent_in) = pipe(true)?;
        let (child_out, parent_out) = pipe(false)?;
        let (child_err, parent_err) = pipe(false)?;
        let mut desktop_name = wide(&format!("{station_name}\\default"));
        let startup = STARTUPINFOW {
            cb: mem::size_of::<STARTUPINFOW>() as u32,
            lpDesktop: PWSTR(desktop_name.as_mut_ptr()),
            dwFlags: STARTF_USESTDHANDLES | STARTF_USESHOWWINDOW,
            wShowWindow: 0,
            hStdInput: child_in.0,
            hStdOutput: child_out.0,
            hStdError: child_err.0,
            ..Default::default()
        };
        let mut line = wide(command_line);
        let mut secret = wide(password);
        let domain = std::env::var("COMPUTERNAME").map_err(|_| FAILURE.to_string())?;
        let mut info = PROCESS_INFORMATION::default();
        // SAFETY: every buffer outlives the call; the password copy is wiped right after it.
        let started = unsafe {
            CreateProcessWithLogonW(
                &HSTRING::from(user),
                &HSTRING::from(domain),
                PCWSTR(secret.as_ptr()),
                LOGON_WITH_PROFILE,
                &HSTRING::from(file),
                Some(PWSTR(line.as_mut_ptr())),
                CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT | CREATE_SUSPENDED,
                Some(environment.as_ptr().cast()),
                &HSTRING::from(cwd),
                &startup,
                &mut info,
            )
        };
        secret.fill(0);
        started.map_err(text)?;
        worker.process = Owned(info.hProcess);
        *worker
            .thread
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Owned(info.hThread));
        // SAFETY: each parent end is owned exactly once by the File it moves into.
        unsafe {
            worker.stdin = Some(File::from_raw_handle(mem::ManuallyDrop::new(parent_in).0.0));
            worker.stdout = Some(File::from_raw_handle(
                mem::ManuallyDrop::new(parent_out).0.0,
            ));
            worker.stderr = Some(File::from_raw_handle(
                mem::ManuallyDrop::new(parent_err).0.0,
            ));
        }
        Ok(worker)
    }

    pub fn process(&self) -> HANDLE {
        self.process.0
    }

    /// Lets the suspended main thread run.
    pub fn resume(&self) -> Result<(), String> {
        let thread = self
            .thread
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
            .ok_or(FAILURE)?;
        // SAFETY: the thread handle is valid until the guard drops.
        if unsafe { ResumeThread(thread.0) } == u32::MAX {
            Err(FAILURE.into())
        } else {
            Ok(())
        }
    }

    /// Waits for the process to exit and returns its exit code.
    pub fn wait(&self) -> Result<u32, String> {
        let mut code = 0;
        // SAFETY: the process handle is valid while `self` lives.
        unsafe {
            if WaitForSingleObject(self.process.0, INFINITE) != WAIT_OBJECT_0 {
                return Err(FAILURE.into());
            }
            GetExitCodeProcess(self.process.0, &mut code).map_err(text)?;
        }
        Ok(code)
    }

    pub fn kill(&self) -> Result<(), String> {
        // SAFETY: the process handle is valid while `self` lives.
        unsafe { TerminateProcess(self.process.0, 71) }.map_err(text)
    }

    pub fn stdin(&mut self) -> Option<File> {
        self.stdin.take()
    }

    pub fn stdout(&mut self) -> Option<File> {
        self.stdout.take()
    }

    pub fn stderr(&mut self) -> Option<File> {
        self.stderr.take()
    }
}

impl Drop for NativeWorker {
    fn drop(&mut self) {
        self.stdin = None;
        self.stdout = None;
        self.stderr = None;
        // SAFETY: closed once each, after the process handle is no longer needed by anyone else.
        unsafe {
            if !self.desktop.is_invalid() && !self.desktop.0.is_null() {
                let _ = CloseDesktop(self.desktop);
            }
            if !self.station.is_invalid() && !self.station.0.is_null() {
                let _ = CloseWindowStation(self.station);
            }
        }
    }
}

/// A Windows environment block: `key=value` entries, each ending in NUL, then a final NUL.
pub fn environment_block(values: &[(String, String)]) -> Vec<u16> {
    let mut block: Vec<u16> = values
        .iter()
        .flat_map(|(key, value)| {
            format!("{key}={value}")
                .encode_utf16()
                .chain([0])
                .collect::<Vec<_>>()
        })
        .collect();
    block.push(0);
    if values.is_empty() {
        block.push(0);
    }
    block
}

fn random_bytes(count: usize) -> Result<Vec<u8>, String> {
    let mut bytes = vec![0u8; count];
    // SAFETY: fills the local buffer.
    unsafe { BCryptGenRandom(None, &mut bytes, BCRYPT_USE_SYSTEM_PREFERRED_RNG) }
        .ok()
        .map_err(text)?;
    Ok(bytes)
}

fn random_hex(count: usize) -> Result<String, String> {
    Ok(random_bytes(count)?
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// A random lowercase UUID, for job names (`[a-f0-9-]{36}`).
fn random_uuid() -> Result<String, String> {
    let hex = random_hex(16)?;
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}

/// A worker password like `provision-worker.ps1` makes: `Aa1!` and 48 random bytes in base64.
pub fn new_password() -> Result<String, String> {
    let mut bytes = random_bytes(48)?;
    let password = format!(
        "Aa1!{}",
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    );
    bytes.fill(0);
    Ok(password)
}

/// The SID of local account `name`, or `None` if there is no such account.
pub fn account_sid(name: &str) -> Result<Option<String>, String> {
    let computer = std::env::var("COMPUTERNAME").map_err(|_| FAILURE.to_string())?;
    let mut sid = [0u8; 68];
    let mut domain = [0u16; 256];
    let (mut sid_size, mut domain_size, mut kind) = (
        sid.len() as u32,
        domain.len() as u32,
        SID_NAME_USE::default(),
    );
    // SAFETY: both buffers are local and sized.
    let found = unsafe {
        LookupAccountNameW(
            PCWSTR::null(),
            &HSTRING::from(format!("{computer}\\{name}")),
            Some(PSID(sid.as_mut_ptr().cast())),
            &mut sid_size,
            Some(PWSTR(domain.as_mut_ptr())),
            &mut domain_size,
            &mut kind,
        )
    };
    match found {
        Err(error) if error.code() == ERROR_NONE_MAPPED.to_hresult() => Ok(None),
        Err(error) => Err(error.to_string()),
        Ok(()) => sid_string(PSID(sid.as_mut_ptr().cast())).map(Some),
    }
}

fn account_enabled(name: &str) -> Result<bool, String> {
    let mut buffer = ptr::null_mut();
    // SAFETY: the buffer is freed once with NetApiBufferFree.
    unsafe {
        let status = NetUserGetInfo(PCWSTR::null(), &HSTRING::from(name), 1, &mut buffer);
        if status != 0 {
            return Err(format!("NetUserGetInfo {status}"));
        }
        let enabled = (*(buffer as *const USER_INFO_1)).usri1_flags.0 & UF_ACCOUNTDISABLE.0 == 0;
        NetApiBufferFree(Some(buffer.cast()));
        Ok(enabled)
    }
}

/// The local name of a well-known group, which differs between Windows languages.
fn group_name(sid: &str) -> Result<String, String> {
    let sid = Sid::parse(sid)?;
    let (mut name, mut domain) = ([0u16; 256], [0u16; 256]);
    let (mut name_size, mut domain_size, mut kind) = (
        name.len() as u32,
        domain.len() as u32,
        SID_NAME_USE::default(),
    );
    // SAFETY: both buffers are local and sized.
    unsafe {
        LookupAccountSidW(
            PCWSTR::null(),
            sid.0,
            Some(PWSTR(name.as_mut_ptr())),
            &mut name_size,
            Some(PWSTR(domain.as_mut_ptr())),
            &mut domain_size,
            &mut kind,
        )
    }
    .map_err(text)?;
    Ok(String::from_utf16_lossy(&name[..name_size as usize]))
}

/// Creates a standard local account in the Users group (`New-LocalUser`, `Add-LocalGroupMember`) and returns its
/// SID. Needs an elevated owner.
pub fn create_account(name: &str, password: &str, comment: &str) -> Result<String, String> {
    let (mut user, mut secret, mut note) = (wide(name), wide(password), wide(comment));
    let info = USER_INFO_1 {
        usri1_name: PWSTR(user.as_mut_ptr()),
        usri1_password: PWSTR(secret.as_mut_ptr()),
        usri1_priv: USER_PRIV_USER,
        usri1_comment: PWSTR(note.as_mut_ptr()),
        usri1_flags: UF_SCRIPT | UF_DONT_EXPIRE_PASSWD | UF_PASSWD_CANT_CHANGE,
        ..Default::default()
    };
    // SAFETY: every buffer outlives the call; the password copy is wiped right after it.
    let status = unsafe { NetUserAdd(PCWSTR::null(), 1, ptr::addr_of!(info).cast(), None) };
    secret.fill(0);
    if status != 0 {
        return Err(format!("NetUserAdd {status}"));
    }
    let sid = account_sid(name)?.ok_or(FAILURE)?;
    let member = Sid::parse(&sid)?;
    let entry = LOCALGROUP_MEMBERS_INFO_0 {
        lgrmi0_sid: member.0,
    };
    // SAFETY: the entry's SID outlives the call.
    let status = unsafe {
        NetLocalGroupAddMembers(
            PCWSTR::null(),
            &HSTRING::from(group_name(USERS)?),
            0,
            ptr::addr_of!(entry).cast(),
            1,
        )
    };
    const ALREADY_MEMBER: u32 = 1378;
    if status != 0 && status != ALREADY_MEMBER {
        return Err(format!("NetLocalGroupAddMembers {status}"));
    }
    Ok(sid)
}

/// Removes local account `name`. A missing account is not an error. Needs an elevated owner.
pub fn delete_account(name: &str) -> Result<(), String> {
    // SAFETY: plain call.
    match unsafe { NetUserDel(PCWSTR::null(), &HSTRING::from(name)) } {
        0 => Ok(()),
        status if status == NERR_UserNotFound => Ok(()),
        status => Err(format!("NetUserDel {status}")),
    }
}

/// `TjWorkerDirectoryGrant.Grant` (`windows-worker.ts:104-140`): adds an inherited Modify allow for the worker to the
/// exact folder `path` names (a link to elsewhere is refused), never to the peer's profile.
pub fn grant_directory(path: &Path, sid: &str, peer_sid: &str) -> Result<(), String> {
    // SAFETY: the file handle is owned by the guard; buffers are local.
    let file = Owned(
        unsafe {
            CreateFileW(
                &HSTRING::from(path),
                0x0006_0000,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )
        }
        .map_err(text)?,
    );
    let canonical = final_path(file.0)?;
    let expected = plain(&std::path::absolute(path).map_err(text)?);
    if !canonical.eq_ignore_ascii_case(&expected) {
        return Err("Directory target changed.".into());
    }
    if let Some(profile) = profile_path(peer_sid) {
        let profile = fs::canonicalize(&profile)
            .map(|p| plain(&p))
            .unwrap_or_else(|_| plain(&profile));
        let (a, b) = (canonical.to_lowercase(), profile.to_lowercase());
        if a == b || a.starts_with(&format!("{b}\\")) || b.starts_with(&format!("{a}\\")) {
            return Err("Protected peer profile.".into());
        }
    }
    let worker = Sid::parse(sid)?;
    let access = EXPLICIT_ACCESS_W {
        grfAccessPermissions: MODIFY,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        Trustee: TRUSTEE_W {
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: PWSTR(worker.0.0.cast()),
            ..Default::default()
        },
    };
    let (mut dacl, mut descriptor, mut updated) = (
        ptr::null_mut(),
        PSECURITY_DESCRIPTOR::default(),
        ptr::null_mut(),
    );
    // SAFETY: the old DACL lives in `descriptor`, the new one in `updated`; both are freed once.
    unsafe {
        GetSecurityInfo(
            file.0,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(&mut dacl),
            None,
            Some(&mut descriptor),
        )
        .ok()
        .map_err(text)?;
        let merged = SetEntriesInAclW(Some(&[access]), Some(dacl), &mut updated)
            .ok()
            .map_err(text);
        let written = merged.and_then(|()| {
            SetSecurityInfo(
                file.0,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(updated),
                None,
            )
            .ok()
            .map_err(text)
        });
        LocalFree(Some(HLOCAL(updated.cast())));
        LocalFree(Some(HLOCAL(descriptor.0)));
        written
    }
}

/// A path without its `\\?\` prefix or trailing separator, as .NET's `Path.GetFullPath(...).TrimEnd('\\')` gives it.
fn plain(path: &Path) -> String {
    let text = path.to_string_lossy();
    let text = if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else {
        text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned()
    };
    text.trim_end_matches('\\').to_owned()
}

fn final_path(handle: HANDLE) -> Result<String, String> {
    let mut buffer = vec![0u16; 32768];
    // SAFETY: the buffer is local and sized.
    let count =
        unsafe { GetFinalPathNameByHandleW(handle, &mut buffer, FILE_NAME_NORMALIZED) } as usize;
    if count == 0 || count >= buffer.len() {
        return Err(FAILURE.into());
    }
    Ok(plain(Path::new(&String::from_utf16_lossy(
        &buffer[..count],
    ))))
}

/// The peer account's profile folder from the registry, if it has one.
fn profile_path(sid: &str) -> Option<PathBuf> {
    let key = HSTRING::from(format!(
        r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\{sid}"
    ));
    let mut buffer = vec![0u16; 1024];
    let mut size = (buffer.len() * 2) as u32;
    // SAFETY: the buffer is local and sized in bytes; REG_EXPAND_SZ values come back expanded.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            &key,
            &HSTRING::from("ProfileImagePath"),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status.is_err() {
        return None;
    }
    let value = String::from_utf16_lossy(&buffer[..(size as usize / 2).saturating_sub(1)]);
    (!value.is_empty())
        .then(|| std::path::absolute(&value).unwrap_or_else(|_| PathBuf::from(value)))
}

/// A process's command line, or `None` when Windows won't say (another account's process).
fn process_command_line(pid: u32) -> Option<String> {
    // SAFETY: the handle is owned by the guard; the buffer is local and holds a UNICODE_STRING into itself.
    unsafe {
        let process = Owned(OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?);
        let mut needed = 0u32;
        let _ = NtQueryInformationProcess(
            process.0,
            ProcessCommandLineInformation,
            ptr::null_mut(),
            0,
            &mut needed,
        );
        if needed == 0 || needed > 1 << 20 {
            return None;
        }
        let mut buffer = vec![0u64; (needed as usize).div_ceil(8)];
        if NtQueryInformationProcess(
            process.0,
            ProcessCommandLineInformation,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
        .is_err()
        {
            return None;
        }
        let line = &*(buffer.as_ptr() as *const UNICODE_STRING);
        Some(String::from_utf16_lossy(std::slice::from_raw_parts(
            line.Buffer.0,
            usize::from(line.Length) / 2,
        )))
    }
}

fn process_owner(pid: u32) -> Option<String> {
    // SAFETY: the handle is owned by the guard.
    let process =
        Owned(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?);
    token_user(process_token(process.0).ok()?.0).ok()
}

/// The process scan of `windows-worker.ps1:287-297`: no Claude, Codex or provider Node may run outside the known
/// jobs as the owner or a worker, and a Node process that can't be identified stops the check.
fn provider_scan(config: &WorkerConfiguration, jobs: &[&Job]) -> Result<(), String> {
    // SAFETY: the snapshot is owned by the guard; the entry is a local, sized struct.
    let snapshot = Owned(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.map_err(text)?);
    let mut entry = PROCESSENTRY32W {
        dwSize: mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut more = unsafe { Process32FirstW(snapshot.0, &mut entry) }.is_ok();
    while more {
        let length = entry
            .szExeFile
            .iter()
            .position(|c| *c == 0)
            .unwrap_or(entry.szExeFile.len());
        let name = String::from_utf16_lossy(&entry.szExeFile[..length]);
        let pid = entry.th32ProcessID;
        if ["claude.exe", "codex.exe", "node.exe"]
            .iter()
            .any(|known| name.eq_ignore_ascii_case(known))
        {
            let mut owned = false;
            for job in jobs {
                owned |= job.contains(pid).unwrap_or(false);
            }
            if unreceipted_provider(&name, process_command_line(pid).as_deref(), owned)? {
                match process_owner(pid) {
                    None => return Err("An owner-level provider is still running. Close older Tjommie/provider apps before enabling Logins.".into()),
                    Some(owner) if owner == config.owner_sid => {
                        return Err("An owner-level provider is still running. Close older Tjommie/provider apps before enabling Logins.".into());
                    }
                    Some(owner) if owner == config.worker_sid || owner == config.peer_sid => {
                        return Err("An untracked worker provider is running. Reconcile older hosts before enabling Logins.".into());
                    }
                    Some(_) => {}
                }
            }
        }
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot.0, &mut entry) }.is_ok();
    }
    Ok(())
}

/// `windows-worker.ps1:298-309`: only the owner, SYSTEM and Administrators may hold more than the worker's own
/// write access on the protected roots.
fn protected_roots(config: &WorkerConfiguration) -> Result<(), String> {
    let trusted = |sid: &str| sid == config.owner_sid || sid == SYSTEM || sid == ADMINISTRATORS;
    for root in [
        &config.owner_data_root,
        &config.runtime_root,
        &config.worker_root,
        &config.peer_root,
    ] {
        for ace in Descriptor::of_path(root)?
            .aces()?
            .ok_or("protected ACL")?
            .iter()
            .filter(|ace| ace.kind == Ace::ALLOWED)
        {
            if trusted(&ace.sid) {
                continue;
            }
            let own_work = (root == &config.worker_root && ace.sid == config.worker_sid)
                || (root == &config.peer_root && ace.sid == config.peer_sid);
            if own_work && ace.mask & (CHANGE_PERMISSIONS | TAKE_OWNERSHIP) == 0 {
                continue;
            }
            let reads_runtime = root == &config.runtime_root
                && (ace.sid == config.worker_sid || ace.sid == config.peer_sid);
            if reads_runtime && ace.mask & (WRITE | CHANGE_PERMISSIONS | TAKE_OWNERSHIP) == 0 {
                continue;
            }
            return Err("protected ACL".into());
        }
    }
    Ok(())
}

/// The helper's verify mode (`windows-worker.ps1:91-314`): the owner runs this, the worker account is the one
/// configured and enabled, Windows restarted since provisioning, no untracked provider runs, the protected roots
/// and the runtime tree are locked down, and for review the cross-host review lease can be taken.
pub fn verify(config: &WorkerConfiguration, jobs: &[&Job]) -> Result<(), String> {
    if current_user_sid()? != config.owner_sid || config.owner_sid == config.worker_sid {
        return Err("boundary".into());
    }
    if account_sid(&config.worker_name)?.as_deref() != Some(config.worker_sid.as_str())
        || !account_enabled(&config.worker_name)?
    {
        return Err("boundary".into());
    }
    let provisioned: jiff::Timestamp = config.provisioned_at.parse().map_err(text)?;
    // SAFETY: plain call.
    let uptime = Duration::from_millis(unsafe { GetTickCount64() });
    let boot = jiff::Timestamp::now() - jiff::SignedDuration::try_from(uptime).map_err(text)?;
    if boot <= provisioned {
        return Err("Restart Windows after provisioning before accepting this boundary.".into());
    }
    provider_scan(config, jobs)?;
    protected_roots(config)?;
    assert_runtime_tree(&config.runtime_root, &config.owner_sid)?;
    if config.purpose == Purpose::Review {
        drop(ReviewLease::open(config)?);
    }
    Ok(())
}

/// The review identity's cross-host lease (`Open-TrustedReviewLease`, `windows-worker.ps1:61-74`): a `Global\`
/// mutex only the owner, SYSTEM and Administrators can open, and a marker file while a review tree may be alive.
pub struct ReviewLease {
    mutex: Owned,
    marker: PathBuf,
    marked: bool,
}

impl ReviewLease {
    pub fn open(config: &WorkerConfiguration) -> Result<Self, String> {
        use sha2::{Digest, Sha256};
        let key = format!("{:X}", Sha256::digest(config.worker_sid.as_bytes()));
        let owner = &config.owner_sid;
        let descriptor = Descriptor::from_sddl(&format!(
            "O:{owner}D:P(A;;GA;;;{owner})(A;;GA;;;SY)(A;;GA;;;BA)"
        ))?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor.0.0,
            bInheritHandle: false.into(),
        };
        // SAFETY: the attributes outlive the call; the handle is owned by the guard.
        let mutex = Owned(
            unsafe {
                CreateMutexW(
                    Some(&attributes),
                    false,
                    &HSTRING::from(format!(r"Global\TjommieReview-{key}")),
                )
            }
            .map_err(text)?,
        );
        assert_mutex_descriptor(&Descriptor::of_object(mutex.0)?, owner)?;
        let marker = config
            .owner_data_root
            .join(format!("review-worker-active-{key}.json"));
        // SAFETY: the mutex handle is valid.
        let wait = match unsafe { WaitForSingleObject(mutex.0, 0) } {
            WAIT_OBJECT_0 => MutexWait::Acquired,
            WAIT_ABANDONED => MutexWait::Abandoned,
            WAIT_TIMEOUT => MutexWait::Busy,
            _ => return Err(FAILURE.into()),
        };
        // SAFETY: released only when this thread holds it.
        enter_review_mutex(wait, &marker, &config.worker_sid, || unsafe {
            let _ = ReleaseMutex(mutex.0);
        })?;
        Ok(Self {
            mutex,
            marker,
            marked: false,
        })
    }

    /// Writes the marker before a review tree starts.
    pub fn mark(&mut self, worker_sid: &str) -> Result<(), String> {
        write_review_marker(&self.marker, worker_sid)?;
        self.marked = true;
        Ok(())
    }

    /// Removes the marker once the tree is confirmed stopped; otherwise it stays and fences later reviews.
    pub fn finish(mut self, stopped: bool) -> bool {
        if self.marked && stopped {
            self.marked = fs::remove_file(&self.marker).is_err();
        }
        stopped && !self.marked
    }
}

impl Drop for ReviewLease {
    fn drop(&mut self) {
        // SAFETY: this thread acquired the mutex in `open`.
        let _ = unsafe { ReleaseMutex(self.mutex.0) };
    }
}

/// A launched worker: its process, the job holding its tree, and for review the lease.
pub struct WorkerProcess {
    pub worker: NativeWorker,
    pub job: Job,
    lease: Option<ReviewLease>,
}

impl WorkerProcess {
    /// Stops the tree and settles the review lease. `false` means the tree could not be confirmed stopped, and the
    /// caller must treat the identity as unsafe (`WorkerLaunchTerminationError`).
    pub fn retire(self) -> bool {
        let stopped = self.job.stop();
        let WorkerProcess { worker, job, lease } = self;
        drop(worker);
        drop(job);
        lease.map_or(stopped, |lease| lease.finish(stopped))
    }
}

/// The helper's launch mode (`windows-worker.ps1:315-346`) for a verified identity: runs the fixed bootstrap in
/// `runtime\node.exe` under the worker account, suspended, checks its token, binds it to a new
/// `Global\TjommieWorkerJob-<purpose>-<uuid>` job and returns it. The caller writes `launch_frame` to stdin, then
/// resumes it.
pub fn launch(
    config: &WorkerConfiguration,
    password: &str,
    cwd: &Path,
    environment: &[(String, String)],
) -> Result<WorkerProcess, String> {
    let bootstrap = config.runtime_root.join("node.exe");
    let file = bootstrap.to_str().ok_or(FAILURE)?;
    let line = command_line(file, &["-e".into(), BOOTSTRAP.into()])?;
    if line.encode_utf16().count() + 1 > 1024 {
        return Err("Bootstrap command limit.".into());
    }
    let block = environment_block(&worker_environment(config, environment)?);
    let mut lease = if config.purpose == Purpose::Review {
        Some(ReviewLease::open(config)?)
    } else {
        None
    };
    if let Some(lease) = lease.as_mut() {
        lease.mark(&config.worker_sid)?;
    }
    let worker = match NativeWorker::start(
        &config.worker_name,
        password,
        file,
        &line,
        cwd,
        &block,
        &config.owner_sid,
        &config.worker_sid,
    ) {
        Ok(worker) => worker,
        Err(error) => {
            // Nothing started, so the marker can go.
            if let Some(lease) = lease {
                lease.finish(true);
            }
            return Err(error);
        }
    };
    let bound = verify_token(worker.process(), &config.worker_sid).and_then(|()| {
        let name = format!(
            r"Global\TjommieWorkerJob-{}-{}",
            config.purpose.as_str(),
            random_uuid()?
        );
        Job::bind(worker.process(), Some(&name), &config.owner_sid)
    });
    match bound {
        Ok(job) => Ok(WorkerProcess { worker, job, lease }),
        Err(error) => {
            // Still suspended and in no job: kill it and wait, so the marker is only removed for a dead process.
            let stopped = worker.kill().is_ok() && worker.wait().is_ok();
            drop(worker);
            if let Some(lease) = lease {
                lease.finish(stopped);
            }
            Err(error)
        }
    }
}

/// `provision-worker.ps1`, run by the owner from an elevated window: creates the provider and review accounts for
/// `owner_data_root`, saves their passwords in Credential Manager, locks down their folders and writes both
/// configurations. Undoes the accounts and passwords if anything fails.
pub fn provision(owner_data_root: &Path) -> Result<String, String> {
    // SAFETY: the pseudo handle needs no closing.
    let token = process_token(unsafe { GetCurrentProcess() })?;
    if !elevated(token.0)? {
        return Err("Run this setup from an elevated window as the Tjommie owner.".into());
    }
    let owner_sid = token_user(token.0)?;
    let owner_root = PathBuf::from(plain(&std::path::absolute(owner_data_root).map_err(text)?));
    let lower = |path: &Path| path.to_string_lossy().to_lowercase();
    let system = std::env::var("SystemRoot")
        .unwrap_or_default()
        .to_lowercase();
    let profile = std::env::var("USERPROFILE")
        .unwrap_or_default()
        .to_lowercase();
    if owner_root.parent().is_none()
        || lower(&owner_root) == profile
        || (!system.is_empty() && lower(&owner_root).starts_with(&system))
    {
        return Err("Choose the dedicated Tjommie data folder.".into());
    }
    if ["worker.json", "review-worker.json"]
        .iter()
        .any(|name| owner_root.join(name).exists())
    {
        return Err("This host already has worker configuration. Do not overwrite it.".into());
    }
    let namespace = Namespace::new(&owner_root).map_err(text)?;
    let base = PathBuf::from(std::env::var("ProgramData").map_err(text)?)
        .join(format!(r"Tjommie\Worker-{}", namespace.short()));
    let runtime = base.join("runtime");
    let workers = [
        (
            Purpose::Provider,
            worker_name(&namespace, Purpose::Provider),
            base.join("work"),
            namespace.isolated_worker_target(),
        ),
        (
            Purpose::Review,
            worker_name(&namespace, Purpose::Review),
            base.join("review"),
            namespace.isolated_review_target(),
        ),
    ];
    for (_, name, _, _) in &workers {
        if account_sid(name)?.is_some() {
            return Err(
                "A generated worker account already exists. Reconcile it before setup.".into(),
            );
        }
    }
    let folders = [&owner_root, &base, &runtime, &workers[0].2, &workers[1].2];
    for folder in folders {
        fs::create_dir_all(folder).map_err(text)?;
        if fs::symlink_metadata(folder).map_err(text)?.is_symlink() {
            return Err("Setup folders must not be links.".into());
        }
    }
    let mut created = vec![];
    let result = (|| -> Result<String, String> {
        let mut sids = vec![];
        for (purpose, name, _, target) in &workers {
            let mut password = new_password()?;
            let made = create_account(
                name,
                &password,
                &format!("Tjommie isolated {} worker", purpose.as_str()),
            );
            if made.is_ok() {
                created.push(name.clone());
            }
            let sid = made.and_then(|sid| {
                let mut secret =
                    serde_json::json!({ "username": name, "password": password }).to_string();
                let saved = cred::write_as(target, &secret, WORKER_CREDENTIAL_USER).map_err(text);
                // SAFETY: overwriting with zeros keeps the string valid UTF-8.
                unsafe { secret.as_bytes_mut().fill(0) };
                saved.map(|()| sid)
            });
            // SAFETY: as above.
            unsafe { password.as_bytes_mut().fill(0) };
            sids.push(sid?);
        }
        let full =
            format!("O:{owner_sid}D:P(A;OICI;FA;;;{owner_sid})(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)");
        Descriptor::from_sddl(&full)?.apply_protected(&owner_root)?;
        Descriptor::from_sddl(&full)?.apply_protected(&base)?;
        Descriptor::from_sddl(&format!(
            "{full}(A;OICI;{READ_AND_EXECUTE:#x};;;{})(A;OICI;{READ_AND_EXECUTE:#x};;;{})",
            sids[0], sids[1]
        ))?
        .apply_protected(&runtime)?;
        for (index, (_, _, root, _)) in workers.iter().enumerate() {
            Descriptor::from_sddl(&format!("{full}(A;OICI;{MODIFY:#x};;;{})", sids[index]))?
                .apply_protected(root)?;
        }
        let provisioned_at = jiff::Timestamp::now().to_string();
        fs::write(owner_root.join("vault-protection-required"), "Separate provider and review identities remain required until all Logins secrets are removed.").map_err(text)?;
        for (index, (purpose, name, root, target)) in workers.iter().enumerate() {
            let peer = &workers[1 - index];
            for child in ["tmp", "accounts", r"AppData\Roaming", r"AppData\Local"] {
                fs::create_dir_all(root.join(child)).map_err(text)?;
            }
            let config = WorkerConfiguration {
                version: 2,
                purpose: *purpose,
                owner_sid: owner_sid.clone(),
                worker_sid: sids[index].clone(),
                peer_sid: sids[1 - index].clone(),
                worker_name: name.clone(),
                worker_root: root.clone(),
                peer_root: peer.2.clone(),
                runtime_root: runtime.clone(),
                owner_data_root: owner_root.clone(),
                provisioned_at: provisioned_at.clone(),
                credential_target: target.clone(),
            };
            fs::write(
                owner_root.join(purpose.configuration_file()),
                serde_json::to_string(&config).map_err(text)?,
            )
            .map_err(text)?;
        }
        Ok(format!(
            "Created separate standard provider and review identities. Install runtimes in {}, restart Windows and run acceptance. The desktop remains unelevated. No owner authentication was copied.",
            runtime.display()
        ))
    })();
    result.map_err(|_| {
        for name in &created {
            let _ = delete_account(name);
        }
        for (_, _, _, target) in &workers {
            let _ = cred::delete(target);
        }
        "Worker setup failed. No Logins feature was enabled. Review the generated folders and protection marker before retrying.".to_string()
    })
}
