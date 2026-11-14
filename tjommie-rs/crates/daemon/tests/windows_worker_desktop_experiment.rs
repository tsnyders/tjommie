//! Admin experiment, two questions in one run, both about the real worker launch path (`logon::launch`, the same call
//! the boundary makes, with the same private window station and desktop):
//!
//! 1. The acceptance probe's `ownerDesktopDenied` was `false` on a hosted runner. It counts a denial only when
//!    `OpenInputDesktop(0,false,0x0108)` returns no handle AND `GetLastError()==5`, so `false` is either (a) the worker
//!    got a handle, a real isolation hole, or (b) the call failed with another error (no interactive session, a station
//!    that is not interactive). The report below says which, with the error as a number and a name, for each worker
//!    identity and, for comparison, for the test process itself.
//! 2. PowerShell took 12 to 19 s to start under a worker's environment but 0.2 s under the policy observer's minimal
//!    one. The bisect times PowerShell under each identity with the full environment, the minimal one, the full one
//!    minus each variable (and a few groups), three runs each, and with the probe's `Add-Type`.
//!
//! It changes nothing: the pinned probe and its wrapper are untouched. It prints, and asserts only that no worker was
//! handed the owner's input desktop. Admin-gated (it creates two accounts); the opt-in `Worker acceptance (admin,
//! disposable runner)` workflow runs it. Never run it on a PC with a real Tjommie install. Run with `--nocapture`.
#![cfg(windows)]

