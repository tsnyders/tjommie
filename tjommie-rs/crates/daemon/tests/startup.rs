//! Arguments, environment and stdin handling from `main.ts`.

use std::collections::HashMap;
use std::io::{self, Read};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Duration;

use daemon::startup::{Launch, Mode, StdinEvent, parse_launch, watch_stdin};

fn parse(args: &[&str], env: &[(&str, &str)]) -> Result<Launch, String> {
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    let env: HashMap<String, String> = env
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    parse_launch(&args, |key| env.get(key).cloned()).map_err(|error| error.to_string())
}

#[test]
fn picks_one_mode_from_the_flags_and_tjommie_dev() {
    assert_eq!(parse(&["--dev"], &[]).unwrap().mode, Mode::Development);
    assert_eq!(
        parse(&[], &[("TJOMMIE_DEV", "1")]).unwrap().mode,
        Mode::Development
    );
    assert_eq!(
        parse(&["--token-stdin"], &[]).unwrap().mode,
        Mode::Production
    );
    assert_eq!(
        parse(&["--token-stdin"], &[("TJOMMIE_DEV", "0")])
            .unwrap()
            .mode,
        Mode::Production
    );
    assert_eq!(
        parse(&["--dev", "--token-stdin"], &[]).unwrap_err(),
        "Choose either --dev or --token-stdin."
    );
    assert_eq!(
        parse(&["--dev", "--port", "1"], &[]).unwrap_err(),
        "Unknown argument: --port"
    );
}

#[test]
fn reads_the_ports_and_the_account_folder_override() {
    let defaults = parse(&["--dev"], &[]).unwrap();
    let ports = if cfg!(debug_assertions) {
        (47200, 47201)
    } else {
        (47100, 47101)
    };
    assert_eq!(
        (defaults.port, defaults.lan_port, defaults.accounts_dir),
        (ports.0, ports.1, None)
    );
    let set = parse(
        &["--dev"],
        &[
            ("TJOMMIE_PORT", "0"),
            ("TJOMMIE_LAN_PORT", "47200"),
            ("TJOMMIE_ACCOUNTS_DIR", "C:\\accounts"),
        ],
    )
    .unwrap();
    assert_eq!(
        (set.port, set.lan_port, set.accounts_dir),
        (0, 47200, Some(PathBuf::from("C:\\accounts")))
    );
    // main.ts reads TJOMMIE_PORT with Number(), so an empty value means any free port; an empty folder means none.
    let empty = parse(
        &["--dev"],
        &[("TJOMMIE_PORT", ""), ("TJOMMIE_ACCOUNTS_DIR", "")],
    )
    .unwrap();
    assert_eq!((empty.port, empty.accounts_dir), (0, None));
    assert_eq!(
        parse(&["--dev"], &[("TJOMMIE_LAN_PORT", "lan")]).unwrap_err(),
        "TJOMMIE_LAN_PORT must be a port number."
    );
}

fn events(input: impl Read + Send + 'static) -> Vec<StdinEvent> {
    let receiver: Receiver<StdinEvent> = watch_stdin(input);
    std::iter::from_fn(|| receiver.recv_timeout(Duration::from_secs(5)).ok()).collect()
}

/// A reader that fails once its data is used up, like a broken pipe.
struct FailsAfter(io::Cursor<Vec<u8>>);

impl Read for FailsAfter {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self.0.read(buffer)? {
            0 => Err(io::Error::other("synthetic stdin error")),
            read => Ok(read),
        }
    }
}

#[test]
fn reports_the_token_line_then_stdin_closing() {
    assert_eq!(
        events(io::Cursor::new(b"token\r\nignored second line\n".to_vec())),
        [StdinEvent::Line("token".into()), StdinEvent::Closed]
    );
    assert_eq!(
        events(io::Cursor::new(b"no newline".to_vec())),
        [StdinEvent::Line("no newline".into()), StdinEvent::Closed]
    );
    assert_eq!(events(io::Cursor::new(Vec::new())), [StdinEvent::Closed]);
}

#[test]
fn stops_cleanly_when_production_stdin_errors() {
    // main.test.ts injects the error with a Node preload; here a failing reader stands in for the pipe.
    assert_eq!(
        events(FailsAfter(io::Cursor::new(b"token\n".to_vec()))),
        [StdinEvent::Line("token".into()), StdinEvent::Closed]
    );
    assert_eq!(
        events(FailsAfter(io::Cursor::new(Vec::new()))),
        [StdinEvent::Closed]
    );
}

// T3.1 review follow-up.

/// Spec §4: a debug build must never default to the production install's data root and ports.
#[cfg(debug_assertions)]
#[test]
fn debug_builds_default_to_the_dev_identity() {
    use daemon::paths::data_root_from;
    use std::ffi::OsString;
    let local = |key: &str| {
        (key == "LOCALAPPDATA").then(|| OsString::from(r"C:\Users\owner\AppData\Local"))
    };
    assert_eq!(
        data_root_from(local),
        PathBuf::from(r"C:\Users\owner\AppData\Local\Tjommie-rs")
    );
    let home = |key: &str| (key == "TJOMMIE_HOME").then(|| OsString::from(r"D:\elsewhere"));
    assert_eq!(data_root_from(home), PathBuf::from(r"D:\elsewhere"));
    let launch = parse(&["--dev"], &[]).unwrap();
    assert_eq!((launch.port, launch.lan_port), (47200, 47201));
    let set = parse(
        &["--dev"],
        &[("TJOMMIE_PORT", "47100"), ("TJOMMIE_LAN_PORT", "47101")],
    )
    .unwrap();
    assert_eq!((set.port, set.lan_port), (47100, 47101));
}

#[test]
fn removes_a_stale_development_token_that_is_briefly_locked() {
    // rmSync(..., { maxRetries: 3 }): an indexer or antivirus can hold the file for a moment.
    let root = std::env::temp_dir().join(format!("tjommie-rs-test-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let file = root.join("daemon-token");
    std::fs::write(&file, "x".repeat(43)).unwrap();
    let locked =
        std::os::windows::fs::OpenOptionsExt::share_mode(std::fs::OpenOptions::new().read(true), 0)
            .open(&file)
            .unwrap();
    let release = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        drop(locked);
    });
    assert!(daemon::startup::remove_leftover_token(&file).unwrap());
    release.join().unwrap();
    assert!(!file.exists());
    assert!(!daemon::startup::remove_leftover_token(&file).unwrap());
    std::fs::remove_dir_all(&root).unwrap();
}
