# The quiet-PC performance session (plan M7, T7.4)

One sitting, about two and a half hours, on your PC with nothing else running. It proves the motion frame times, the
memory budget, the cold start and the heavy-load experiment. No real accounts and no paid turns are needed anywhere:
the motion runs use invented data, and the memory runs use a recorded-conversation ("fixtures") daemon.

Every command is PowerShell, run from the repo folder of this branch. Nothing here touches your installed Tjommie
(`%LOCALAPPDATA%\Tjommie`), its ports 47100/47101, or Credential Manager. The app's data goes to throwaway folders under
`%TEMP%` or to `%LOCALAPPDATA%\Tjommie-rs\perf\`.

The motions are listed in `tjommie-rs\motion-map.md`. Step 2 times the ones 0.6.0 ships that have a timed scenario:
`motion-recents`, `motion-flight`, `motion-toast`, `motion-panel`, `motion-numbers`, `motion-arrival`, `motion-alive`
and `scroll-10k`. The motions the map defers to 0.6.1 are not in this build, so nothing here measures them.

## 0. Close everything first

1. Quit the installed Tjommie (tray icon too), Paseo and every agent, browsers, Teams or Slack, OneDrive sync, games, video
   players, Visual Studio or rust-analyzer, and any `cargo` build. Check Task Manager: CPU should sit under 10%.
2. Plug the laptop in (if it is one), set the power mode to Best performance, and let the screen stay on.
3. Keep the screen unlocked and leave the PC alone while a script runs. A full-screen app or the lock screen in front
   makes the app's window unable to draw; the scripts detect it and wait.
4. PresentMon: the scripts look for `PresentMon*.exe` in `%LOCALAPPDATA%\Tjommie-rs\perf\tools\presentmon\`. If it is not there:

```powershell
$to = "$env:LOCALAPPDATA\Tjommie-rs\perf\tools\presentmon"
New-Item -ItemType Directory -Force $to | Out-Null
Copy-Item "$env:LOCALAPPDATA\Tjommie-spike\tools\presentmon\PresentMon*.exe" $to   # the spike's copy, if you still have it
```

   Otherwise download PresentMon 2.x from its GitHub releases and put the exe there.

## 1. Build

```powershell
cd <repo>\tjommie-rs
cargo build --profile perf -p desktop --features test-driver --locked
cargo build --profile perf -p daemon --bin tjommie-daemon --locked
cd ..
```

This gives `tjommie-rs\target\perf\tjommie-desktop.exe` and `tjommie-daemon.exe`. (The `perf` profile is a release
build with the test driver; the normal release stays free of it.) Run the plumbing proof once before the long runs:

```powershell
.\tjommie-rs\tools\perf\run-motion-checks.ps1 -Smoke -Labels motion-toast,motion-panel
```

It runs 5 loops of each named motion once. If it ends without a warning, the tools work. It is not a measurement.

## 2. Motion p95 (T7.2, T7.4): every timed motion, 3 runs of 50 loops

```powershell
.\tjommie-rs\tools\perf\run-motion-checks.ps1 -Only p95
```

About 25 minutes. Each motion runs 3 times; the script waits for a quiet PC before each run (CPU at most 35% over 5 s)
and marks a run `loaded` if the CPU averaged above 50% during it.

**Pass rule:** the p95 frame interval is at most **16.7 ms on all 3 runs** of every motion. The number comes from PresentMon;
if PresentMon loses more than 1% of the presents (it does while the browser page moves), the run is judged on the app's own
frame intervals instead, and the line says `judged on app`. `motion-alive` is the living avatars (20 s) and `scroll-10k`
the 10,000-message scroll; they use the same rule.

- Record: the final line per motion (`... pass (p95 <= 16.7 ms on every run): True/False`) and any `loaded` warning.
  A failing motion that had a `loaded` run: rerun just that motion on a quieter PC:
  `.\tjommie-rs\tools\perf\run-motion-checks.ps1 -Only p95 -Labels motion-panel`.
- Evidence lands in `tjommie-rs\evidence\<motion name>\` (`summary.json`, per-run CSVs). Raw captures are in
  `%LOCALAPPDATA%\Tjommie-rs\perf\runs\<motion name>\`.
- Send back: the `summary.json` of every motion folder (or the folder zipped).

## 3. The checks (no timing)

```powershell
.\tjommie-rs\tools\perf\run-motion-checks.ps1 -Only checks
```

This runs three checks and prints one line per result (`pass: true/false`):

- keyboard: the motions a key can start (opening a chat from Recents, closing a toast) land with 0 in-between frames;
- reduced motion, **simulated** (`--force-reduced`): the same by the pointer, and Recents reordering, with 0 in-between
  frames;
- idle (about 45 s): with one agent at work, the window draws for 20 s in front, then the taskbar takes the front and the
  window, still visible, must draw at most 1 frame a second for 20 s. Leave the PC alone while it runs. Record both
  rates from the results file (`frames_per_second_behind`, `frames_per_second_in_front`).

Interruption (a motion caught mid-flight turns without a jump) is proven by each motion's unit tests (`motion-map.md`).

Evidence: `tjommie-rs\evidence\motion\results.jsonl`.

### 3b. Reduced motion, the real setting (by hand)

1. Windows Settings, Accessibility, Visual effects, turn **Animation effects** off.
2. Run the check without the simulation flag (the app then reads Windows' setting):

```powershell
$t = Join-Path $env:TEMP 'tjommie-perf-reduced'; New-Item -ItemType Directory -Force $t | Out-Null
$env:TJOMMIE_HOME = $t
New-Item -ItemType Directory -Force .\tjommie-rs\evidence\motion-reduced-real | Out-Null
$p = Start-Process .\tjommie-rs\target\perf\tjommie-desktop.exe -PassThru -Wait -ArgumentList '--scenario','motion-reduced','--out',"$PWD\tjommie-rs\evidence\motion-reduced-real\results.jsonl"
"exit code: $($p.ExitCode)"
Remove-Item Env:TJOMMIE_HOME; Remove-Item $t -Recurse -Force
```

3. **Turn Animation effects back on.** Record the exit code (0 is a pass) and send back the results file.

## 4. The 10,000-message scroll

It is part of step 2 (`scroll-10k`), judged by the same rule. Record its p95 (the spike measured 11 to 13 ms).

## 5. Memory, states S0 to S4 on the real app

The targets (private working set, MiB): **S0 at most 120**, **S1 at most 280**, **S2 at most S1 + 40**, **S3 at most S1 - 20**, **S4 at most S1 + 10**.
Private commit: S0 at most 160, S1 at most 330, S2 at most S1 + 50. `measure-memory.ps1` waits for a quiet PC, idles 60 s,
takes 10 samples and reports the median; it prints each pass/fail against the targets.

### Setup (once)

1. A fixtures daemon: a daemon that plays recorded conversations, so nothing real runs. It must be a dev-profile build
   (a release build refuses the feature):

```powershell
cd <repo>\tjommie-rs
cargo build -p daemon --bin tjommie-daemon --features provider-fixtures
cd ..
```

2. A recorded turn that opens the browser page. Save as `%LOCALAPPDATA%\Tjommie-rs\perf\fixtures\default.json` (every
   agent without its own file uses it):

```powershell
$f = "$env:LOCALAPPDATA\Tjommie-rs\perf\fixtures"; New-Item -ItemType Directory -Force $f | Out-Null
@'
{ "version": 1, "turns": [ { "events": [
  { "type": "browser", "id": "b1", "tool": "browser_navigate", "arguments": { "url": "http://127.0.0.1:8080/controls.html" } },
  { "type": "text", "delta": "Opened the page." } ] } ] }
