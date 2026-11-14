<#
Memory of an app's whole process tree (Phase 3 plan, T0.3 and "Memory budget"), fixed as the WebView2 memory
research's Step 1: private working set is the primary metric, private commit the secondary one, and summed working
set is kept only for continuity with the 496.1 MiB baseline (it counts shared pages once per process).

    # Owner step D2: the installed production app, already running in state S0 (then again in S1):
    & "<repo>\tjommie-rs\tools\perf\measure-memory.ps1" -Target Electron -State S0
    # The desktop app (its root process):
    & "<repo>\tjommie-rs\tools\perf\measure-memory.ps1" -Target Desktop -State S1 -Pid <pid>
    # The tool's own check, on a hidden cmd.exe tree it starts (no CPU gate, no idle wait):
    & "<repo>\tjommie-rs\tools\perf\measure-memory.ps1" -SelfTest

The process set is the tree under the root process: its children, their children and so on (a child starts after its
parent, so an older process naming a reused id is not taken). It is never matched by name. With -Target Electron and
no -Pid, the root is the main process of %LOCALAPPDATA%\Programs\Tjommie\Tjommie.exe.

Gating, as the spike's baseline: wait up to QuietTimeout s for system CPU to average at most MaxLoad % over 5 s (then
measure anyway), let the app idle IdleSeconds while recording system CPU, then sample Samples times about 1 s apart.
The report is the median sample by private working set, so its totals are its per-process sums. The run is marked
loaded when CPU averaged above MaxLoad + 15 % while idling. Results go to %LOCALAPPDATA%\Tjommie-rs\perf\ (never the
production root); the app's data is not read.
#>
param(
    [ValidateSet('Electron', 'Desktop')][string]$Target = 'Desktop',
    [ValidateSet('S0', 'S1', 'S2', 'S3', 'S4')][string]$State = 'S0',
    # The plan calls it -Pid; the variable can't be $Pid, which is PowerShell's own process id.
    [Alias('Pid')][int]$RootPid = 0,
    [int]$IdleSeconds = 60,
    [int]$Samples = 10,
    [double]$MaxLoad = 35,
    [int]$QuietTimeout = 600,
    [switch]$SelfTest
)
$ErrorActionPreference = 'Stop'
$cpuCounter = '\Processor(_Total)\% Processor Time'

function Get-Load([int]$Seconds) {
    [math]::Round(((Get-Counter $cpuCounter -SampleInterval 1 -MaxSamples $Seconds).CounterSamples.CookedValue | Measure-Object -Average).Average, 1)
}

function Get-Tree([int]$Root) {
    $all = @(Get-CimInstance Win32_Process)
    $tree = [System.Collections.Generic.List[object]]::new()
    $tree.AddRange(@($all | Where-Object ProcessId -eq $Root))
    if ($tree.Count -eq 0) { throw "No process with id $Root." }
    for ($i = 0; $i -lt $tree.Count; $i++) {
        $parent = $tree[$i]
        $tree.AddRange(@($all | Where-Object { $_.ParentProcessId -eq $parent.ProcessId -and $_.ProcessId -ne $parent.ProcessId -and $_.CreationDate -ge $parent.CreationDate }))
    }
    $tree
}

function MiB([double]$Bytes) { [math]::Round($Bytes / 1MB, 2) }

function Measure-Once([int]$Root) {
    $tree = Get-Tree $Root
    $filter = ($tree | ForEach-Object { "IDProcess=$($_.ProcessId)" }) -join ' OR '
    $privateWs = @{}
    foreach ($row in Get-CimInstance Win32_PerfFormattedData_PerfProc_Process -Filter $filter) { $privateWs[[int]$row.IDProcess] = [double]$row.WorkingSetPrivate }
    $rows = @(foreach ($p in $tree) {
        $process = Get-Process -Id $p.ProcessId -ErrorAction SilentlyContinue
        if (-not $process -or -not $privateWs.ContainsKey([int]$p.ProcessId)) { continue }   # exited while sampling
        [ordered]@{
            pid = [int]$p.ProcessId; name = $p.Name
            private_working_set_mib = MiB $privateWs[[int]$p.ProcessId]
            private_commit_mib = MiB $process.PrivateMemorySize64
            working_set_mib = MiB $process.WorkingSet64
        }
    })
    $sample = [ordered]@{ processes = $rows }
    foreach ($field in 'private_working_set_mib', 'private_commit_mib', 'working_set_mib') {
        $sample[$field] = [math]::Round([double]($rows | ForEach-Object { $_[$field] } | Measure-Object -Sum).Sum, 2)
    }
    $sample
}

function Measure-Tree([int]$Root, [int]$Samples) {
    $taken = @(for ($i = 0; $i -lt $Samples; $i++) {
        $next = (Get-Date).AddSeconds(1)
        Measure-Once $Root
        $wait = ($next - (Get-Date)).TotalMilliseconds
        if ($i -lt $Samples - 1 -and $wait -gt 0) { Start-Sleep -Milliseconds $wait }
    })
    # The lower median of an even count, so the report is a sample that really happened.
    @($taken | Sort-Object { $_.private_working_set_mib })[[math]::Floor(($taken.Count - 1) / 2)]
}