use daemon::win::cred::{self, Namespace};
use daemon::win::logon;
use daemon::win::worker::{Purpose, WorkerConfiguration, launch_frame, worker_name};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// Reports what a process can see of the desktop, as this user. C# only for the Win32 calls; PowerShell builds the JSON.
const DESKTOP_REPORT: &str = r#"
$ErrorActionPreference='Stop'
Add-Type -TypeDefinition @'
using System;using System.Runtime.InteropServices;using System.Text;
public static class TjDesk {
 [DllImport("user32.dll",SetLastError=true)] static extern IntPtr OpenInputDesktop(uint flags,bool inherit,uint access);
 [DllImport("user32.dll",SetLastError=true)] static extern bool CloseDesktop(IntPtr desktop);
 [DllImport("user32.dll",SetLastError=true)] static extern IntPtr GetProcessWindowStation();
 [DllImport("user32.dll",SetLastError=true)] static extern IntPtr GetThreadDesktop(uint thread);
 [DllImport("user32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern IntPtr OpenWindowStation(string name,bool inherit,uint access);
 [DllImport("user32.dll",SetLastError=true)] static extern bool CloseWindowStation(IntPtr station);
 [DllImport("user32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern IntPtr OpenDesktop(string name,uint flags,bool inherit,uint access);
 [DllImport("user32.dll",CharSet=CharSet.Unicode,EntryPoint="GetUserObjectInformationW",SetLastError=true)] static extern bool ObjectName(IntPtr handle,int index,StringBuilder value,int length,out int needed);
 [DllImport("user32.dll",EntryPoint="GetUserObjectInformationW",SetLastError=true)] static extern bool ObjectFlags(IntPtr handle,int index,out Flags value,int length,out int needed);
 [DllImport("kernel32.dll")] static extern uint GetCurrentThreadId();
 [DllImport("kernel32.dll")] static extern bool ProcessIdToSessionId(uint process,out uint session);
 [DllImport("kernel32.dll")] static extern uint WTSGetActiveConsoleSessionId();
 [DllImport("wtsapi32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool WTSQuerySessionInformation(IntPtr server,uint session,int info,out IntPtr buffer,out int length);
 [DllImport("wtsapi32.dll")] static extern void WTSFreeMemory(IntPtr memory);
 [StructLayout(LayoutKind.Sequential)] struct Flags { public int Inherit; public int Reserved; public uint Value; }
 static string Name(IntPtr handle) { var text=new StringBuilder(256); int needed; return ObjectName(handle,2,text,text.Capacity*2,out needed) ? text.ToString() : "(no name: error "+Marshal.GetLastWin32Error()+")"; }
 static string Visible(IntPtr handle) { Flags value; int needed; return ObjectFlags(handle,1,out value,Marshal.SizeOf(typeof(Flags)),out needed) ? ((value.Value&1)!=0 ? "visible (interactive)" : "not visible (not interactive)") : "(no flags: error "+Marshal.GetLastWin32Error()+")"; }
 public static uint Session() { uint session; return ProcessIdToSessionId((uint)System.Diagnostics.Process.GetCurrentProcess().Id,out session) ? session : 0xffffffff; }
 public static uint Console() { return WTSGetActiveConsoleSessionId(); }
 public static string ConsoleUser() { IntPtr buffer; int length; uint session=WTSGetActiveConsoleSessionId(); if(session==0xffffffff) return "(no console session)"; if(!WTSQuerySessionInformation(IntPtr.Zero,session,5,out buffer,out length)) return "(error "+Marshal.GetLastWin32Error()+")"; var user=Marshal.PtrToStringUni(buffer); WTSFreeMemory(buffer); return user==""?"(nobody)":user; }
 public static string Station() { var station=GetProcessWindowStation(); return station==IntPtr.Zero ? "(none)" : Name(station)+" / "+Visible(station); }
 public static string ThreadDesktop() { var desktop=GetThreadDesktop(GetCurrentThreadId()); return desktop==IntPtr.Zero ? "(none)" : Name(desktop); }
 public static string Input(uint access) { var desktop=OpenInputDesktop(0,false,access); int error=Marshal.GetLastWin32Error(); if(desktop==IntPtr.Zero) return "0|"+error+"|"; var name=Name(desktop); CloseDesktop(desktop); return "1|0|"+name; }
 public static string WinSta0() { var station=OpenWindowStation("WinSta0",false,2); int error=Marshal.GetLastWin32Error(); if(station==IntPtr.Zero) return "0|"+error+"|"; var text=Name(station)+" / "+Visible(station); CloseWindowStation(station); return "1|0|"+text; }
 public static string OwnDesktop() { var desktop=OpenDesktop("Default",0,false,1); int error=Marshal.GetLastWin32Error(); if(desktop==IntPtr.Zero) return "0|"+error+"|"; var name=Name(desktop); CloseDesktop(desktop); return "1|0|"+name; }
}
'@
$names=@{0='NONE';1='ERROR_INVALID_FUNCTION';5='ERROR_ACCESS_DENIED';6='ERROR_INVALID_HANDLE';87='ERROR_INVALID_PARAMETER';1314='ERROR_PRIVILEGE_NOT_HELD';1459='ERROR_REQUIRES_INTERACTIVE_WINDOWSTATION'}
function Describe($label,$text) { $p=$text.Split('|'); $e=[int]$p[1]; $n=if($names.ContainsKey($e)){$names[$e]}else{'ERROR_'+$e}; [ordered]@{call=$label;handle=($p[0] -eq '1');error=$e;errorName=$n;errorMessage=(New-Object ComponentModel.Win32Exception($e)).Message;detail=$p[2]} }
[ordered]@{
 user=[Environment]::UserName
 sid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value
 session=[TjDesk]::Session()
 consoleSession=[TjDesk]::Console()
 consoleUser=[TjDesk]::ConsoleUser()
 station=[TjDesk]::Station()
 desktop=[TjDesk]::ThreadDesktop()
 calls=@(
  (Describe 'OpenInputDesktop(0,false,0x0108) [the probe]' ([TjDesk]::Input(0x0108))),
  (Describe 'OpenInputDesktop(0,false,0)' ([TjDesk]::Input(0))),
  (Describe 'OpenInputDesktop(0,false,DESKTOP_READOBJECTS)' ([TjDesk]::Input(1))),
  (Describe 'OpenWindowStation(WinSta0,READATTRIBUTES)' ([TjDesk]::WinSta0())),
  (Describe 'OpenDesktop(Default,READOBJECTS) on its own station' ([TjDesk]::OwnDesktop()))
 )
} | ConvertTo-Json -Compress -Depth 5
"#;

/// The bisect, run by the worker's own `node.exe`: PowerShell timed three times under each environment variant, one JSON
/// line per run as it finishes. The full environment is the one `worker_environment` built for this worker.
const BISECT: &str = r#"const {spawnSync}=require('node:child_process');const ps=process.env.SystemRoot+'/System32/WindowsPowerShell/v1.0/powershell.exe';const base=Object.assign({},process.env);const minus=names=>{const e=Object.assign({},base);for(const k of Object.keys(e))if(names.some(n=>n.toLowerCase()===k.toLowerCase()))delete e[k];return e};const minimal={SystemRoot:base.SystemRoot,WINDIR:base.SystemRoot};const exit0='exit 0';const compile='Add-Type -TypeDefinition "public class Probe { public static int Value() { return 1; } }"; exit 0';const variants=[['full',base,exit0],['minimal',minimal,exit0]];for(const k of Object.keys(base))variants.push(['minus '+k,minus([k]),exit0]);for(const g of [['USERPROFILE','HOME'],['APPDATA','LOCALAPPDATA'],['TEMP','TMP'],['USERPROFILE','HOME','APPDATA','LOCALAPPDATA','TEMP','TMP']])variants.push(['minus '+g.join('+'),minus(g),exit0]);variants.push(['full + Add-Type',base,compile],['minimal + Add-Type',minimal,compile]);for(const [variant,env,body] of variants)for(let run=1;run<=3;run++){const t=Date.now();const r=spawnSync(ps,['-NoLogo','-NoProfile','-NonInteractive','-Command',body],{env,windowsHide:true,encoding:'utf8',timeout:60000});process.stdout.write(JSON.stringify({variant,run,ms:Date.now()-t,status:r.status})+'\n')}process.stdout.write(JSON.stringify({done:true,vars:Object.keys(base)})+'\n')"#;

/// `node -e` arguments that run `script` once in PowerShell and print what came back as one JSON line.
fn report_args(script: &str) -> Vec<String> {
    let program = format!(
        "const{{spawnSync}}=require('node:child_process');const p=spawnSync(process.env.SystemRoot+'/System32/WindowsPowerShell/v1.0/powershell.exe',['-NoLogo','-NoProfile','-NonInteractive','-Command',{}],{{windowsHide:true,encoding:'utf8',timeout:120000}});process.stdout.write(JSON.stringify({{status:p.status,out:p.stdout,err:p.stderr}})+'\\n')",
        serde_json::to_string(script).unwrap()
    );
    vec!["-e".into(), program]
}

/// What the probe's call meant, as (a) or (b), for one report.
fn conclusion(report: &Value) -> String {
    let call = &report["calls"][0];
    let (handle, error) = (call["handle"].as_bool(), call["error"].as_i64());
    match (handle, error) {
        (Some(true), _) => format!(
            "(a) A HANDLE WAS GOT to the input desktop `{}` (for a worker that is a real isolation hole; for the owner it is the control: the call can succeed)",
            call["detail"].as_str().unwrap_or("?")
        ),
        (Some(false), Some(5)) => {
            "denied with error 5 (ERROR_ACCESS_DENIED), which the probe counts as a denial".into()
        }
        (Some(false), Some(code)) => format!(
            "(b) no handle, and the error was {code} {} (\"{}\"), not 5: the probe's rule calls that not denied, but nothing was reached",
            call["errorName"].as_str().unwrap_or("?"),
            call["errorMessage"].as_str().unwrap_or("?")
        ),
        _ => "no result".into(),
    }
}

/// The median of three runs, in ms; a run that was killed counts as its timeout.
fn median(runs: &[i64]) -> i64 {
    let mut sorted = runs.to_vec();
    sorted.sort_unstable();
    sorted[sorted.len() / 2]
}

/// One line naming what makes PowerShell slow, from the runs of each variant.
fn verdict(runs: &BTreeMap<String, Vec<i64>>) -> String {
    let (Some(full), Some(minimal)) = (runs.get("full"), runs.get("minimal")) else {
        return "incomplete: the full or minimal environment was not timed".into();
    };
    let (full, minimal) = (median(full), median(minimal));
    if full < 3000 {
        return format!("PowerShell is not slow here: {full} ms under the full environment");
    }
    if minimal >= 3000 {
        return format!(
            "not the environment: PowerShell is slow even under the minimal one ({minimal} ms, full {full} ms)"
        );
    }
    let restoring = |group: bool| -> Vec<String> {
        runs.iter()
            .filter(|(name, _)| name.starts_with("minus ") && name.contains('+') == group)
            .filter(|(_, ms)| median(ms) < full / 2)
            .map(|(name, _)| name.trim_start_matches("minus ").to_string())
            .collect()
    };
    let (singles, groups) = (restoring(false), restoring(true));
    if !singles.is_empty() {
        format!(
            "caused by {}: removing it alone restores speed (full {full} ms, minimal {minimal} ms)",
            singles.join(" and ")
        )
    } else if !groups.is_empty() {
        format!(
            "not a single variable: removing {} together restores speed, no one of them alone does (full {full} ms, minimal {minimal} ms)",
            groups.join(" or ")
        )
    } else {
        format!(
            "not a single variable and no group tried restores speed (full {full} ms, minimal {minimal} ms)"
        )
    }
}

fn which_node() -> PathBuf {
    let output = daemon::spawn::command("where")
        .arg("node")
        .output()
        .unwrap();
    PathBuf::from(
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .trim(),
    )
}

/// Runs `node.exe` with `args` as `config`'s worker through the real launch path and returns its output lines, printed
/// as they arrive. A run past `limit` is stopped.
fn under_worker(
    label: &str,
    config: &WorkerConfiguration,
    args: &[String],
    limit: Duration,
) -> Vec<String> {
    let started = Instant::now();
    let say = |text: String| eprintln!("[{label} +{:>6}ms] {text}", started.elapsed().as_millis());
    let scratch = config
        .worker_root
        .join(format!("experiment-{}", label.replace(' ', "-")));
    fs::create_dir_all(&scratch).unwrap();
    let saved = cred::read(&config.credential_target).unwrap().unwrap();
    let password = serde_json::from_str::<Value>(&saved).unwrap()["password"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut process = match logon::launch(config, &password, &scratch, &[]) {
        Ok(process) => process,
        Err((message, stopped)) => {
            say(format!(
                "LAUNCH FAILED (tree confirmed stopped: {stopped}): {message}"
            ));
            return vec![];
        }
    };
    let node = config.runtime_root.join("node.exe");
    let frame = launch_frame(node.to_str().unwrap(), args).unwrap();
    let mut stdin = process.worker.stdin().unwrap();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(&frame);
        stdin
    });
    let (mut out, mut err) = (
        process.worker.stdout().unwrap(),
        process.worker.stderr().unwrap(),
    );
    let (sender, lines) = mpsc::channel();
    std::thread::spawn(move || {
        let (mut chunk, mut pending) = ([0u8; 4096], Vec::new());
        while let Ok(count) = out.read(&mut chunk) {
            if count == 0 {
                break;
            }
            pending.extend_from_slice(&chunk[..count]);
            while let Some(at) = pending.iter().position(|b| *b == 10) {
                let line: Vec<u8> = pending.drain(..=at).collect();
                let _ = sender.send(String::from_utf8_lossy(&line).trim().to_owned());
            }
        }
    });
    std::thread::spawn(move || {
        let mut sink = Vec::new();
        let _ = err.read_to_end(&mut sink);
    });
    process.worker.resume().unwrap();
    let mut collected = Vec::new();
    loop {
        match lines.recv_timeout(Duration::from_millis(500)) {
            Ok(line) => {
                say(line.clone());
                collected.push(line);
            }
            Err(RecvTimeoutError::Timeout) if started.elapsed() > limit => {
                say(format!("STOPPED after {}s", limit.as_secs()));
                break;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    let settled = process.retire();
    say(format!("retire() returned {settled}"));
    let _ = writer.join();
    collected
}

/// The worker's own report, or this process's: the JSON of `DESKTOP_REPORT`.
fn parse_report(lines: &[String]) -> Option<Value> {
    let wrapped: Value = serde_json::from_str(lines.last()?).ok()?;
    serde_json::from_str(wrapped["out"].as_str()?.trim()).ok()
}

fn print_report(who: &str, report: &Value) {
    eprintln!(
        "== {who}: user {} ({}), session {}, console session {} (user at the console: {})",
        report["user"],
        report["sid"],
        report["session"],
        report["consoleSession"],
        report["consoleUser"]
    );
    eprintln!(
        "   window station: {} | desktop: {}",
        report["station"], report["desktop"]
    );
    for call in report["calls"].as_array().into_iter().flatten() {
        eprintln!(
            "   {} -> handle {}, error {} {} \"{}\" {}",
            call["call"],
            call["handle"],
            call["error"],
            call["errorName"],
            call["errorMessage"],
            call["detail"]
        );
    }
    eprintln!("   CONCLUSION ({who}): {}", conclusion(report));
}

/// The report run as this user, with PowerShell directly.
fn own_report() -> Value {
    let output = daemon::spawn::command("powershell")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            DESKTOP_REPORT,
        ])
        .output()
        .unwrap();
    serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|_| panic!("{}", String::from_utf8_lossy(&output.stderr)))
}

#[test]
fn the_desktop_report_runs_as_this_user_and_has_every_field() {
    let report = own_report();
    for key in [
        "user",
        "sid",
        "session",
        "consoleSession",
        "consoleUser",
        "station",
        "desktop",
    ] {
        assert!(!report[key].is_null(), "{key} missing");
    }
    let calls = report["calls"].as_array().unwrap();
    assert_eq!(calls.len(), 5);
    assert!(
        calls[0]["call"]
            .as_str()
            .unwrap()
            .starts_with("OpenInputDesktop(0,false,0x0108)")
    );
    // Informational: what this machine's own user sees (a PC with a signed-in user normally gets a handle).
    print_report("this user, locally", &report);
}

#[test]
fn the_conclusion_tells_a_handle_from_a_denial_from_another_error() {
    let call = |handle: bool, error: i64, name: &str| serde_json::json!({"calls":[{"handle":handle,"error":error,"errorName":name,"errorMessage":"m","detail":"Default"}]});
    assert!(conclusion(&call(true, 0, "NONE")).starts_with("(a) A HANDLE WAS GOT"));
    assert!(conclusion(&call(false, 5, "ERROR_ACCESS_DENIED")).starts_with("denied with error 5"));
    let other = conclusion(&call(
        false,
        1459,
        "ERROR_REQUIRES_INTERACTIVE_WINDOWSTATION",
    ));
    assert!(
        other.starts_with("(b)") && other.contains("1459") && other.contains("not 5"),
        "{other}"
    );
}

#[test]
fn the_verdict_names_a_single_variable_a_group_or_neither() {
    let table = |rows: &[(&str, [i64; 3])]| {
        rows.iter()
            .map(|(name, ms)| (name.to_string(), ms.to_vec()))
            .collect::<BTreeMap<_, _>>()
    };
    let slow = [12000, 12100, 11900];
    let fast = [200, 210, 190];
    let single = verdict(&table(&[
        ("full", slow),
        ("minimal", fast),
        ("minus APPDATA", slow),
        ("minus LOCALAPPDATA", fast),
        ("minus TEMP+TMP", slow),
    ]));
    assert!(single.starts_with("caused by LOCALAPPDATA"), "{single}");
    let group = verdict(&table(&[
        ("full", slow),
        ("minimal", fast),
        ("minus APPDATA", slow),
        ("minus APPDATA+LOCALAPPDATA", fast),
    ]));
    assert!(
        group.starts_with("not a single variable: removing APPDATA+LOCALAPPDATA"),
        "{group}"
    );
    let neither = verdict(&table(&[
        ("full", slow),
        ("minimal", fast),
        ("minus APPDATA", slow),
    ]));
    assert!(
        neither.starts_with("not a single variable and no group"),
        "{neither}"
    );
    assert!(
        verdict(&table(&[("full", fast), ("minimal", fast)])).starts_with("PowerShell is not slow")
    );
    assert!(
        verdict(&table(&[("full", slow), ("minimal", slow)])).starts_with("not the environment")
    );
}

#[test]
fn the_bisect_program_is_valid_javascript() {
    let mut check = daemon::spawn::command(which_node());
    check
        .arg("--check")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    let mut child = check.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(BISECT.as_bytes())
        .unwrap();
    let done = child.wait_with_output().unwrap();
    assert!(
        done.status.success(),
        "{}",
        String::from_utf8_lossy(&done.stderr)
    );
}

/// Admin, opt-in (`TJOMMIE_RS_ADMIN_TESTS=1`): see the file's header.
#[test]
#[ignore = "admin: needs elevation; run with TJOMMIE_RS_ADMIN_TESTS=1 cargo test -- --ignored"]
fn admin_worker_desktop_and_powershell_experiment() {
    if std::env::var("TJOMMIE_RS_ADMIN_TESTS").as_deref() != Ok("1") {
        eprintln!("skipped: set TJOMMIE_RS_ADMIN_TESTS=1 on a disposable machine");
        return;
    }
    let folder = TempDir::new().unwrap();
    let owner_root = folder.path().join("owner");
    assert!(
        !cred::is_production_root(&owner_root),
        "refusing the production root"
    );
    let ns = Namespace::new(&owner_root).unwrap();
    let names = [
        worker_name(&ns, Purpose::Provider),
        worker_name(&ns, Purpose::Review),
    ];
    let program_data = PathBuf::from(std::env::var_os("ProgramData").unwrap());
    let base = program_data.join(format!(r"Tjommie\Worker-{}", ns.short()));
    for name in &names {
        assert!(
            logon::account_sid(name).unwrap().is_none(),
            "{name} already exists"
        );
    }
    let root = owner_root.clone();
    let unit = base.clone();
    let result = std::panic::catch_unwind(move || {
        logon::provision(&root).unwrap();
        fs::copy(which_node(), unit.join("runtime").join("node.exe")).unwrap();
        let provider =
            WorkerConfiguration::load(&root.join("worker.json"), Purpose::Provider).unwrap();
        let review =
            WorkerConfiguration::load(&root.join("review-worker.json"), Purpose::Review).unwrap();

        eprintln!(
            "######## 1. THE DESKTOP: what each identity sees (the probe's call first) ########"
        );
        let own = own_report();
        print_report("the test process (the runner's own admin user)", &own);
        let mut holes = vec![];
        for (label, config) in [("provider", &provider), ("review", &review)] {
            let lines = under_worker(
                &format!("{label} desktop"),
                config,
                &report_args(DESKTOP_REPORT),
                Duration::from_secs(150),
            );
            match parse_report(&lines) {
                Some(report) => {
                    print_report(&format!("the {label} worker"), &report);
                    if report["calls"][0]["handle"] == true {
                        holes.push(label);
                    }
                }
                None => eprintln!("== the {label} worker: NO REPORT (see the lines above)"),
            }
        }

        eprintln!(
            "######## 2. POWERSHELL: milliseconds per run, by environment (one launch per identity) ########"
        );
        for (label, config) in [("provider", &provider), ("review", &review)] {
            let lines = under_worker(
                &format!("{label} bisect"),
                config,
                &["-e".into(), BISECT.into()],
                Duration::from_secs(12 * 60),
            );
            let mut runs: BTreeMap<String, Vec<i64>> = BTreeMap::new();
            for line in &lines {
                if let Ok(value) = serde_json::from_str::<Value>(line)
                    && let (Some(variant), Some(ms)) =
                        (value["variant"].as_str(), value["ms"].as_i64())
                {
                    runs.entry(variant.to_string()).or_default().push(ms);
                } else if line.contains("\"done\"") {
                    eprintln!("   the environment the worker was given: {line}");
                }
            }
            eprintln!("   TABLE {label}: variant | run 1 | run 2 | run 3 (ms)");
            for (variant, ms) in &runs {
                eprintln!(
                    "   {label} | {variant:<34} | {}",
                    ms.iter()
                        .map(|m| format!("{m:>6}"))
                        .collect::<Vec<_>>()
                        .join(" | ")
                );
            }
            eprintln!("   VERDICT {label}: {}", verdict(&runs));
        }
        assert!(
            holes.is_empty(),
            "the worker(s) {holes:?} got a handle to the input desktop"
        );
    });
    let base = program_data.join(format!(r"Tjommie\Worker-{}", ns.short()));
    for name in &names {
        let _ = logon::delete_account(name);
    }
    let _ = cred::delete(&ns.isolated_worker_target());
    let _ = cred::delete(&ns.isolated_review_target());
    if base.starts_with(program_data.join("Tjommie")) {
        let _ = fs::remove_dir_all(&base);
    }
    for name in &names {
        assert!(
            logon::account_sid(name).unwrap().is_none(),
            "{name} was not removed"
        );
    }
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