'@ | Set-Content -Encoding utf8 "$f\default.json"
```

3. Serve the page the bots open (leave this window open; it uses next to no CPU):

```powershell
python -m http.server 8080 --bind 127.0.0.1 --directory <repo>\tjommie-rs\crates\desktop-host\tests\fixtures
```

   The address bar and the bots' browser accept `http://127.0.0.1:<port>/...` (local addresses get plain `http`, and
   the browser policy allows `http` and `https` only, no credentials), so no internet is used.

### Start the app on a dev root

```powershell
$env:TJOMMIE_HOME = "$env:LOCALAPPDATA\Tjommie-rs\perf\dev-root"   # a dev root; never %LOCALAPPDATA%\Tjommie
$env:TJOMMIE_PROVIDER_FIXTURES = "$env:LOCALAPPDATA\Tjommie-rs\perf\fixtures"
$env:TJOMMIE_DAEMON_EXE = "<repo>\tjommie-rs\target\debug\tjommie-daemon.exe"     # the fixtures daemon, for S1 onwards
$app = Start-Process "<repo>\tjommie-rs\target\perf\tjommie-desktop.exe" -PassThru
$app.Id    # the root pid for -Pid
```

Use the fixtures daemon for every state (its fixtures accounts let the shell open with no sign-in). A debug build
uses more memory than the shipped daemon, so the per-process list in each report matters: the daemon row of S0 is the
one to correct. If you want a cleaner S0, also measure it once with the shipped-style daemon: quit, set
`TJOMMIE_DAEMON_EXE` to `<repo>\tjommie-rs\target\perf\tjommie-daemon.exe`, leave `TJOMMIE_PROVIDER_FIXTURES` unset, and if the shell
opens without a sign-in, measure S0 again; send back both.

