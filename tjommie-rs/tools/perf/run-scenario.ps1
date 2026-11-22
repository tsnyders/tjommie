<#
Runs one timed scenario of tjommie-desktop.exe several times under PresentMon and prints msBetweenPresents
p50/p95/max per run (plan T7.2: p95 at most 16.7 ms on 3 runs).

    .\tools\perf\run-scenario.ps1 -Scenario motion-panel                       # 3 runs, label "motion-panel"
    .\tools\perf\run-scenario.ps1 -Scenario motion-panel -Label panel-live-2   # filed under another label
    .\tools\perf\run-scenario.ps1 -Scenario motion-alive -Seconds 20
    .\tools\perf\run-scenario.ps1 -Scenario motion-panel -Loops 5 -Runs 1      # a quick proof
    .\tools\perf\run-scenario.ps1 -Scenario motion-panel -AppArgs '--mmcss'    # a T7.3 option, passed to the app

Build first, inside tjommie-rs (the perf profile inherits release, so target\release stays driver-free):
    cargo build --profile perf -p desktop --features test-driver --locked
    cargo build --profile perf -p daemon --bin tjommie-daemon --locked
The exe is target\perf\tjommie-desktop.exe (-Exe names another). Every run gets a fresh TJOMMIE_HOME under %TEMP%,
created before the run and deleted after it; a scenario shows invented fixture data (no daemon, no network).
Raw captures go to %LOCALAPPDATA%\Tjommie-rs\perf\runs\<label>\ (never the production root); trimmed in-window CSVs,
markers, the app's per-check results (runN-results.jsonl) and summary.json go to tjommie-rs\evidence\<label>\.
PresentMon is looked for in %LOCALAPPDATA%\Tjommie-rs\perf\tools\presentmon\ (-PresentMon names it). It runs hidden;
the app's window opens without focus.

Judging: PresentMon's p95, or the app's own frame intervals when PresentMon lost more than 1% of the presents
(frame-stats.py "presentmon_unusable"). A run is marked loaded when the CPU averaged above MaxLoad + 15 % during it.
#>
param(
    [string]$Scenario = 'motion-panel',
    [int]$Runs = 3,
    [string]$Label = $Scenario,
    [string[]]$AppArgs = @(),
    [int]$Loops = 0,        # 0: the app's default (50 pointer-triggered loops)
    [int]$Seconds = 0,      # 0: the app's default (time-based scenarios only)
    [string]$Exe = '',
    [string]$PresentMon = '',
    # The owner's PC is shared with other agents' builds. Each run waits until system CPU averages at
    # most MaxLoad % over 5 s and the app's window can be drawn (covered.ps1). QuietTimeout is the total wait for
    # all runs of this call: once it is spent, runs start anyway and record their load (a run whose
    # window could not be drawn is skipped). A deliberate background load (load.ps1) sets MaxLoad high.
    [double]$MaxLoad = 35,
    [int]$QuietTimeout = 1200
)
$ErrorActionPreference = 'Stop'
$crate = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent   # tjommie-rs
$exe = if ($Exe) { $Exe } else { Join-Path $crate 'target\perf\tjommie-desktop.exe' }
$root = Join-Path $env:LOCALAPPDATA 'Tjommie-rs\perf'            # never the production root (Tjommie)
if (-not $PresentMon) { $PresentMon = (Get-ChildItem (Join-Path $root 'tools\presentmon') -Filter 'PresentMon*.exe' -ErrorAction SilentlyContinue | Select-Object -First 1).FullName }
foreach ($tool in $exe, $PresentMon) { if (-not $tool -or -not (Test-Path $tool)) { throw "Missing '$tool' (build the app; put PresentMon under $root\tools\presentmon)" } }
$processName = Split-Path $exe -Leaf   # tjommie-desktop.exe
$raw = Join-Path $root "runs\$Label"
$evidence = Join-Path $crate "evidence\$Label"
New-Item -ItemType Directory -Force $raw, $evidence | Out-Null
. (Join-Path $PSScriptRoot 'covered.ps1')

