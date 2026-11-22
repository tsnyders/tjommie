<#
Plan T7.4: cold start of tjommie-desktop.exe, launch to the first interactive frame. Reports both:
  - cold_start_to_first_present_ms: launch to the first present PresentMon sees (as the spike and the 3,837.5 ms
    baseline of the installed 0.4.2.0 measured it);
  - cold_start_to_ui_ready_ms: launch to the log marker "ui ready" in <root>\logs\desktop.log (the shell's first frame is
    presented and the daemon is connected), polled every 20 ms.

    .\tools\perf\cold-start.ps1            # 3 launches, one after another, each on a fresh TJOMMIE_HOME under %TEMP%
    .\tools\perf\cold-start.ps1 -Runs 1

Each launch starts the app normally (no scenario), with TJOMMIE_HOME on a fresh folder and TJOMMIE_DAEMON_EXE on
target\perf\tjommie-daemon.exe, so it starts its own daemon on the dev ports 47200 and 47201 (nothing else may hold
them) and never touches an installed Tjommie. The app is stopped after each launch (only the process this script
started, and its children). Run it on a quiet PC: it waits for CPU at most MaxLoad % over 5 s first.
Writes evidence\cold-start\summary.json and prints each run.
#>
param(
    [int]$Runs = 3,
    [string]$Exe = '',
    [string]$DaemonExe = '',
    [string]$PresentMon = '',
    [double]$MaxLoad = 35,
    [int]$QuietTimeout = 600,
    [int]$ReadyTimeoutSeconds = 60
)
$ErrorActionPreference = 'Stop'
$crate = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
if (-not $Exe) { $Exe = Join-Path $crate 'target\perf\tjommie-desktop.exe' }
if (-not $DaemonExe) { $DaemonExe = Join-Path $crate 'target\perf\tjommie-daemon.exe' }
$root = Join-Path $env:LOCALAPPDATA 'Tjommie-rs\perf'
if (-not $PresentMon) { $PresentMon = (Get-ChildItem (Join-Path $root 'tools\presentmon') -Filter 'PresentMon*.exe' -ErrorAction SilentlyContinue | Select-Object -First 1).FullName }
foreach ($tool in $Exe, $DaemonExe, $PresentMon) { if (-not $tool -or -not (Test-Path $tool)) { throw "Missing '$tool'" } }
$processName = Split-Path $Exe -Leaf
$evidence = Join-Path $crate 'evidence\cold-start'
$raw = Join-Path $root 'runs\cold-start'
New-Item -ItemType Directory -Force $evidence, $raw | Out-Null
. (Join-Path $PSScriptRoot 'covered.ps1')
$session = 'TjommieDesktopPerf'
$counter = '\Processor(_Total)\% Processor Time'
$freq = [System.Diagnostics.Stopwatch]::Frequency   # QPC ticks per second, the clock PresentMon stamps with