### The states

Each ends with a measuring command. Leave the PC alone while it runs (about 75 s).

- **S0.** App and daemon only; no agent ever opened its browser. `msedgewebview2.exe` must not appear in the tree
  (the script warns if it does).

```powershell
.\tjommie-rs\tools\perf\measure-memory.ps1 -Target Desktop -State S0 -Pid $app.Id
```

- **S1.** Create two agents (Bots, New) and switch each one's **Browser** on in its settings. Select agent A and send it
  "hello" (the recorded turn opens the page). Open the browser panel (Ctrl+B) so A's page shows.

```powershell
.\tjommie-rs\tools\perf\measure-memory.ps1 -Target Desktop -State S1 -Pid $app.Id
```

  Note the two numbers it prints for S1 (`private working set` and `private commit`); the next states need them.
  The daemon row of the fixtures run is bigger than the real daemon; send back the per-process list (the JSON file) and
  also subtract (fixtures daemon row minus S0's daemon row) so S1 reflects the shipped daemon.
- **S2.** From S1: select agent B and send it "hello" (B's page opens), then select agent A again so B is hidden. Do
  nothing for 60 s.

```powershell
.\tjommie-rs\tools\perf\measure-memory.ps1 -Target Desktop -State S2 -Pid $app.Id -S1Mib <S1 private WS> -S1CommitMib <S1 private commit>
```

- **S3.** Back to S1 (A shown, B closed to the idle state): close the panel with Ctrl+B, with nothing streaming, and run
  at once (the script's own 60 s idle is the wait).

```powershell
.\tjommie-rs\tools\perf\measure-memory.ps1 -Target Desktop -State S3 -Pid $app.Id -S1Mib <S1 private WS>
```

- **S4.** Redo S2 (B opened then hidden). Then wait **11 minutes** without touching the app (the hidden bot's browser
  closes after 10 idle minutes), then:

```powershell
.\tjommie-rs\tools\perf\measure-memory.ps1 -Target Desktop -State S4 -Pid $app.Id -S1Mib <S1 private WS>
```

  The tree should show one browser set again.

Record per state: the printed line (private WS, private commit, working set, process count, `LOADED` if the PC was
busy) and the pass/fail lines. Reports land in `%LOCALAPPDATA%\Tjommie-rs\perf\memory-desktop-<state>-<time>.json`.
Send back those five files. A `LOADED` state is rerun on a quieter PC.

### The leak check

After S1 is measured (panel open, A shown, 60 s settled), keep its report path as `$before`, then do 30 panel
open/close cycles and 20 bot switches:

```powershell
$before = (Get-ChildItem "$env:LOCALAPPDATA\Tjommie-rs\perf\memory-desktop-S1-*.json" | Sort-Object LastWriteTime | Select-Object -Last 1).FullName
Add-Type -AssemblyName System.Windows.Forms
Write-Host 'Click inside the Tjommie window now; the toggles start in 5 s and take 60 s.'
Start-Sleep -Seconds 5
1..60 | ForEach-Object { [System.Windows.Forms.SendKeys]::SendWait('^b'); Start-Sleep -Milliseconds 1000 }   # 60 toggles = 30 open/close cycles
```

Then switch between agent A and agent B in the sidebar by hand 20 times (click A, B, A, B...). End with the panel
open on A (the same state as before), wait 60 s, and measure:

```powershell
.\tjommie-rs\tools\perf\measure-memory.ps1 -Target Desktop -State S1 -Pid $app.Id -Compare $before
```

**Pass rule:** `pass_leak_le_15_mib: True` (private commit grew by at most 15 MiB). Record the growth.

### Hidden bots use no CPU

With the panel closed and both bots hidden and idle (as in S3, a few minutes after the last page load):

```powershell
.\tjommie-rs\tools\perf\measure-memory.ps1 -Target Desktop -Pid $app.Id -HiddenCpu
```

**Pass rule:** every renderer used at most 1 s of CPU over 60 s. Record the printed line; the file is
`%LOCALAPPDATA%\Tjommie-rs\perf\hidden-cpu-<time>.json`.

### Finish the memory work

```powershell
Stop-Process -Id $app.Id    # the app stops its daemon; if a tjommie-daemon.exe remains, end only that one in Task Manager
Remove-Item Env:TJOMMIE_HOME, Env:TJOMMIE_PROVIDER_FIXTURES, Env:TJOMMIE_DAEMON_EXE
```

Close the `python -m http.server` window.

## 6. Cold start to the first interactive frame

```powershell
.\tjommie-rs\tools\perf\cold-start.ps1
```

It launches the app 3 times (each on a fresh throwaway root, with the perf daemon), each time to the first present and to
the log marker `ui ready` (logged as the shell renders its first frame with the daemon connected, one frame before that
frame is presented; the first present beside it is PresentMon's). Ports 47200 and 47201 must
be free (no dev daemon running). Compare with the installed 0.4.2.0's **3,837.5 ms** (to the first present).
Record both numbers per run and the medians. Evidence: `tjommie-rs\evidence\cold-start\summary.json`.

## 7. The heavy-load experiment (T7.3)

This one puts the CPU at 60 to 80% on purpose, with a background load of idle-priority threads (`load.ps1`). Do it last;
nothing else may run.

```powershell
.\tjommie-rs\tools\perf\t73-experiment.ps1
```

About 45 minutes. It measures `motion-panel` and `motion-recents` three ways (no option; `--mmcss`, which asks Windows'
multimedia scheduler for "Games" treatment; `--above-normal`, a higher thread priority), 3 runs each, loaded, plus 3 quiet runs of each for
the idle-CPU check. It prints `keep = True/False` per option and writes `tjommie-rs\evidence\t73\decision.json`.

**Rule:** keep an option only if its p95 is at least 2 ms better on all 3 runs with no worse idle CPU. If it warns
"not conclusive" (a loaded run was outside 60 to 80% CPU), adjust the load and repeat: `-LoadThreads 8` (more) or a
lower number (fewer). To see what load a thread count reaches, without measuring anything (about 15 s):
`.\tjommie-rs\tools\perf\load.ps1 -Threads 8 -Seconds 15`. Ctrl+C stops it at once.

Send back: `decision.json` and the `t73-*` evidence folders. Either answer is a result; no option is kept if none helps.

## What to send back

- `tjommie-rs\evidence\` (every motion folder, `motion\`, `motion-reduced-real\`, `cold-start\`, `t73\`), as a zip.
- The memory and hidden-CPU JSON files from `%LOCALAPPDATA%\Tjommie-rs\perf\`.
- Your notes: anything marked `loaded`, any motion that failed, whether the real reduced-motion run passed, and
  whether Animation effects were switched back on.
