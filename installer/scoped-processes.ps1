# Stopping processes for the scripts that install and uninstall Tjommie (T8.1, T8.2). Dot-source this file; it defines functions only.
#
# A process is stopped only if its executable is inside one folder, the install folder of this run, never because of its
# name: "Tjommie.exe" is also the name of the owner's own app on a real PC. The hosted-runner guard (hosted-runner.ps1) is
# the first defence against that PC; this is the second, so a lost guard still can't reach a Tjommie that lives somewhere
# else. Nothing in this repository stops, kills or looks up a process by its image name.

Set-StrictMode -Version Latest

if (-not ('TjommieLongPath' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
using System.Text;

public static class TjommieLongPath
{
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern uint GetLongPathNameW(string shortPath, StringBuilder longPath, uint size);

    // The path with its 8.3 names (RUNNER~1) spelled out, or the path as given if it does not exist.
    public static string Get(string path)
    {
        StringBuilder buffer = new StringBuilder(32768);
        uint length = GetLongPathNameW(path, buffer, (uint)buffer.Capacity);
        if (length == 0 || length >= buffer.Capacity) return path;
        return buffer.ToString();
    }
}
'@
}

# The full path with 8.3 names spelled out, even where its last parts do not exist yet (the existing part is resolved, the rest is
# appended as given). A process started through a short name such as RUNNER~1 reports that spelling, and so may a folder.
function Get-LongPath([string]$Path) {
    $full = [IO.Path]::GetFullPath($Path)
    $rest = @()
    $current = $full.TrimEnd('\')
    while ($current -and -not [IO.File]::Exists($current) -and -not [IO.Directory]::Exists($current)) {
        $parent = [IO.Path]::GetDirectoryName($current)
        if (-not $parent) { return $full }
        $rest = @([IO.Path]::GetFileName($current)) + $rest
        $current = $parent
    }
    if (-not $current) { return $full }
    $long = [TjommieLongPath]::Get($current)
    foreach ($part in $rest) { $long = $long.TrimEnd('\') + '\' + $part }
    return $long
}

# Whether $Path is inside $Folder, spelled however Windows would resolve it (case-insensitive, `..` folded, 8.3 names spelled out).
function Test-PathInFolder([string]$Path, [string]$Folder) {
    try {
        $full = Get-LongPath $Path
        $root = (Get-LongPath $Folder).TrimEnd('\') + '\'
    } catch { return $false }
    return $full.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)
}

# Refuses a folder that is a drive, or is, or holds, a system or profile folder: stopping "everything in" such a folder would
# stop much more than one app. Throws; has no other effect.
function Assert-NarrowFolder([string]$Folder) {
    if (-not $Folder) { throw 'No folder was given to stop processes in.' }
    $full = (Get-LongPath $Folder).TrimEnd('\')
    $root = $full + '\'
    if (@($full.Split('\') | Where-Object { $_ }).Count -lt 3) { throw "Refusing to stop processes in ${full}: it is too broad (a drive or a top-level folder)." }
    foreach ($name in 'SystemRoot', 'ProgramFiles', 'ProgramFiles(x86)', 'ProgramData', 'USERPROFILE', 'LOCALAPPDATA', 'APPDATA', 'TEMP', 'RUNNER_TEMP', 'GITHUB_WORKSPACE') {
        $value = [Environment]::GetEnvironmentVariable($name)
        if (-not $value) { continue }
        $other = (Get-LongPath $value).TrimEnd('\') + '\'
        if ($other.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) { throw "Refusing to stop processes in ${full}: it is, or holds, $name ($value)." }
    }
}

# Whether a process owns a visible top-level window: a program the user is looking at, such as a browser the app opened, as
# opposed to a helper, a daemon or a command-line tool, which have none. False for a process that is gone.
function Test-OwnsVisibleWindow([int]$ProcessId) {
    try { return ([Diagnostics.Process]::GetProcessById($ProcessId).MainWindowHandle -ne [IntPtr]::Zero) } catch { return $false }
}

# What Select-ProcessesInFolder left alone on its last call because it had a window: process id -> process.
$script:LastSkippedWindowed = @{}

# The processes whose executable is inside $Folder, and the processes they started, however deep, as a table of process id to
# process. A child counts only if it started after its parent, so a process id Windows has reused for something else is not
# mistaken for a child. A child whose executable is outside the folder and which owns a visible window is left out, with
# everything it started: closing an app must not close the browser it opened (the table $script:LastSkippedWindowed lists those).
# This script's own process is never in it, and neither is any id in $ExceptIds (an uninstaller that runs from inside the folder it
# removes must not stop itself). Has no effect.
function Select-ProcessesInFolder([string]$Folder, [int[]]$ExceptIds = @()) {
    Assert-NarrowFolder $Folder
    $all = @(Get-CimInstance Win32_Process)
    $left = @($PID) + $ExceptIds
    $selected = @{}
    $skipped = @{}
    foreach ($process in $all) {
        $id = [int]$process.ProcessId
        if ($left -contains $id -or -not $process.ExecutablePath) { continue }
        if (Test-PathInFolder $process.ExecutablePath $Folder) { $selected[$id] = $process }
    }
    do {
        $added = $false
        foreach ($process in $all) {
            $id = [int]$process.ProcessId
            if ($left -contains $id -or $selected.ContainsKey($id) -or $skipped.ContainsKey($id)) { continue }
            $parent = [int]$process.ParentProcessId
            if ($selected.ContainsKey($parent) -and $process.CreationDate -ge $selected[$parent].CreationDate) {
                if (Test-OwnsVisibleWindow $id) { $skipped[$id] = $process; continue }
                $selected[$id] = $process
                $added = $true
            }
        }
    } while ($added)
    $script:LastSkippedWindowed = $skipped
    return $selected
}

# Stops what Select-ProcessesInFolder selects. Returns what was stopped.
function Stop-ProcessesInFolder([string]$Folder, [int[]]$ExceptIds = @()) {
    $selected = Select-ProcessesInFolder $Folder $ExceptIds
    $stopped = @()
    foreach ($id in $selected.Keys) {
        Stop-Process -Id $id -Force -ErrorAction SilentlyContinue
        $stopped += , ([ordered]@{ id = $id; name = $selected[$id].Name; executablePath = $selected[$id].ExecutablePath })
    }
    return $stopped
}

# The program a command line starts: the quoted path if it begins with a quote, else the first word. $null if there is none.
function Get-CommandExecutable([string]$CommandLine) {
    $text = $CommandLine.Trim()
    if (-not $text) { return $null }
    if ($text.StartsWith('"')) {
        $end = $text.IndexOf('"', 1)
        if ($end -lt 2) { return $null }
        return $text.Substring(1, $end - 1)
    }
    return ($text -split '\s+')[0]
}