$results = @()
$savedHome = $env:TJOMMIE_HOME
$savedDaemon = $env:TJOMMIE_DAEMON_EXE
for ($run = 1; $run -le $Runs; $run++) {
    $deadline = (Get-Date).AddSeconds($QuietTimeout)
    while ($true) {
        $mean = ((Get-Counter $counter -SampleInterval 1 -MaxSamples 5).CounterSamples.CookedValue | Measure-Object -Average).Average
        if (($mean -le $MaxLoad -and -not (Test-Covered)) -or (Get-Date) -gt $deadline) { break }
        Write-Host ('waiting for a quiet machine (or an undisturbed screen): CPU {0:N0}%' -f $mean)
    }
    $temp = Join-Path ([IO.Path]::GetTempPath()) ('tjommie-perf-' + [guid]::NewGuid().ToString('N').Substring(0, 12))
    New-Item -ItemType Directory -Force $temp | Out-Null
    $csv = Join-Path $raw "run$run-presentmon.csv"
    Remove-Item $csv -ErrorAction SilentlyContinue
    $capture = $app = $null
    try {
        $capture = Start-Process -FilePath $PresentMon -WindowStyle Hidden -PassThru -ArgumentList @(
            '--process_name', $processName, '--output_file', $csv, '--v1_metrics', '--qpc_time',
            '--stop_existing_session', '--session_name', $session, '--no_console_stats')
        Start-Sleep -Seconds 2
        $env:TJOMMIE_HOME = $temp
        $env:TJOMMIE_DAEMON_EXE = $DaemonExe
        $launched = [System.Diagnostics.Stopwatch]::GetTimestamp()
        $app = Start-Process -FilePath $Exe -PassThru
        $log = Join-Path $temp 'logs\desktop.log'
        $readyMs = $null
        while (-not $app.HasExited -and ([System.Diagnostics.Stopwatch]::GetTimestamp() - $launched) / $freq -lt $ReadyTimeoutSeconds) {
            if ((Test-Path $log) -and (Select-String -Path $log -SimpleMatch 'ui ready' -Quiet -ErrorAction SilentlyContinue)) {
                $readyMs = ([System.Diagnostics.Stopwatch]::GetTimestamp() - $launched) * 1000 / $freq
                break
            }
            Start-Sleep -Milliseconds 20
        }
        Start-Sleep -Seconds 2   # let PresentMon see a few presents
        Start-Process -FilePath $PresentMon -WindowStyle Hidden -Wait -ArgumentList '--terminate_existing_session', '--session_name', $session
        if (-not $capture.WaitForExit(15000)) { Stop-Process -Id $capture.Id }
        # PresentMon writes no row for a swap chain's first present: every row's msBetweenPresents reaches back to the
        # present before it, so the first present is the earliest (QPCTime - msBetweenPresents).
        $first = if (Test-Path $csv) {
            Import-Csv $csv | Where-Object { $_.QPCTime -and $_.msBetweenPresents } |
                ForEach-Object { [int64]$_.QPCTime - [double]$_.msBetweenPresents * $freq / 1000 } | Measure-Object -Minimum
        }
        $presentMs = if ($first -and $first.Count) { ($first.Minimum - $launched) * 1000 / $freq } else { $null }
        $row = [ordered]@{
            run = $run; cold_start_to_first_present_ms = if ($null -ne $presentMs) { [math]::Round($presentMs, 1) } else { $null }
            cold_start_to_ui_ready_ms = if ($null -ne $readyMs) { [math]::Round($readyMs, 1) } else { $null }
            cpu_before = [math]::Round($mean, 1)
        }
        $results += $row
        'run {0}: first present {1} ms, ui ready {2} ms (CPU before {3:N0}%)' -f $run, $row.cold_start_to_first_present_ms, $row.cold_start_to_ui_ready_ms, $mean
        if ($null -eq $readyMs) { Write-Warning "no 'ui ready' within $ReadyTimeoutSeconds s; the log is $log (kept)"; $temp = $null }
    } finally {
        $env:TJOMMIE_HOME = $savedHome
        $env:TJOMMIE_DAEMON_EXE = $savedDaemon
        if ($app -and -not $app.HasExited) { & taskkill.exe /PID $app.Id /T /F 2>&1 | Out-Null }   # only what this run started
        if ($capture -and -not $capture.HasExited) { Stop-Process -Id $capture.Id -ErrorAction SilentlyContinue }
        Start-Sleep -Seconds 2
        if ($temp) { Remove-Item $temp -Recurse -Force -ErrorAction SilentlyContinue }
    }
}
[ordered]@{
    measured = (Get-Date).ToString('s'); baseline_first_present_ms = 3837.5; runs = $results
    median_first_present_ms = ($results | Where-Object { $null -ne $_.cold_start_to_first_present_ms } | ForEach-Object { $_.cold_start_to_first_present_ms } | Sort-Object | Select-Object -Skip ([math]::Floor(($results.Count - 1) / 2)) -First 1)
    median_ui_ready_ms = ($results | Where-Object { $null -ne $_.cold_start_to_ui_ready_ms } | ForEach-Object { $_.cold_start_to_ui_ready_ms } | Sort-Object | Select-Object -Skip ([math]::Floor(($results.Count - 1) / 2)) -First 1)
} | ConvertTo-Json -Depth 4 | Set-Content -Encoding utf8 (Join-Path $evidence 'summary.json')
Write-Host "Saved $(Join-Path $evidence 'summary.json') (baseline: 3,837.5 ms to the first present)"
