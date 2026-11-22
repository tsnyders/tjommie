<#
T7.3: does MMCSS or ABOVE_NORMAL priority help the panel and Recents motions under heavy background load?

    .\tools\perf\t73-experiment.ps1                    # 3 runs per cell; about 45 minutes; puts load on the PC
    .\tools\perf\t73-experiment.ps1 -LoadThreads 10    # tune load.ps1's thread count to reach 60 to 80% CPU

Options (app flags, passed as -AppArgs to run-scenario.ps1): none; --mmcss (AvSetMmThreadCharacteristicsW "Games");
--above-normal (SetThreadPriority ABOVE_NORMAL). Scenarios: motion-panel and motion-recents.
For each option x scenario it measures 3 runs with load.ps1 running (labels t73-<scenario>-<option>-load) and, for the
idle-CPU check, 3 runs on a quiet PC (t73-<scenario>-<option>-quiet), all with run-scenario.ps1 (a fresh
TJOMMIE_HOME each; the app runs on fixtures). Evidence: evidence\t73\decision.json plus each label's folder.

Keep rule (plan): an option stays only if its p95 improves by at least 2 ms on all 3 runs with no worse idle CPU.
Decided conservatively, whatever the pairing of runs: the option's worst loaded run is at least 2 ms better than
"none"'s best loaded run, for BOTH scenarios. "No worse idle CPU": the option's mean system CPU in the quiet runs
(the run's own CPU, a proxy for the app's idle cost) is at most 1 point above "none"'s. Loaded runs outside
60 to 80% system CPU are flagged and make the verdict "not conclusive". The result is reported either way.
Never run by CI.
#>
param(
    [int]$LoadThreads = 0,
    [int]$Runs = 3,
    [string]$Exe = ''
)
$ErrorActionPreference = 'Stop'
$crate = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$runScenario = Join-Path $PSScriptRoot 'run-scenario.ps1'
$load = Join-Path $PSScriptRoot 'load.ps1'
$out = Join-Path $crate 'evidence\t73'
New-Item -ItemType Directory -Force $out | Out-Null
$options = [ordered]@{ none = @(); mmcss = @('--mmcss'); above_normal = @('--above-normal') }
$scenarios = 'motion-panel', 'motion-recents'
$exeArgs = if ($Exe) { @{ Exe = $Exe } } else { @{} }

function Read-Summary([string]$Label) {
    Get-Content (Join-Path $crate "evidence\$Label\summary.json") -Raw | ConvertFrom-Json
}

# Quiet cells first (no load running), then loaded cells with load.ps1 in a hidden process.
foreach ($scenario in $scenarios) {
    foreach ($name in $options.Keys) {
        & $runScenario -Scenario $scenario -Label "t73-$scenario-$name-quiet" -Runs $Runs -AppArgs $options[$name] @exeArgs | Select-Object -Last 1
    }
}
$loadArgs = @('-NoProfile', '-File', $load, '-Seconds', '7200')
if ($LoadThreads -gt 0) { $loadArgs += '-Threads', $LoadThreads }
$loader = Start-Process -FilePath (Get-Process -Id $PID).Path -ArgumentList $loadArgs -WindowStyle Hidden -PassThru
try {
    Start-Sleep -Seconds 8   # the load ramps up
    foreach ($scenario in $scenarios) {
        foreach ($name in $options.Keys) {
            # MaxLoad 100: the load is deliberate, so no waiting for quiet; the covered-screen gate still applies.
            & $runScenario -Scenario $scenario -Label "t73-$scenario-$name-load" -Runs $Runs -MaxLoad 100 -AppArgs $options[$name] @exeArgs | Select-Object -Last 1
        }
    }
} finally {
    if (-not $loader.HasExited) { Stop-Process -Id $loader.Id -Force -ErrorAction SilentlyContinue }
}

$decision = [ordered]@{ measured = (Get-Date).ToString('s'); load_threads = $LoadThreads; rule = 'worst loaded run <= none best loaded run - 2 ms, for both scenarios; quiet mean CPU <= none + 1'; options = [ordered]@{} }
$conclusive = $true
foreach ($name in $options.Keys | Where-Object { $_ -ne 'none' }) {
    $keep = $true
    $detail = [ordered]@{}
    foreach ($scenario in $scenarios) {
        $base = Read-Summary "t73-$scenario-none-load"
        $opt = Read-Summary "t73-$scenario-$name-load"
        $baseQuiet = Read-Summary "t73-$scenario-none-quiet"
        $optQuiet = Read-Summary "t73-$scenario-$name-quiet"
        $bestNone = ($base.runs | ForEach-Object { $_.judged_p95 } | Measure-Object -Minimum).Minimum
        $worstOpt = ($opt.runs | ForEach-Object { $_.judged_p95 } | Measure-Object -Maximum).Maximum
        $cpuNone = ($baseQuiet.runs | ForEach-Object { $_.cpu_mean } | Measure-Object -Average).Average
        $cpuOpt = ($optQuiet.runs | ForEach-Object { $_.cpu_mean } | Measure-Object -Average).Average
        $loads = @($base.runs) + @($opt.runs) | ForEach-Object { $_.cpu_mean }
        $outside = @($loads | Where-Object { $_ -lt 60 -or $_ -gt 80 }).Count
        if ($outside -or @($base.runs).Count -lt $Runs -or @($opt.runs).Count -lt $Runs) { $conclusive = $false }
        $improves = $null -ne $worstOpt -and $null -ne $bestNone -and $worstOpt -le $bestNone - 2
        $idleOk = $cpuOpt -le $cpuNone + 1
        if (-not ($improves -and $idleOk)) { $keep = $false }
        $detail[$scenario] = [ordered]@{
            none_loaded_p95 = @($base.runs | ForEach-Object { $_.judged_p95 }); option_loaded_p95 = @($opt.runs | ForEach-Object { $_.judged_p95 })
            improves_2ms_on_all_runs = $improves; quiet_cpu_none = [math]::Round($cpuNone, 1); quiet_cpu_option = [math]::Round($cpuOpt, 1)
            idle_cpu_not_worse = $idleOk; loaded_runs_outside_60_80_percent = $outside
        }
    }
    $decision.options[$name] = [ordered]@{ keep = $keep; detail = $detail }
}
$decision.conclusive = $conclusive
$decision | ConvertTo-Json -Depth 6 | Set-Content -Encoding utf8 (Join-Path $out 'decision.json')
$decision.options.Keys | ForEach-Object { '{0}: keep = {1}' -f $_, $decision.options[$_].keep }
if (-not $conclusive) { Write-Warning 'Not conclusive: a loaded run missed the 60 to 80% CPU window, or a run failed. Adjust -LoadThreads and repeat.' }
'Saved ' + (Join-Path $out 'decision.json')
