<#
Plan T7.2 and T7.4: every timed motion (3 runs of 50 loops, PresentMon p95 at most 16.7 ms) and the three checks
(keyboard, simulated reduced motion, and idle: a window not in front draws at most 1 frame a second), for
tjommie-desktop.exe.

    .\tools\perf\run-motion-checks.ps1                       # everything (about 40 minutes on a quiet PC)
    .\tools\perf\run-motion-checks.ps1 -Only p95             # just the timed runs
    .\tools\perf\run-motion-checks.ps1 -Only checks          # just the three checks (no timing)
    .\tools\perf\run-motion-checks.ps1 -Only p95 -Labels motion-flight,motion-alive    # some of them
    .\tools\perf\run-motion-checks.ps1 -Smoke -Labels motion-flight,motion-toast      # a quick proof

The list of motions is tjommie-rs\motion-map.md; $timed below must match it (add a line when a motion is added).
-Smoke: one run of 5 loops (motion-alive: 5 s) per named motion (all when no -Labels), filed under
evidence\smoke-<name>\ so real evidence is never overwritten, no checks, no verdict: it proves the plumbing only.

Writes evidence\motion\results.jsonl (one JSON line per check: keyboard, reduced, idle) and
evidence\<name>\ (PresentMon p95 per motion, via run-scenario.ps1). Timed runs wait for a quiet PC: up to
QuietTimeout s per motion in total, then they run anyway and record their load. Each app run gets a fresh
TJOMMIE_HOME under %TEMP% (deleted after). Build first, see run-scenario.ps1.
The real reduced-motion check (Windows' Animation effects off) is the owner's by hand: owner-perf-run.md.
Exit code: 0 when every timed motion passed and every check passed, else 1.
#>
param(
    [ValidateSet('', 'p95', 'checks')][string]$Only = '',
    [string[]]$Labels = @(),
    [int]$Runs = 3,
    [switch]$Smoke,
    [double]$MaxLoad = 35,
    [int]$QuietTimeout = 1200,
    [string]$Exe = ''
)
$ErrorActionPreference = 'Stop'
$crate = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent   # tjommie-rs
if (-not $Exe) { $Exe = Join-Path $crate 'target\perf\tjommie-desktop.exe' }
if (-not (Test-Path $Exe)) { throw "Missing $Exe (cargo build --profile perf -p desktop --features test-driver --locked, inside tjommie-rs)" }
$evidence = Join-Path $crate 'evidence\motion'
$results = Join-Path $evidence 'results.jsonl'
$counter = '\Processor(_Total)\% Processor Time'
$problems = @()

function Wait-Quiet {
    $deadline = (Get-Date).AddSeconds($QuietTimeout)
    while ($true) {
        $mean = ((Get-Counter $counter -SampleInterval 1 -MaxSamples 5).CounterSamples.CookedValue | Measure-Object -Average).Average
        if ($mean -le $MaxLoad) { return [math]::Round($mean, 1) }
        # The checks judge state, not frame times: a busy PC slows them but can't fail them, so they run anyway.
        if ((Get-Date) -gt $deadline) { Write-Warning ('CPU never fell to {0}% within {1} s; running at {2:N0}%' -f $MaxLoad, $QuietTimeout, $mean); return [math]::Round($mean, 1) }
        Write-Host ('waiting for a quiet machine: CPU {0:N0}% > {1}%' -f $mean, $MaxLoad)
    }
}

# One check: the app runs the scenario, appends its JSON lines to results.jsonl and exits 0 only when it passed.
function Invoke-Check([string[]]$Arguments) {
    $temp = Join-Path ([IO.Path]::GetTempPath()) ('tjommie-perf-' + [guid]::NewGuid().ToString('N').Substring(0, 12))
    $saved = $env:TJOMMIE_HOME
    New-Item -ItemType Directory -Force $temp | Out-Null
    $env:TJOMMIE_HOME = $temp
    try {
        $app = Start-Process -FilePath $Exe -PassThru -ArgumentList ($Arguments + @('--out', $results))
        if (-not $app.WaitForExit(300000)) { Stop-Process -Id $app.Id; throw "timed out: $Arguments" }
        if ($app.ExitCode -ne 0) { throw "failed (exit $($app.ExitCode)): $Arguments; see $temp\logs\desktop.log" }
        Remove-Item $temp -Recurse -Force -ErrorAction SilentlyContinue
    } finally { $env:TJOMMIE_HOME = $saved }
}

# Interruption (the spike report's two rules) is proven by unit tests per motion (motion-map.md); no app scenario.
$checks = @(
    @{ Label = 'motion-keyboard'; Note = 'keyboard: key-triggered motions land with no frames in between'; Args = @('--scenario', 'motion-keyboard') },
    @{ Label = 'motion-reduced'; Note = 'reduced motion, simulated (--force-reduced; the real check is the owner''s)'; Args = @('--scenario', 'motion-reduced', '--force-reduced') },
    @{ Label = 'motion-idle'; Note = 'idle: one agent at work, the window not in front draws at most 1 frame a second (its in-front rate beside it; about 45 s)'; Args = @('--scenario', 'motion-idle', '--seconds', '20') }
)
if (-not $Smoke -and (-not $Only -or $Only -eq 'checks')) {
    New-Item -ItemType Directory -Force $evidence | Out-Null
    if (-not $Only -and -not $Labels) { Remove-Item $results -ErrorAction SilentlyContinue }
    foreach ($check in $checks | Where-Object { -not $Labels -or $Labels -contains $_.Label }) {
        Write-Host "== $($check.Note) (CPU before: $(Wait-Quiet)%)"
        try { Invoke-Check $check.Args } catch { Write-Warning "$($check.Label): $($_.Exception.Message)"; $problems += $check.Label }
    }
}

if ($Smoke -or -not $Only -or $Only -eq 'p95') {
    $runScenario = Join-Path $PSScriptRoot 'run-scenario.ps1'
    # The app's timed scenarios (crates/desktop/src/scenario.rs TIMED, plus motion-alive and scroll-10k).
    $timed = @(
        'motion-recents', 'motion-flight', 'motion-toast', 'motion-panel', 'motion-numbers', 'motion-arrival',
        'motion-alive', 'scroll-10k'
    )
    foreach ($name in $timed | Where-Object { -not $Labels -or $Labels -contains $_ }) {
        Write-Host "== $name ($(Get-Date -Format 'HH:mm:ss'))"
        $extra = @{}
        if ($name -eq 'motion-alive') { $extra.Seconds = if ($Smoke) { 5 } else { 20 } }
        if ($Smoke) { $extra.Loops = 5; $extra.Runs = 1; $extra.Label = "smoke-$name" } else { $extra.Runs = $Runs; $extra.Label = $name }
        # One motion's failure must not stop the others.
        try {
            $line = & $runScenario -Scenario $name -MaxLoad $MaxLoad -QuietTimeout $QuietTimeout -Exe $Exe @extra
            $line
            if (-not $Smoke -and "$($line | Select-Object -Last 1)" -notmatch 'pass \(p95 <= 16\.7 ms on every run\): True') { $problems += $name }
        } catch { Write-Warning "${name}: $($_.Exception.Message)"; $problems += $name }
    }
}
if (Test-Path $results) {
    Get-Content $results | ForEach-Object { $_ }
    $bad = @(Get-Content $results | ForEach-Object { try { $_ | ConvertFrom-Json } catch { } } | Where-Object { $_.pass -eq $false })
    foreach ($b in $bad) { $problems += "$($b.scenario)/$($b.motion)" }
}
if ($Smoke) { Write-Host 'smoke: the plumbing ran; nothing here is a verdict'; if ($problems) { Write-Warning ('smoke problems: ' + ($problems -join ', ')); exit 1 }; exit 0 }
if ($problems) { Write-Warning ('NOT PASSING: ' + (($problems | Select-Object -Unique) -join ', ')); exit 1 }
Write-Host 'all motions and checks passed'
exit 0
