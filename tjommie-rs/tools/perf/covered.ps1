# Can the app's window be drawn? It opens without focus, under whatever is in front. GPUI drew no
# frames at all under a full-screen video player (a borderless window exactly the size of the screen,
# 2026-11-10) or with the screen locked, but drew normally under an ordinary maximized window, whose
# rect overhangs the screen by its borders (147 frames in a 3-loop test under Paseo, 2026-11-13).
# Dot-sourced by run-scenario.ps1, load.ps1's experiment and cold-start.ps1. These findings are from the GPUI spike; tjommie-desktop is expected to behave the same.
if (-not ('PerfGate.Window' -as [type])) {
    Add-Type -Namespace PerfGate -Name Window -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr GetForegroundWindow();
[DllImport("user32.dll")] public static extern bool GetWindowRect(System.IntPtr h, out RECT r);
[DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
[DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(System.IntPtr h, System.Text.StringBuilder s, int n);
public struct RECT { public int Left, Top, Right, Bottom; }
'@
}
function Test-Covered {
    if (Get-Process -Name LogonUI -ErrorAction SilentlyContinue) { return $true }   # the lock screen is up
    $h = [PerfGate.Window]::GetForegroundWindow()
    if ($h -eq [IntPtr]::Zero) { return $false }
    $class = New-Object System.Text.StringBuilder 64
    [void][PerfGate.Window]::GetClassNameW($h, $class, 64)
    if ($class.ToString() -in 'Progman', 'WorkerW', 'Shell_TrayWnd') { return $false }   # the desktop and taskbar
    $r = New-Object PerfGate.Window+RECT
    [void][PerfGate.Window]::GetWindowRect($h, [ref]$r)
    # Full screen: exactly the screen's rect (a maximized window overhangs it by its borders).
    $r.Left -eq 0 -and $r.Top -eq 0 -and $r.Right -eq [PerfGate.Window]::GetSystemMetrics(0) -and $r.Bottom -eq [PerfGate.Window]::GetSystemMetrics(1)
}
