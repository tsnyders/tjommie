# Stopping processes for the scripts that install and uninstall Tjommie (T8.1, T8.2). Dot-source this file; it defines functions only.
#
# A process is stopped only if its executable is inside one folder, the install folder of this run, never because of its
# name: "Tjommie.exe" is also the name of the owner's own app on a real PC. The hosted-runner guard (hosted-runner.ps1) is
# the first defence against that PC; this is the second, so a lost guard still can't reach a Tjommie that lives somewhere
# else. Nothing in this repository stops, kills or looks up a process by its image name.
#
# Nothing here compiles code (no Add-Type): the helpers that use this run inside an installer, where a compiler that is slow, blocked
# or scanned would make them fail for a reason that has nothing to do with the install.

Set-StrictMode -Version Latest

# The full path with 8.3 names spelled out (RUNNER~1 becomes runneradmin), even where its last parts do not exist yet. A process
# started through a short name reports that spelling, and so may a folder. Only a part that looks like an 8.3 alias (it holds a
# tilde and a digit) is looked up, by listing its parent for that name, which Windows answers with the long entry; every other part
# is kept as given, because case does not matter to the comparisons. Never throws: if a part cannot be resolved it is kept as given.
function Get-LongPath([string]$Path) {
    try { $full = [IO.Path]::GetFullPath($Path) } catch { return $Path }
    if ($full -notmatch '~\d') { return $full }
    try {
        $root = [IO.Path]::GetPathRoot($full)
        $result = $root.TrimEnd('\')
        foreach ($part in $full.Substring($root.Length).Split('\')) {
            if (-not $part) { continue }
            $resolved = $null
            if ($part -match '~\d') {
                try {
                    $entries = @([IO.Directory]::GetFileSystemEntries($result + '\', $part))
                    if ($entries.Count -eq 1) { $resolved = $entries[0] }
                } catch { }
            }
            $result = if ($resolved) { $resolved } else { $result + '\' + $part }
        }
        return $result
    } catch { return $full }
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

# What Select-ProcessesInFolder left alone on its last call because it had a window: process id -> process. And why it chose each
# process it did: process id -> "in the folder" or "child of <id>". For a helper to log; both are replaced on every call.
$script:LastSkippedWindowed = @{}
$script:LastReasons = @{}

# The processes whose executable is inside $Folder, and the processes they started, however deep, as a table of process id to
# process. A child counts only if it started after its parent, so a process id Windows has reused for something else is not
# mistaken for a child; one whose parent is gone is nobody's child and is left alone. A child whose executable is outside the folder
# and which owns a visible window is left out, with everything it started: closing an app must not close the browser it opened (the
# table $script:LastSkippedWindowed lists those). This script's own process is never in it, and neither is any id in $ExceptIds (an
# uninstaller that runs from inside the folder it removes must not stop itself). The process list is asked for with a time limit.
# Has no effect.
function Select-ProcessesInFolder([string]$Folder, [int[]]$ExceptIds = @()) {
    Assert-NarrowFolder $Folder
    $all = @(Get-CimInstance Win32_Process -OperationTimeoutSec 30)
    $left = @($PID) + $ExceptIds
    $selected = @{}
    $skipped = @{}
    $reasons = @{}
    foreach ($process in $all) {
        $id = [int]$process.ProcessId
        if ($left -contains $id -or -not $process.ExecutablePath) { continue }
        if (Test-PathInFolder $process.ExecutablePath $Folder) { $selected[$id] = $process; $reasons[$id] = 'in the folder' }
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
                $reasons[$id] = "child of $parent"
                $added = $true
            }
        }
    } while ($added)
    $script:LastSkippedWindowed = $skipped
    $script:LastReasons = $reasons
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
