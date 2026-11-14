<#
Runs one spike scenario several times under PresentMon and prints msBetweenPresents p50/p95/max per run.

    .\scripts\run-scenario.ps1 -Scenario scroll                       # 3 runs, label "scroll"
    .\scripts\run-scenario.ps1 -Scenario scroll -Label scroll-150pct  # same, filed under another label
    .\scripts\run-scenario.ps1 -Scenario scroll -AppArgs '--speed','12000'

Build first: cargo build --release (in tjommie-rs\spikes\gpui).
Raw captures go to %LOCALAPPDATA%\Tjommie-spike\runs\<label>\; trimmed in-window CSVs, markers and
summary.json go to evidence\<label>\. PresentMon runs hidden; the spike window opens without focus.
#>
param(
    [string]$Scenario = 'scroll',
    [int]$Runs = 3,
    [string]$Label = $Scenario,
    [string[]]$AppArgs = @(),
    # The owner's PC is shared with other agents' builds. Each run waits until system CPU averages at
    # most MaxLoad % over 5 s and the spike's window can be drawn (covered.ps1). QuietTimeout is the total wait for
    # all runs of this call: once it is spent, runs start anyway and record their load (a run whose
    # window could not be drawn is skipped).
    [double]$MaxLoad = 35,
    [int]$QuietTimeout = 1200
)
$ErrorActionPreference = 'Stop'
$crate = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $crate 'target\release\tjommie-spike.exe'
$root = Join-Path $env:LOCALAPPDATA 'Tjommie-spike'
$presentMon = Join-Path $root 'tools\presentmon\PresentMon-2.6.0-x64.exe'
$raw = Join-Path $root "runs\$Label"
$evidence = Join-Path $crate "evidence\$Label"
foreach ($tool in $exe, $presentMon) { if (-not (Test-Path $tool)) { throw "Missing $tool" } }
New-Item -ItemType Directory -Force $raw, $evidence | Out-Null
. (Join-Path $PSScriptRoot 'covered.ps1')