if ($SelfTest) {
    # A hidden tree of our own: cmd.exe, PING.EXE and cmd's windowless conhost. Not notepad: on Windows 11 that is the
    # Store app, which hands off to the owner's open Notepad (one instance, restored tabs) outside the spawned tree.
    $info = [System.Diagnostics.ProcessStartInfo]::new("$env:SystemRoot\System32\cmd.exe", '/d /c ping -n 60 127.0.0.1 >nul')
    $info.CreateNoWindow = $true
    $info.UseShellExecute = $false
    $shell = [System.Diagnostics.Process]::Start($info)
    try {
        Start-Sleep -Seconds 2   # ping starts and the performance counters see it
        $m = Measure-Tree -Root $shell.Id -Samples 3
        $failures = @()
        $names = @($m.processes | ForEach-Object { $_.name })
        if ($names -notcontains 'cmd.exe' -or $names -notcontains 'PING.EXE') { $failures += "the tree should hold cmd.exe and PING.EXE, got: $($names -join ', ')" }
        foreach ($p in $m.processes) {
            if (-not ($p.private_working_set_mib -gt 0 -and $p.private_working_set_mib -le $p.working_set_mib)) { $failures += "$($p.name) $($p.pid): private WS $($p.private_working_set_mib) MiB is not in (0, WS $($p.working_set_mib) MiB]" }
            if (-not ($p.private_commit_mib -gt 0)) { $failures += "$($p.name) $($p.pid): private commit $($p.private_commit_mib) MiB" }
        }
        foreach ($field in 'private_working_set_mib', 'private_commit_mib', 'working_set_mib') {
            $sum = ($m.processes | ForEach-Object { $_[$field] } | Measure-Object -Sum).Sum
            if ([math]::Abs($sum - $m[$field]) -gt 0.01) { $failures += "total $field $($m[$field]) is not the per-process sum $sum" }
        }
        if ($failures) { $failures | ForEach-Object { Write-Host "FAIL: $_" }; exit 1 }
        Write-Host "PASS: $($m.processes.Count) processes ($($names -join ', ')), private WS $($m.private_working_set_mib) MiB, private commit $($m.private_commit_mib) MiB, WS $($m.working_set_mib) MiB"
        exit 0
    } finally {
        $ids = @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($shell.Id)" | ForEach-Object ProcessId) + $shell.Id
        $ids | ForEach-Object { Stop-Process -Id $_ -Force -ErrorAction SilentlyContinue }
    }
}

if (-not $RootPid -and $Target -eq 'Electron') {
    $installed = Join-Path $env:LOCALAPPDATA 'Programs\Tjommie\Tjommie.exe'
    $app = @(Get-CimInstance Win32_Process -Filter "Name='Tjommie.exe'" | Where-Object ExecutablePath -eq $installed)
    $main = @($app | Where-Object { ($app | ForEach-Object ProcessId) -notcontains $_.ParentProcessId })
    if ($main.Count -ne 1) { throw "Start the installed Tjommie ($installed) first; found $($main.Count) main processes." }
    $RootPid = $main[0].ProcessId
}
if (-not $RootPid) { throw 'Give the root process: -Pid <id>.' }
$rootProcess = Get-Process -Id $RootPid
$exe = $rootProcess.Path
$version = if ($exe) { (Get-Item $exe).VersionInfo.ProductVersion } else { $null }

$deadline = (Get-Date).AddSeconds($QuietTimeout)
while ($true) {
    $before = Get-Load 5
    if ($before -le $MaxLoad -or (Get-Date) -gt $deadline) { break }
    Write-Host "waiting for a quiet machine: CPU $before% > $MaxLoad%"
}
Write-Host "Idling $IdleSeconds s, then $Samples samples ($Target $State, root $RootPid $exe $version)"
$idle = Get-Load $IdleSeconds
$m = Measure-Tree -Root $RootPid -Samples $Samples
$webview = @($m.processes | Where-Object name -eq 'msedgewebview2.exe').Count
if ($State -eq 'S0' -and $webview) { Write-Warning "S0 means no bot browser was ever opened, but the tree has $webview msedgewebview2.exe processes." }

$result = [ordered]@{
    target = $Target; state = $State; root_pid = $RootPid; exe = $exe; version = $version
    measured = (Get-Date).ToString('s'); idle_seconds = $IdleSeconds; samples = $Samples
    cpu_before = $before; cpu_idle = $idle; loaded = $idle -gt $MaxLoad + 15
    private_working_set_mib = $m.private_working_set_mib
    private_commit_mib = $m.private_commit_mib
    working_set_mib = $m.working_set_mib
    processes = $m.processes
}
$dir = Join-Path $env:LOCALAPPDATA 'Tjommie-rs\perf'
New-Item -ItemType Directory -Force $dir | Out-Null
$file = Join-Path $dir ('memory-{0}-{1}-{2}.json' -f $Target.ToLower(), $State, (Get-Date -Format 'yyyyMMdd-HHmmss'))
$result | ConvertTo-Json -Depth 4 | Set-Content -Encoding utf8 $file
'{0} {1} {2}: private WS {3} MiB, private commit {4} MiB, WS {5} MiB, {6} processes; CPU before {7}%, idle {8}%{9}. Saved {10}' -f $Target, $version, $State, $m.private_working_set_mib, $m.private_commit_mib, $m.working_set_mib, $m.processes.Count, $before, $idle, $(if ($result.loaded) { ' (LOADED)' } else { '' }), $file