$display = Get-CimInstance Win32_VideoController | Select-Object -First 1
$dpi = (Get-ItemProperty 'HKCU:\Control Panel\Desktop\WindowMetrics' -Name AppliedDPI -ErrorAction SilentlyContinue).AppliedDPI
$session = 'TjommieDesktopPerf'
$counter = '\Processor(_Total)\% Processor Time'
$quietDeadline = (Get-Date).AddSeconds($QuietTimeout)
function Wait-Quiet {
    while ($true) {
        $mean = ((Get-Counter $counter -SampleInterval 1 -MaxSamples 5).CounterSamples.CookedValue | Measure-Object -Average).Average
        $covered = Test-Covered
        if ($mean -le $MaxLoad -and -not $covered) { return [math]::Round($mean, 1) }
        if ((Get-Date) -gt $quietDeadline) {
            if ($covered) { throw 'the app window would not be drawn (a full-screen app in front, or the screen is locked)' }
            Write-Warning ('{0}: no quiet moment within its {1} s budget; running at CPU {2:N0}%, and the load is recorded' -f $Label, $QuietTimeout, $mean)
            return [math]::Round($mean, 1)
        }
        if ($covered) { Write-Host 'waiting: a full-screen app is in front or the screen is locked, and the app window would not be drawn' }
        else { Write-Host ('waiting for a quiet machine: CPU {0:N0}% > {1}%' -f $mean, $MaxLoad) }
    }
}
$results = @()
$failed = @()
$savedHome = $env:TJOMMIE_HOME
for ($run = 1; $run -le $Runs; $run++) {
    $csv = Join-Path $raw "run$run-presentmon.csv"
    $markers = Join-Path $raw "run$run-markers.csv"
    $windows = $markers -replace '\.csv$', '.windows.csv'
    $trace = Join-Path $raw "run$run-trace.csv"
    $cpu = Join-Path $raw "run$run-cpu.csv"
    $out = Join-Path $raw "run$run-results.jsonl"
    Remove-Item $csv, $markers, $windows, $trace, $cpu, $out -ErrorAction SilentlyContinue
    $capture = $load = $app = $null
    $temp = Join-Path ([IO.Path]::GetTempPath()) ('tjommie-perf-' + [guid]::NewGuid().ToString('N').Substring(0, 12))
    try {
        $preLoad = Wait-Quiet
        New-Item -ItemType Directory -Force $temp | Out-Null
        $capture = Start-Process -FilePath $PresentMon -WindowStyle Hidden -PassThru -ArgumentList @(
            '--process_name', $processName, '--output_file', $csv, '--v1_metrics', '--qpc_time',
            '--stop_existing_session', '--session_name', $session, '--no_console_stats')
        Start-Sleep -Seconds 2
        # System-wide CPU load for the whole run (stopped when the app exits): the PC is shared.
        $load = Start-Process -FilePath typeperf -WindowStyle Hidden -PassThru -ArgumentList '"\Processor(_Total)\% Processor Time"', '-si', '1', '-sc', '240', '-f', 'CSV', '-y', '-o', $cpu
        $appArgs = @('--scenario', $Scenario, '--markers', $markers, '--trace', $trace, '--out', $out)
        if ($Loops -gt 0) { $appArgs += '--loops', $Loops }
        if ($Seconds -gt 0) { $appArgs += '--seconds', $Seconds }
        $env:TJOMMIE_HOME = $temp   # inherited by the app; restored after the loop
        $app = Start-Process -FilePath $exe -PassThru -ArgumentList ($appArgs + $AppArgs)
        if (-not $app.WaitForExit(180000)) { throw "run $run timed out" }
        Stop-Process -Id $load.Id -ErrorAction SilentlyContinue
        Start-Sleep -Seconds 1
        Start-Process -FilePath $PresentMon -WindowStyle Hidden -Wait -ArgumentList '--terminate_existing_session', '--session_name', $session
        if (-not $capture.WaitForExit(15000)) { Stop-Process -Id $capture.Id; Write-Warning "PresentMon run $run had to be killed" }
        if ($app.ExitCode -ne 0 -or -not (Test-Path $markers)) { throw "run $run failed (exit $($app.ExitCode)); see $temp\logs\desktop.log (kept)" }
        Copy-Item $markers, $trace, $cpu, $out -Destination $evidence -ErrorAction SilentlyContinue
        $statsArgs = @('--process', $processName, $csv, $markers, (Join-Path $evidence "run$run.csv"), $trace, $cpu)
        if (Test-Path $windows) { $statsArgs += $windows; Copy-Item $windows $evidence }
        $stats = python -I (Join-Path $PSScriptRoot 'frame-stats.py') @statsArgs | ConvertFrom-Json
        if ($LASTEXITCODE -ne 0 -or $null -eq $stats.judged_p95) { throw "frame-stats failed for run $run" }
        $stats | Add-Member run $run
        $stats | Add-Member pre_cpu $preLoad
        # In-run load includes the app and PresentMon themselves (about 1.5 of 12 threads, ~12%).
        $stats | Add-Member loaded ($stats.cpu_mean -gt $MaxLoad + 15)
        $results += $stats
        $presentMonLine = if ($stats.presentmon_unusable) { 'PresentMon could not resolve presents' } else { '{0:N2} / {1:N2} / {2:N2} ms ({3} presents)' -f $stats.p50, $stats.p95, $stats.max, $stats.n }
        $appLine = if ($null -ne $stats.app_interval_p95) { 'app frame intervals p50/p95/max {0:N2} / {1:N2} / {2:N2} ms' -f $stats.app_interval_p50, $stats.app_interval_p95, $stats.app_interval_max } else { '' }
        '{0} run {1}: PresentMon p50/p95/max {2}; {3}; judged on {4}; app work p95 {5:N2} ms; CPU before {6:N0}% during {7:N0}%' -f $Label, $run, $presentMonLine, $appLine, $stats.judged_on, $stats.work_p95, $preLoad, $stats.cpu_mean
        Remove-Item $temp -Recurse -Force -ErrorAction SilentlyContinue
    } catch {
        # One failed run (a covered window, a crash) must not lose the others.
        Write-Warning "$Label run ${run}: $($_.Exception.Message)"
        $failed += [ordered]@{ run = $run; error = $_.Exception.Message }
        foreach ($process in $app, $load) { if ($process -and -not $process.HasExited) { Stop-Process -Id $process.Id -ErrorAction SilentlyContinue } }
        if ($capture -and -not $capture.HasExited) { Start-Process -FilePath $PresentMon -WindowStyle Hidden -Wait -ArgumentList '--terminate_existing_session', '--session_name', $session }
        if (-not (Test-Path (Join-Path $temp 'logs\desktop.log'))) { Remove-Item $temp -Recurse -Force -ErrorAction SilentlyContinue }
    } finally {
        $env:TJOMMIE_HOME = $savedHome
    }
}
# Judged on PresentMon where it worked, otherwise on the app's own frame intervals.
$pass = $results.Count -gt 0 -and $failed.Count -eq 0 -and -not ($results | Where-Object { $_.judged_p95 -gt 16.7 })
if ($results | Where-Object loaded) { Write-Warning "Some runs had heavy CPU load from other work during the run (loaded = true in summary.json)" }
[ordered]@{
    label = $Label; scenario = $Scenario; app_args = $AppArgs; pass_p95_le_16_7 = $pass; max_load = $MaxLoad
    display = "$($display.Name), $($display.CurrentHorizontalResolution)x$($display.CurrentVerticalResolution) @ $($display.CurrentRefreshRate) Hz"
    scaling = if ($dpi) { "$([math]::Round($dpi / 96 * 100))% (AppliedDPI $dpi)" } else { 'unknown' }
    measured = (Get-Date).ToString('s')
    runs = $results
    failed_runs = $failed
} | ConvertTo-Json -Depth 4 | Set-Content -Encoding utf8 (Join-Path $evidence 'summary.json')
'{0} pass (p95 <= 16.7 ms on every run): {1} ({2} runs measured, {3} failed)' -f $Label, $pass, $results.Count, $failed.Count
