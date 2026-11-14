//! The worker launch path's diagnostics (`win::diag`) and what a job object reports about itself. The launch path
//! answers every failure with one fixed sentence, so these are what a failure is diagnosed from: they record what each
//! step saw, and `Job::describe` / `Job::stop_reported` say which processes a worker's job held and why a stop did or
//! did not finish. The job tests use a plain process tree owned by this user: no account and no elevation.
#![cfg(windows)]

use daemon::spawn;
use daemon::win::diag;
use daemon::win::logon::{self, Job};
use std::os::windows::io::AsRawHandle;
use std::process::Stdio;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::HANDLE;

#[test]
fn notes_and_problems_are_kept_in_order_with_their_times() {
    diag::note("diag-test first");
    diag::problem("diag-test second");
    diag::note("diag-test third");
    let lines = diag::recent(3000);
    let at = |text: &str| lines.iter().rposition(|line| line.contains(text)).unwrap();
    assert!(at("diag-test first") < at("diag-test second"));
    assert!(at("diag-test second") < at("diag-test third"));
    // Every line starts with its time since the first note, and a problem is marked as one.
    assert!(lines[at("diag-test first")].starts_with("[worker-diag +"));
    assert!(lines[at("diag-test second")].contains("PROBLEM diag-test second"));
    assert!(!lines[at("diag-test first")].contains("PROBLEM"));
}

#[test]
fn only_the_most_recent_lines_are_kept() {
    for number in 0..3200 {
        diag::note(format!("diag-flood {number}"));
    }
    let lines = diag::recent(100_000);
    assert!(lines.len() <= 3000, "{} lines kept", lines.len());
    assert!(lines.last().unwrap().ends_with("diag-flood 3199"));
    assert!(
        !lines.iter().any(|line| line.ends_with("diag-flood 0")),
        "the oldest line should have been dropped"
    );
    assert_eq!(diag::recent(2).len(), 2);
}

#[test]
fn an_excerpt_makes_control_characters_visible_and_bounds_its_length() {
    // "a", carriage return, line feed, "b", NUL, "c"
    let bytes = [b'a', 13, 10, b'b', 0, b'c'];
    assert_eq!(diag::excerpt(&bytes, 100), "a..b.c");
    // The first three bytes are "a" and two control characters, then the marker that more was cut off.
    assert_eq!(diag::excerpt(&bytes, 3), "a..... (6 bytes)");
    assert_eq!(diag::excerpt(b"", 10), "");
}

/// `cmd` that starts a grandchild only after a second, so the job is assigned first.
fn process_tree() -> std::process::Child {
    let mut command = spawn::command("cmd");
    command.args(["/c", "ping -n 2 127.0.0.1 >NUL & ping -n 60 127.0.0.1 >NUL"]);
    command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap()
}

fn wait_until(what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_job_names_its_members_and_a_stop_says_what_it_saw() {
    let owner = logon::current_user_sid().unwrap();
    let mut child = process_tree();
    let job = Job::bind(HANDLE(child.as_raw_handle()), None, &owner).unwrap();
    // Not just "two processes": the console host (`conhost.exe`, a child of the console program, and a member of its
    // job) is the second one at first, and the grandchild only starts a second later.
    wait_until("the grandchild", || {
        job.describe().to_lowercase().contains("ping.exe(pid ")
    });

    // The members are the root and what it started, by id, and described by image name with their parents.
    let members = job.members().unwrap();
    assert!(members.len() >= 2, "{members:?}");
    assert!(
        members.contains(&child.id()),
        "{members:?} lacks {}",
        child.id()
    );
    let described = job.describe();
    assert!(described.starts_with("active="), "{described}");
    assert!(described.contains("total="), "{described}");
    assert!(described.contains("terminated=0"), "{described}");
    assert!(
        described
            .to_lowercase()
            .contains(&format!("cmd.exe(pid {} ppid ", child.id())),
        "{described}"
    );
    assert!(
        described.to_lowercase().contains("ping.exe(pid "),
        "{described}"
    );

    // A stop that works says so, and how long it took; without TJOMMIE_WORKER_DIAG it does not list the job first.
    let (stopped, seen) = job.stop_reported();
    assert!(stopped, "{seen}");
    assert!(seen.contains("empty "), "{seen}");
    assert!(seen.contains("after TerminateJobObject"), "{seen}");
    assert_eq!(job.active().unwrap(), 0);
    assert_eq!(job.members().unwrap(), Vec::<u32>::new());
    assert!(job.describe().contains("active=0"), "{}", job.describe());
    assert_eq!(child.wait().unwrap().code(), Some(71));
}

#[test]
fn stopping_an_empty_job_is_confirmed_at_once() {
    let owner = logon::current_user_sid().unwrap();
    let mut child = spawn::command("cmd");
    child.args(["/c", "exit 0"]).stdout(Stdio::null());
    let mut child = child.spawn().unwrap();
    let job = Job::bind(HANDLE(child.as_raw_handle()), None, &owner).unwrap();
    child.wait().unwrap();
    wait_until("the job to empty", || job.active().unwrap() == 0);
    let (stopped, seen) = job.stop_reported();
    assert!(stopped, "{seen}");
    assert!(job.stop());
}
