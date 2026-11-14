<#
Criterion 8 and the owner's two motion measurements, for the spike's Aegis motions.

    .\scripts\run-motion-checks.ps1              # everything (about 30 minutes on a quiet PC)
    .\scripts\run-motion-checks.ps1 -Only p95    # just the timed runs (about 25 minutes)
    .\scripts\run-motion-checks.ps1 -Only p95 -Labels motion-approve,living-30   # some of them

Writes evidence\motion\results.jsonl (pivot, keyboard, interruption, lag, simulated reduced motion) and
evidence\motion-<name>\ (PresentMon p95 per motion, 3 runs of 50 loops each, via run-scenario.ps1).
Timed runs wait for a quiet PC: up to QuietTimeout s per motion in total, then they run anyway and
record their load. Build first: cargo build --release.
#>
param([string]$Only = '', [string[]]$Labels = @(), [int]$Runs = 3, [double]$MaxLoad = 35, [int]$QuietTimeout = 1200)
$ErrorActionPreference = 'Stop'
$crate = Split-Path $PSScriptRoot -Parent
$exe = Join-Path $crate 'target\release\tjommie-spike.exe'
$evidence = Join-Path $crate 'evidence\motion'
New-Item -ItemType Directory -Force $evidence | Out-Null
$results = Join-Path $evidence 'results.jsonl'
$counter = '\Processor(_Total)\% Processor Time'

function Wait-Quiet {
    $deadline = (Get-Date).AddSeconds($QuietTimeout)
    while ($true) {
        $mean = ((Get-Counter $counter -SampleInterval 1 -MaxSamples 5).CounterSamples.CookedValue | Measure-Object -Average).Average
        if ($mean -le $MaxLoad) { return [math]::Round($mean, 1) }
        if ((Get-Date) -gt $deadline) { throw "CPU never fell to $MaxLoad% within $QuietTimeout s" }
        Write-Host ('waiting for a quiet machine: CPU {0:N0}% > {1}%' -f $mean, $MaxLoad)
    }
}

function Invoke-Spike([string[]]$Arguments) {
    $app = Start-Process -FilePath $exe -PassThru -ArgumentList ($Arguments + @('--out', $results, '--shots', $evidence))
    if (-not $app.WaitForExit(300000)) { Stop-Process -Id $app.Id; throw "spike timed out: $Arguments" }
}

if (-not $Only -or $Only -eq 'checks') {
    if (-not $Only) { Remove-Item $results -ErrorAction SilentlyContinue }
    Write-Host '== eyes pivot'; Invoke-Spike @('--scenario', 'eyes-pivot')
    Write-Host '== keyboard'; Invoke-Spike @('--scenario', 'motion-keyboard')
    Write-Host '== reduced motion, simulated (--force-reduced; the real check is the owner''s)'; Invoke-Spike @('--scenario', 'motion-reduced', '--force-reduced')
    Write-Host "== interruption (CPU before: $(Wait-Quiet)%)"; Invoke-Spike @('--scenario', 'motion-interrupt')
    Write-Host "== panel lag (CPU before: $(Wait-Quiet)%)"; Invoke-Spike @('--scenario', 'motion-lag')
    Write-Host '== flight shots'; Invoke-Spike @('--scenario', 'flight-shots')
}
if ($Only -eq 'rerun') {
    Write-Host "== interruption (CPU before: $(Wait-Quiet)%)"; Invoke-Spike @('--scenario', 'motion-interrupt')
    Write-Host '== flight shots'; Invoke-Spike @('--scenario', 'flight-shots')
}

if (-not $Only -or $Only -eq 'p95') {
    $runScenario = Join-Path $PSScriptRoot 'run-scenario.ps1'
    $timed = @(
        @{ Label = 'motion-panel-snapshot'; Scenario = 'motion-panel'; AppArgs = @() },
        @{ Label = 'motion-panel-live'; Scenario = 'motion-panel'; AppArgs = @('--panel-live') },
        @{ Label = 'motion-recents'; Scenario = 'motion-recents'; AppArgs = @() },
        @{ Label = 'motion-approve'; Scenario = 'motion-approve'; AppArgs = @() },
        @{ Label = 'motion-flight'; Scenario = 'motion-flight'; AppArgs = @() },
        # The flight on its own (with the browser, every flight also swaps the bots' WebView2 pages).
        @{ Label = 'motion-flight-nobrowser'; Scenario = 'motion-flight'; AppArgs = @('--no-browser') },
        @{ Label = 'living-30'; Scenario = 'living'; AppArgs = @('--seconds', '20') }
    )
    foreach ($job in $timed | Where-Object { -not $Labels -or $Labels -contains $_.Label }) {
        Write-Host "== $($job.Label) ($(Get-Date -Format 'HH:mm:ss'))"
        # One motion's failure must not stop the others.
        try { & $runScenario -Scenario $job.Scenario -Label $job.Label -Runs $Runs -MaxLoad $MaxLoad -QuietTimeout $QuietTimeout -AppArgs $job.AppArgs }
        catch { Write-Warning "$($job.Label): $($_.Exception.Message)" }
    }
}
if (Test-Path $results) { Get-Content $results | ForEach-Object { $_ -replace ',"shot":"[^"]*"', '' -replace ',"frames":\{.*\}\}', '}' } }
