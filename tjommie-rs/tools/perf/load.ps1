<#
A fixed synthetic background load (plan T7.3): N busy threads at IDLE priority (thread priority idle inside an Idle-class
process), so other work still runs but the CPU reads busy. It prints the system CPU it reaches, then every 10 s.
Never run by CI; it puts real load on the PC, so run it only in the owner's T7.3 session (owner-perf-run.md).

    .\tools\perf\load.ps1                       # Threads = 70% of the logical processors, 60 s
    .\tools\perf\load.ps1 -Threads 9 -Seconds 300

Stops at its deadline or on Ctrl+C (the threads are background threads, so they die with the process).
Aim for 60 to 80% system CPU: raise -Threads if it reaches less, lower it if more.
#>
param(
    [int]$Threads = 0,
    [int]$Seconds = 60
)
$ErrorActionPreference = 'Stop'
if ($Threads -le 0) { $Threads = [math]::Max(1, [int][math]::Round([Environment]::ProcessorCount * 0.7)) }
if (-not ('PerfLoad.Spin' -as [type])) {
    Add-Type -Namespace PerfLoad -Name Spin -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("kernel32.dll")] static extern System.IntPtr GetCurrentThread();
[System.Runtime.InteropServices.DllImport("kernel32.dll")] static extern bool SetThreadPriority(System.IntPtr h, int p);
public static volatile bool Stop;
public static System.Collections.Generic.List<System.Threading.Thread> Start(int n) {
    Stop = false;
    var list = new System.Collections.Generic.List<System.Threading.Thread>();
    for (int i = 0; i < n; i++) {
        var t = new System.Threading.Thread(() => {
            SetThreadPriority(GetCurrentThread(), -15);   // THREAD_PRIORITY_IDLE
            ulong x = 1;
            while (!Stop) { x = x * 6364136223846793005UL + 1442695040888963407UL; }
        });
        t.IsBackground = true;
        t.Start();
        list.Add(t);
    }
    return list;
}
'@
}
$counter = '\Processor(_Total)\% Processor Time'
function Get-Load([int]$Seconds) {
    [math]::Round(((Get-Counter $counter -SampleInterval 1 -MaxSamples $Seconds).CounterSamples.CookedValue | Measure-Object -Average).Average, 1)
}
$self = [System.Diagnostics.Process]::GetCurrentProcess()
$oldClass = $self.PriorityClass
$threads = $null
try {
    $self.PriorityClass = [System.Diagnostics.ProcessPriorityClass]::Idle
    $before = Get-Load 3
    $deadline = (Get-Date).AddSeconds($Seconds)
    $threads = [PerfLoad.Spin]::Start($Threads)
    Write-Host "load: $Threads idle-priority threads for $Seconds s (CPU before $before%)"
    Start-Sleep -Seconds 2
    Write-Host "load: system CPU reached $(Get-Load 5)%"
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Seconds ([math]::Min(10, [math]::Max(1, ($deadline - (Get-Date)).TotalSeconds)))
        if ((Get-Date) -lt $deadline) { Write-Host "load: system CPU $(Get-Load 2)%" }
    }
} finally {
    [PerfLoad.Spin]::Stop = $true
    if ($threads) { foreach ($t in $threads) { [void]$t.Join(2000) } }
    $self.PriorityClass = $oldClass
    Write-Host 'load: stopped'
}
