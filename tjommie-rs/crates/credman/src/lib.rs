//! One lock around every Windows Credential Manager call Tjommie makes. While several threads or processes use
//! Credential Manager at once, Windows can drop a credential that was just written and keep one that was just deleted
//! (open-source-cooperative/windows-native-keyring-store issue 20; this app's own tests showed both). So every `Cred*`
//! call of the daemon and the desktop runs inside [`locked`], and every delete is checked with [`delete_gone`].
//! It orders Tjommie's own calls only: another program may use Credential Manager at any time.

use std::io;
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::Duration;

use windows::Win32::Foundation::{
    CloseHandle, HANDLE, HLOCAL, LocalFree, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::System::Threading::{
    CreateMutexW, GetCurrentProcess, OpenProcessToken, ReleaseMutex, WaitForSingleObject,
};
use windows::core::{HSTRING, PWSTR};

/// How long a caller waits for another process's call before it goes on without it. A call takes milliseconds.
const WAIT: Duration = Duration::from_secs(10);

/// One mutex per Windows user, because the credential set is per user and spans that user's sessions (hence `Global\`).
static SHARED: LazyLock<Lock> = LazyLock::new(|| match user_sid() {
    Ok(sid) => Lock::new(Some(&format!("Global\\Tjommie-credman-{sid}")), WAIT),
    Err(error) => {
        warn(&format!(
            "This Windows user's id could not be read ({error})."
        ));
        Lock::new(None, WAIT)
    }
});

/// Runs `call` while no other Tjommie thread or process of this Windows user is inside a Credential Manager call. A
/// lock that cannot be had never refuses the call: this process's own lock then covers it, and the log says so.
/// Not reentrant: `call` must not call `locked` again.
pub fn locked<T>(call: impl FnOnce() -> T) -> T {
    SHARED.with(call)
}

/// Deletes, then looks: Windows can keep a credential whose delete it reported as done. Deletes once more, and is an
/// error when the credential is still there, so that the caller keeps its note of it. Call it inside [`locked`].
pub fn delete_gone(
    mut delete: impl FnMut() -> io::Result<()>,
    mut still_there: impl FnMut() -> io::Result<bool>,
) -> io::Result<()> {
    for _ in 0..2 {
        delete()?;
        if !still_there()? {
            return Ok(());
        }
    }
    Err(io::Error::other("Windows kept a deleted credential."))
}

struct Lock {
    process: Mutex<()>,
    /// `None` when the named mutex could not be created or opened.
    user: Option<Handle>,
    wait: Duration,
}

impl Lock {
    fn new(name: Option<&str>, wait: Duration) -> Self {
        // SAFETY: creates or opens a named mutex without taking it; `Handle` closes it.
        let user = name.and_then(|name| {
            match unsafe { CreateMutexW(None, false, &HSTRING::from(name)) } {
                Ok(handle) => Some(Handle(handle)),
                Err(error) => {
                    warn(&format!(
                        "The Credential Manager lock could not be opened ({error})."
                    ));
                    None
                }
            }
        });
        Self {
            process: Mutex::new(()),
            user,
            wait,
        }
    }

    fn with<T>(&self, call: impl FnOnce() -> T) -> T {
        let _process = self.process.lock().unwrap_or_else(PoisonError::into_inner);
        let _user = self.user.as_ref().and_then(|mutex| mutex.hold(self.wait));
        call()
    }
}

fn warn(problem: &str) {
    let line = format!("{problem} Credential Manager calls are ordered inside this process only.");
    log::warn!("{line}");
    eprintln!("{line}");
}

/// A kernel handle, closed on drop.
struct Handle(HANDLE);

// SAFETY: a kernel handle is not tied to the thread that opened it, and the calls made with it are thread-safe.
unsafe impl Send for Handle {}
// SAFETY: as above.
unsafe impl Sync for Handle {}

impl Handle {
    /// Takes the mutex. One whose owner ended without releasing it ("abandoned") is ours too.
    fn hold(&self, wait: Duration) -> Option<Held<'_>> {
        let millis = u32::try_from(wait.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: the handle is open for as long as `self`.
        match unsafe { WaitForSingleObject(self.0, millis) } {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Some(Held(self)),
            WAIT_TIMEOUT => {
                warn("The Credential Manager lock was not free in time.");
                None
            }
            _ => {
                let error = io::Error::last_os_error();
                warn(&format!(
                    "Waiting for the Credential Manager lock failed ({error})."
                ));
                None
            }
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful call and is closed only here.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// The mutex, held by this thread; released on drop, also when the call panics.
struct Held<'a>(&'a Handle);

impl Drop for Held<'_> {
    fn drop(&mut self) {
        // SAFETY: this thread took the mutex in `hold`, and releases it once.
        unsafe {
            let _ = ReleaseMutex(self.0.0);
        }
    }
}

/// The SID of the user this process runs as, as text.
fn user_sid() -> io::Result<String> {
    let mut token = HANDLE::default();
    // SAFETY: the process pseudo handle needs no closing; the token is closed by `Handle`.
    unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)? };
    let token = Handle(token);
    let mut needed = 0;
    // SAFETY: with no buffer the call only says how large one must be (and fails, by design).
    let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut needed) };
    // u64s, so that the TOKEN_USER at the start is aligned.
    let mut buffer = vec![0u64; (needed as usize).div_ceil(8).max(1)];
    // SAFETY: the buffer is at least `needed` bytes.
    unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            needed,
            &mut needed,
        )?;
    }
    let mut text = PWSTR::null();
    // SAFETY: the buffer holds a TOKEN_USER whose SID points into it; the string is freed once with LocalFree.
    unsafe {
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        ConvertSidToStringSidW(user.User.Sid, &mut text)?;
        let sid = text.to_string().map_err(io::Error::other);
        LocalFree(Some(HLOCAL(text.0.cast())));
        sid
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Instant;
    use windows::Win32::System::Threading::CreateEventW;

    /// A name no other test run uses.
    fn name(label: &str) -> String {
        format!("Local\\Tjommie-credman-test-{}-{label}", std::process::id())
    }

    fn mutex(name: &str) -> Handle {
        // SAFETY: as in `Lock::new`.
        Handle(unsafe { CreateMutexW(None, false, &HSTRING::from(name)) }.expect("a mutex"))
    }

    /// Whether another thread, which stands for another process, can take the mutex right now.
    fn free_for_another(name: &str) -> bool {
        std::thread::scope(|scope| {
            scope
                .spawn(|| mutex(name).hold(Duration::ZERO).is_some())
                .join()
                .expect("the other thread")
        })
    }

    #[test]
    fn the_shared_lock_opens_for_this_user() {
        assert!(user_sid().expect("a SID").starts_with("S-1-"));
        assert_eq!(locked(|| 7), 7);
        assert!(SHARED.user.is_some(), "the named mutex was opened");
    }

    #[test]
    fn a_call_keeps_every_other_holder_out_until_it_ends() {
        let name = name("held");
        let lock = Lock::new(Some(&name), WAIT);
        lock.with(|| assert!(!free_for_another(&name), "held during the call"));
        assert!(free_for_another(&name), "released after it");
    }

    #[test]
    fn a_name_that_cannot_be_opened_still_runs_the_call() {
        let name = name("squatted");
        // An event under the mutex's name: opening it as a mutex fails, as a name another account holds would.
        // SAFETY: creates a named event, closed by `Handle`.
        let _squatter = Handle(
            unsafe { CreateEventW(None, false, false, &HSTRING::from(name.as_str())) }
                .expect("an event"),
        );
        let lock = Lock::new(Some(&name), WAIT);

        assert!(lock.user.is_none());
        assert_eq!(lock.with(|| 7), 7);
    }

    #[test]
    fn a_wait_that_times_out_still_runs_the_call() {
        let name = name("busy");
        let (taken, is_taken) = mpsc::channel();
        let (release, may_release) = mpsc::channel::<()>();
        std::thread::scope(|scope| {
            // Owned in here, so that a failed assert below drops it and the holder ends instead of waiting for ever.
            let release = release;
            let name = &name;
            scope.spawn(move || {
                let mutex = mutex(name);
                let _held = mutex.hold(WAIT).expect("taken");
                taken.send(()).expect("told");
                let _ = may_release.recv();
            });
            is_taken.recv().expect("the holder has it");
            let lock = Lock::new(Some(name), Duration::from_millis(100));
            let started = Instant::now();

            assert_eq!(lock.with(|| 7), 7);
            assert!(started.elapsed() >= Duration::from_millis(90), "it waited");
            release.send(()).expect("let go");
        });
    }

    #[test]
    fn a_mutex_left_by_an_ended_owner_is_taken_over() {
        let name = name("abandoned");
        let _keeps_it_alive = mutex(&name);
        let owner = name.clone();
        std::thread::spawn(move || {
            let mutex = mutex(&owner);
            std::mem::forget(mutex.hold(WAIT).expect("taken"));
        })
        .join()
        .expect("the owner ended");
        let lock = Lock::new(Some(&name), WAIT);

        lock.with(|| assert!(!free_for_another(&name), "ours now"));
        assert!(free_for_another(&name), "and released properly");
    }

    #[test]
    fn a_delete_is_repeated_once_while_the_credential_stays() {
        let run = |stays_for: u32| {
            let mut deletes = 0;
            let mut looks = 0;
            let result = delete_gone(
                || {
                    deletes += 1;
                    Ok(())
                },
                || {
                    looks += 1;
                    Ok(looks <= stays_for)
                },
            );
            (result.is_ok(), deletes)
        };
        assert_eq!(run(0), (true, 1));
        assert_eq!(run(1), (true, 2));
        assert_eq!(
            run(2),
            (false, 2),
            "still there: an error, so the note of it is kept"
        );
    }
}
