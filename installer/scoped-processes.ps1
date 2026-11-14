# Stopping processes for the scripts that install and uninstall Tjommie (T8.1). Dot-source this file; it defines functions only.
#
# A process is stopped only if its executable is inside one folder, the install folder of this run, never because of its
# name: "Tjommie.exe" is also the name of the owner's own app on a real PC. The hosted-runner guard (hosted-runner.ps1) is
# the first defence against that PC; this is the second, so a lost guard still can't reach a Tjommie that lives somewhere
# else. Nothing in this repository stops, kills or looks up a process by its image name.

Set-StrictMode -Version Latest

# Whether $Path is inside $Folder, spelled however Windows would resolve it (case-insensitive, `..` folded).
function Test-PathInFolder([string]$Path, [string]$Folder) {
    try {
        $full = [IO.Path]::GetFullPath($Path)
        $root = [IO.Path]::GetFullPath($Folder).TrimEnd('\') + '\'
    } catch { return $false }
    return $full.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)
}

# Refuses a folder that is a drive, or is, or holds, a system or profile folder: stopping "everything in" such a folder would
# stop much more than one app. Throws; has no other effect.
function Assert-NarrowFolder([string]$Folder) {
    if (-not $Folder) { throw 'No folder was given to stop processes in.' }
    $full = [IO.Path]::GetFullPath($Folder).TrimEnd('\')
    $root = $full + '\'
    if (@($full.Split('\') | Where-Object { $_ }).Count -lt 3) { throw "Refusing to stop processes in ${full}: it is too broad (a drive or a top-level folder)." }
    foreach ($name in 'SystemRoot', 'ProgramFiles', 'ProgramFiles(x86)', 'ProgramData', 'USERPROFILE', 'LOCALAPPDATA', 'APPDATA', 'TEMP', 'RUNNER_TEMP', 'GITHUB_WORKSPACE') {
        $value = [Environment]::GetEnvironmentVariable($name)
        if (-not $value) { continue }
        $other = [IO.Path]::GetFullPath($value).TrimEnd('\') + '\'
        if ($other.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) { throw "Refusing to stop processes in ${full}: it is, or holds, $name ($value)." }
    }
}

# The processes whose executable is inside $Folder, and the processes they started, however deep, as a table of process id to
# process. A child counts only if it started after its parent, so a process id Windows has reused for something else is not
# mistaken for a child. This script's own process is never in it, and neither is any id in $ExceptIds (an uninstaller that
# runs from inside the folder it removes must not stop itself). Has no effect.
function Select-ProcessesInFolder([string]$Folder, [int[]]$ExceptIds = @()) {
    Assert-NarrowFolder $Folder
    $all = @(Get-CimInstance Win32_Process)
    $left = @($PID) + $ExceptIds
    $selected = @{}
    foreach ($process in $all) {
        $id = [int]$process.ProcessId
        if ($left -contains $id -or -not $process.ExecutablePath) { continue }
        if (Test-PathInFolder $process.ExecutablePath $Folder) { $selected[$id] = $process }
    }
    do {
        $added = $false
        foreach ($process in $all) {
            $id = [int]$process.ProcessId
            if ($left -contains $id -or $selected.ContainsKey($id)) { continue }
            $parent = [int]$process.ParentProcessId
            if ($selected.ContainsKey($parent) -and $process.CreationDate -ge $selected[$parent].CreationDate) {
                $selected[$id] = $process
                $added = $true
            }
        }
    } while ($added)
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