$display = Get-CimInstance Win32_VideoController | Select-Object -First 1
$dpi = (Get-ItemProperty 'HKCU:\Control Panel\Desktop\WindowMetrics' -Name AppliedDPI -ErrorAction SilentlyContinue).AppliedDPI
$session = 'TjommieSpike'
$counter = '\Processor(_Total)\% Processor Time'
$quietDeadline = (Get-Date).AddSeconds($QuietTimeout)
function Wait-Quiet {
    while ($true) {
        $mean = ((Get-Counter $counter -SampleInterval 1 -MaxSamples 5).CounterSamples.CookedValue | Measure-Object -Average).Average
        $covered = Test-Covered
        if ($mean -le $MaxLoad -and -not $covered) { return [math]::Round($mean, 1) }
        if ((Get-Date) -gt $quietDeadline) {
            if ($covered) { throw 'the spike window would not be drawn (a full-screen app in front, or the screen is locked)' }
            Write-Warning ('{0}: no quiet moment within its {1} s budget; running at CPU {2:N0}%, and the load is recorded' -f $Label, $QuietTimeout, $mean)
            return [math]::Round($mean, 1)
        }
        if ($covered) { Write-Host 'waiting: a full-screen app is in front or the screen is locked, and the spike window would not be drawn' }
        else { Write-Host ('waiting for a quiet machine: CPU {0:N0}% > {1}%' -f $mean, $MaxLoad) }
    }
}
$results = @()
$failed = @()
for ($run = 1; $run -le $Runs; $run++) {
    $csv = Join-Path $raw "run$run-presentmon.csv"
    $markers = Join-Path $raw "run$run-markers.csv"
    $trace = Join-Path $raw "run$run-trace.csv"
    $cpu = Join-Path $raw "run$run-cpu.csv"
    Remove-Item $csv, $markers, $trace, $cpu, ($markers -replace '\.csv$', '.windows.csv') -ErrorAction SilentlyContinue
    $capture = $load = $app = $null
    try {
        $preLoad = Wait-Quiet
        $capture = Start-Process -FilePath $presentMon -WindowStyle Hidden -PassThru -ArgumentList @(
            '--process_name', 'tjommie-spike.exe', '--output_file', $csv, '--v1_metrics', '--qpc_time',
            '--stop_existing_session', '--session_name', $session, '--no_console_stats')
        Start-Sleep -Seconds 2
        # System-wide CPU load for the whole run (stopped when the app exits): the PC is shared.
        $load = Start-Process -FilePath typeperf -WindowStyle Hidden -PassThru -ArgumentList '"\Processor(_Total)\% Processor Time"', '-si', '1', '-sc', '240', '-f', 'CSV', '-y', '-o', $cpu
        $app = Start-Process -FilePath $exe -PassThru -ArgumentList (@('--scenario', $Scenario, '--markers', $markers, '--trace', $trace, '--shots', $raw) + $AppArgs)
        if (-not $app.WaitForExit(180000)) { throw "run $run timed out" }
        Stop-Process -Id $load.Id -ErrorAction SilentlyContinue
        Start-Sleep -Seconds 1
        Start-Process -FilePath $presentMon -WindowStyle Hidden -Wait -ArgumentList '--terminate_existing_session', '--session_name', $session
        if (-not $capture.WaitForExit(15000)) { Stop-Process -Id $capture.Id; Write-Warning "PresentMon run $run had to be killed" }
        if ($app.ExitCode -ne 0 -or -not (Test-Path $markers)) { throw "run $run failed (exit $($app.ExitCode)); see $root\logs\spike.log" }
        Copy-Item $markers, $trace, $cpu -Destination $evidence -ErrorAction SilentlyContinue
        $windows = $markers -replace '\.csv$', '.windows.csv'
        $statsArgs = @($csv, $markers, (Join-Path $evidence "run$run.csv"), $trace, $cpu)
        if (Test-Path $windows) { $statsArgs += $windows; Copy-Item $windows $evidence }
        $stats = python -I (Join-Path $PSScriptRoot 'frame-stats.py') @statsArgs | ConvertFrom-Json
        if ($LASTEXITCODE -ne 0 -or ($null -eq $stats.p95 -and $null -eq $stats.app_interval_p95)) { throw "frame-stats failed for run $run" }
        $stats | Add-Member run $run
        $stats | Add-Member pre_cpu $preLoad
        # In-run load includes the spike and PresentMon themselves (about 1.5 of 12 threads, ~12%).
        $stats | Add-Member loaded ($stats.cpu_mean -gt $MaxLoad + 15)
        $results += $stats
        $presentMonLine = if ($stats.presentmon_unusable) { 'PresentMon could not resolve presents' } else { '{0:N2} / {1:N2} / {2:N2} ms ({3} presents)' -f $stats.p50, $stats.p95, $stats.max, $stats.n }
        $appLine = if ($null -ne $stats.app_interval_p95) { 'app frame intervals p50/p95/max {0:N2} / {1:N2} / {2:N2} ms' -f $stats.app_interval_p50, $stats.app_interval_p95, $stats.app_interval_max } else { '' }
        '{0} run {1}: PresentMon p50/p95/max {2}; {3}; app work p95 {4:N2} ms; CPU before {5:N0}% during {6:N0}%' -f $Label, $run, $presentMonLine, $appLine, $stats.work_p95, $preLoad, $stats.cpu_mean
    } catch {
        # One failed run (a covered window, a crash) must not lose the others.
        Write-Warning "$Label run ${run}: $($_.Exception.Message)"
        $failed += [ordered]@{ run = $run; error = $_.Exception.Message }
        foreach ($process in $app, $load) { if ($process -and -not $process.HasExited) { Stop-Process -Id $process.Id -ErrorAction SilentlyContinue } }
        if ($capture -and -not $capture.HasExited) { Start-Process -FilePath $presentMon -WindowStyle Hidden -Wait -ArgumentList '--terminate_existing_session', '--session_name', $session }
    }
}
# Judged on PresentMon where it worked, otherwise on the app's own frame intervals.
$pass = $results.Count -gt 0 -and -not ($results | Where-Object { $(if ($_.presentmon_unusable) { $_.app_interval_p95 } else { $_.p95 }) -gt 16.7 })
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
