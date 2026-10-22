param([int]$DurationMs = 0, [int]$IntervalMs = 50)
$ErrorActionPreference = 'Stop'
Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public class TerminalSnapshot {
  public delegate bool EnumProc(IntPtr window, IntPtr extra);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc callback, IntPtr extra);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr window);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr window, out uint process);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr window, StringBuilder name, int max);
  public class Window { public long HWND; public uint pid; public string @class; }
  public static List<Window> Visible() {
    var windows = new List<Window>();
    EnumWindows((window, extra) => {
      if (IsWindowVisible(window)) {
        var name = new StringBuilder(256); GetClassName(window, name, 256);
        var kind = name.ToString();
        if (kind == "ConsoleWindowClass" || kind == "CASCADIA_HOSTING_WINDOW_CLASS") {
          uint pid; GetWindowThreadProcessId(window, out pid);
          windows.Add(new Window { HWND=window.ToInt64(), pid=pid, @class=kind });
        }
      }
      return true;
    }, IntPtr.Zero);
    return windows;
  }
}
'@
$watch = [Diagnostics.Stopwatch]::StartNew()
$last = ''
do {
  $current = ConvertTo-Json -InputObject @([TerminalSnapshot]::Visible() | Sort-Object HWND) -Compress
  if ($current -ne $last) { [Console]::WriteLine($current); $last = $current }
  if ($DurationMs -le 0 -or $watch.ElapsedMilliseconds -ge $DurationMs) { break }
  Start-Sleep -Milliseconds ([Math]::Max(20,$IntervalMs))
} while ($true)
