# Helpers for the installer acceptance scripts (T8.2): accept-fresh.ps1 and accept-over-r1.ps1, which install, upgrade and
# uninstall on a GitHub-hosted runner. Dot-source this file; it defines functions only and runs nothing. The scripts that use it
# start with the hosted-runner guard; nothing here is ever to be run on a real PC.
#
# Every check is recorded, not thrown: a failed check is written to the evidence file (and printed), the run goes on, and
# Complete-Acceptance fails it at the end, so a red run still shows everything it saw and the workflow uploads it.
#
# Written for Windows PowerShell 5.1 and PowerShell 7 alike. Dictionaries (what the readers return) are indexed, never read with
# .values or .count, which would give the dictionary's own members; facts read from JSON are objects and use properties.

Set-StrictMode -Version Latest

# The readers (registry keys, shortcuts, processes), which also load the process-scoping functions.
. (Join-Path $PSScriptRoot 'r1-facts-lib.ps1')

$script:Checks = New-Object System.Collections.Generic.List[object]
$script:Scenario = ''
$script:ResultPath = ''
$script:Deadline = [DateTime]::MaxValue
$script:BudgetMinutes = 0

# ---- results ----------------------------------------------------------------------------------------------------------------

# Starts a run. It has a time budget (minutes): a step that would start after it is skipped, and says so, so a run that has gone slow
# or wrong stops spending the job's time and still uploads what it saw. (The job's own timeout is the last resort and loses the evidence.)
function Start-Acceptance([string]$Scenario, [string]$OutDir, [int]$BudgetMinutes = 30) {
    New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
    $script:Scenario = $Scenario
    $script:ResultPath = Join-Path $OutDir "installer-acceptance-$Scenario.json"
    $script:Checks.Clear()
    $script:BudgetMinutes = $BudgetMinutes
    $script:Deadline = (Get-Date).AddMinutes($BudgetMinutes)
}

# The logs the installer and its helpers wrote (installer/helper-log.ps1; the installer's own installer.log is among them): the last
# lines of each, so a failed run carries the helper's exception, its stack and its decisions. Empty if there are none.
function Get-HelperLogs {
    $folder = if ($env:TJOMMIE_HELPER_LOG_DIR) { $env:TJOMMIE_HELPER_LOG_DIR } else { Join-Path ([IO.Path]::GetTempPath()) 'tjommie-installer-logs' }
    if (-not (Test-Path -LiteralPath $folder -PathType Container)) { return @() }
    $logs = @()
    foreach ($file in @(Get-ChildItem -LiteralPath $folder -File | Sort-Object Name | Select-Object -Last 60)) {
        $logs += , ([ordered]@{ file = $file.Name; lines = @(Get-Content -LiteralPath $file.FullName -Tail 150 -ErrorAction SilentlyContinue) })
    }
    return $logs
}

# Writes the evidence file: every check so far and the helper logs, and copies the logs themselves next to it (helper-logs\), because
# a log is easier to read as a file than as JSON. Saving must never fail a run, so the copy is best effort.
function Save-Acceptance {
    $failed = @($script:Checks | Where-Object { -not $_['ok'] }).Count
    # .ToArray(), not @(...): in PowerShell 7, @() of a generic List inside a dictionary literal throws "Argument types do not match".
    [ordered]@{ scenario = $script:Scenario; passed = ($failed -eq 0); failed = $failed; checks = $script:Checks.ToArray(); helperLogs = @(Get-HelperLogs) } |
        ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $script:ResultPath -Encoding UTF8
    try {
        $source = if ($env:TJOMMIE_HELPER_LOG_DIR) { $env:TJOMMIE_HELPER_LOG_DIR } else { Join-Path ([IO.Path]::GetTempPath()) 'tjommie-installer-logs' }
        if (Test-Path -LiteralPath $source -PathType Container) {
            $target = Join-Path (Split-Path -Parent $script:ResultPath) 'helper-logs'
            New-Item -ItemType Directory -Force -Path $target | Out-Null
            Copy-Item -Path (Join-Path $source '*') -Destination $target -Force -ErrorAction SilentlyContinue
        }
    } catch { }
}

function Assert-Check([bool]$Condition, [string]$Message, $Detail = $null) {
    $script:Checks.Add([ordered]@{ ok = $Condition; check = $Message; detail = $Detail })
    if ($Condition) { Write-Output "ok:   $Message"; return }
    $shown = if ($null -ne $Detail) { ' ' + (ConvertTo-Json $Detail -Compress -Depth 6) } else { '' }
    Write-Output "FAIL: $Message$shown"
}

# Records something the run saw that is not a pass or a fail: what a program outside this repository did (R1's own uninstaller).
function Add-Note([string]$Message, $Detail = $null) {
    $script:Checks.Add([ordered]@{ ok = $true; note = $true; check = $Message; detail = $Detail })
    Write-Output "note: $Message"
}

# Runs one step. A step that throws is recorded as a failed check and the next step still runs; the file is saved after it.
function Invoke-Step([string]$Name, [scriptblock]$Body) {
    Write-Output "--- $Name ---"
    if ((Get-Date) -gt $script:Deadline) {
        Assert-Check $false "$Name was skipped: this run's time budget of $($script:BudgetMinutes) minutes is spent, so a step before it took far longer than it should"
        Save-Acceptance
        return
    }
    try { & $Body } catch { Assert-Check $false "$Name stopped: $($_.Exception.Message)" ([string]$_.ScriptStackTrace) }
    Save-Acceptance
}

function Complete-Acceptance {
    Save-Acceptance
    $failed = @($script:Checks | Where-Object { -not $_['ok'] })
    if ($failed.Count -gt 0) { throw "$($failed.Count) of $($script:Checks.Count) checks failed: $(($failed | ForEach-Object { $_['check'] }) -join '; ')" }
    Write-Output "All $($script:Checks.Count) checks passed."
}

function Wait-Until([scriptblock]$Condition, [int]$Seconds = 60) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.Elapsed.TotalSeconds -lt $Seconds) {
        if (& $Condition) { return $true }
        Start-Sleep -Milliseconds 250
    }
    return [bool](& $Condition)
}

# ---- files ------------------------------------------------------------------------------------------------------------------

# Every file under $Root as relative path -> SHA-256 (or 'unreadable' for a file another process holds open).
function Get-TreeSnapshot([string]$Root) {
    $map = [ordered]@{}
    if (-not (Test-Path -LiteralPath $Root -PathType Container)) { return $map }
    $base = (Resolve-Path -LiteralPath $Root).ProviderPath.TrimEnd('\')
    foreach ($file in (Get-ChildItem -LiteralPath $base -Recurse -File -Force | Sort-Object FullName)) {
        $hash = 'unreadable'
        try { $hash = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash } catch { }
        $map[$file.FullName.Substring($base.Length + 1)] = $hash
    }
    return $map
}

# What differs between two snapshots: files gone, files whose content changed, and (unless told to ignore them) files added.
# -IgnorePaths names relative paths that are not to be reported at all, whatever happened to them (case does not matter): the
# files a program has only while it runs. Nothing else is ignored, so a real loss is still reported.
function Compare-Snapshot($Before, $After, [switch]$IgnoreAdded, [string[]]$IgnorePaths = @()) {
    $differences = @()
    $ignored = @($IgnorePaths | ForEach-Object { $_.ToLowerInvariant() })
    foreach ($path in $Before.Keys) {
        if ($ignored -contains $path.ToLowerInvariant()) { continue }
        if (-not $After.Contains($path)) { $differences += "missing: $path"; continue }
        if ($Before[$path] -ne 'unreadable' -and $After[$path] -ne 'unreadable' -and $Before[$path] -ne $After[$path]) { $differences += "changed: $path" }
    }
    if (-not $IgnoreAdded) { foreach ($path in $After.Keys) { if (-not $Before.Contains($path) -and $ignored -notcontains $path.ToLowerInvariant()) { $differences += "added: $path" } } }
    return $differences
}

# What R1 has in its data folders while it runs and not once it has been stopped: the entries its first start left (T8.1's
# firstStart) that its folders no longer held after it was stopped and uninstalled (T8.1's uninstall). Electron's lockfile is one.
# These come and go with the process, so a snapshot taken with R1 running must not count them as data that was lost.
function Get-R1RuntimeOnlyEntries($Facts) {
    $names = @()
    foreach ($pair in @(@($Facts.firstStart.userDataEntries, $Facts.uninstall.userDataEntries), @($Facts.firstStart.dataRootEntries, $Facts.uninstall.dataRootEntries))) {
        $running = @($pair[0] | ForEach-Object { $_.name })
        $stopped = @($pair[1] | ForEach-Object { $_.name })
        $names += @($running | Where-Object { $stopped -notcontains $_ })
    }
    return @($names | Sort-Object -Unique)
}

# A few nested binary files in a data root, like a real one's, to be found again byte for byte. Returns their relative paths.
function Add-Sentinels([string]$Root) {
    $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
    $made = @()
    try {
        foreach ($relative in 'sentinel\acceptance.db', 'sentinel\nested\deeper\secret.bin', 'sentinel\notes.md') {
            $path = Join-Path $Root $relative
            New-Item -ItemType Directory -Force -Path (Split-Path -Parent $path) | Out-Null
            $bytes = New-Object byte[] 4096
            $rng.GetBytes($bytes)
            [IO.File]::WriteAllBytes($path, $bytes)
            $made += $relative
        }
    } finally { $rng.Dispose() }
    return $made
}

# The files a stub-payload install holds: the identity's exe and daemon, and the uninstaller NSIS writes. (The helper scripts the
# uninstaller needs are inside the uninstaller, not beside the app.) Sorted, relative, backslashes.
function Get-ExpectedFiles([string]$Product, [string]$Exe) {
    return @($Exe, 'resources\daemon\tjommie-daemon.exe', "Uninstall $Product.exe") | Sort-Object
}

function Get-InstalledFiles([string]$Folder) {
    if (-not (Test-Path -LiteralPath $Folder -PathType Container)) { return @() }
    $base = (Resolve-Path -LiteralPath $Folder).ProviderPath.TrimEnd('\')
    return @(Get-ChildItem -LiteralPath $base -Recurse -File -Force | ForEach-Object { $_.FullName.Substring($base.Length + 1) } | Sort-Object)
}

# ---- registry ---------------------------------------------------------------------------------------------------------------

$script:RunKey = 'Software\Microsoft\Windows\CurrentVersion\Run'

function Get-RunValue([string]$Name) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($script:RunKey)
    if ($null -eq $key) { return $null }
    try { return $key.GetValue($Name, $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames) } finally { $key.Close() }
}

# Writes (or, with $null, removes) a start-at-login value, as the app's login item does. $Data is not typed: a [string] parameter
# turns $null into "", which would write an empty value instead of removing it.
function Set-RunValue([string]$Name, $Data) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey($script:RunKey)
    try {
        if ($null -eq $Data) { $key.DeleteValue($Name, $false) } else { $key.SetValue($Name, $Data, [Microsoft.Win32.RegistryValueKind]::String) }
    } finally { $key.Close() }
}

# Writes (or, with $null, removes) HKCU\Software\<GUID>\InstallLocation: what the uninstaller's guard reads. $Location is not typed,
# for the reason $Data in Set-RunValue is not.
function Set-InstallLocation([string]$Guid, $Location) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey("Software\$Guid")
    try {
        if ($null -eq $Location) { $key.DeleteValue('InstallLocation', $false) } else { $key.SetValue('InstallLocation', $Location, [Microsoft.Win32.RegistryValueKind]::String) }
    } finally { $key.Close() }
}

# What HKCU\Software\<GUID>\InstallLocation holds now, or $null if it is not set.
function Get-InstallLocation([string]$Guid) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Software\$Guid")
    if ($null -eq $key) { return $null }
    try { return $key.GetValue('InstallLocation') } finally { $key.Close() }
}

# A directory junction: what a developer makes inside an install folder with `mklink /J`. Needs no privilege.
function New-Junction([string]$Link, [string]$Target) {
    New-Item -ItemType Junction -Path $Link -Target $Target | Out-Null
}

# The processes whose executable path contains a word (case does not matter), as process id -> path: to see afterwards which of them
# are gone. Looks by path, never by name. A plain hashtable, not an ordered dictionary: an ordered dictionary reads an int index as a
# position, so $found[$id] with a process id threw "Specified argument was out of the range" on the runner.
function Get-ProcessPathsMatching([string]$Word) {
    $found = @{}
    foreach ($process in (Get-CimInstance Win32_Process)) {
        if ($process.ExecutablePath -and $process.ExecutablePath -match [regex]::Escape($Word)) { $found[[int]$process.ProcessId] = [string]$process.ExecutablePath }
    }
    return $found
}

function Test-KeyExists([string]$SubKey) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey($SubKey)
    if ($null -eq $key) { return $false }
    $key.Close()
    return $true
}

function Get-UninstallKeyName([string]$Guid) { return "Software\Microsoft\Windows\CurrentVersion\Uninstall\$Guid" }

# ---- running installers -----------------------------------------------------------------------------------------------------

# Runs an installer (or any setup exe) with its arguments as one string, verbatim (NSIS wants /D= last and unquoted), and waits.
# With -Watch it also records every process that starts meanwhile, with its command line: how the old uninstaller is run.
function Invoke-Setup([string]$Installer, [string]$Arguments, [int]$TimeoutSeconds = 240, [switch]$Watch) {
    $before = @([Diagnostics.Process]::GetProcesses() | ForEach-Object { $_.Id })
    $process = Start-Process -FilePath $Installer -ArgumentList $Arguments -PassThru -WindowStyle Hidden
    # Touching the handle now keeps ExitCode readable for a process that exits at once (it is $null otherwise, which would read as "not zero").
    $null = $process.Handle
    $seen = [ordered]@{}
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while (-not $process.HasExited) {
        if ($Watch) { foreach ($fact in (Get-NewProcessFacts $before)) { $seen["$($fact['name'])|$($fact['commandLine'])"] = $fact } }
        if ((Get-Date) -gt $deadline) {
            Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
            throw "$Installer did not finish within $TimeoutSeconds seconds."
        }
        Start-Sleep -Milliseconds 20
    }
    $process.WaitForExit()
    if ($null -eq $process.ExitCode) { throw "$Installer finished but its exit code could not be read." }
    return [ordered]@{ exitCode = [int]$process.ExitCode; processes = @($seen.Values) }
}

# Runs the quiet uninstall command an install registered, as Windows would (through cmd), and waits for the install folder to go
# (the uninstaller copies itself to %TEMP% and the first process returns at once). -ExpectRemoval:$false waits a fixed time
# instead, for a run that is meant to refuse. Returns whether the folder is gone.
function Invoke-RegisteredUninstall([string]$Guid, [string]$Folder, [switch]$ExpectRemoval = $true, [int]$Seconds = 120) {
    $key = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey((Get-UninstallKeyName $Guid))
    if ($null -eq $key) { throw "There is no Uninstall key for $Guid." }
    try { $quiet = [string]$key.GetValue('QuietUninstallString') } finally { $key.Close() }
    if (-not $quiet) { throw "The Uninstall key for $Guid has no QuietUninstallString." }
    $process = Start-Process -FilePath "$env:SystemRoot\System32\cmd.exe" -ArgumentList '/d', '/s', '/c', "`"$quiet`"" -PassThru -WindowStyle Hidden
    $null = $process.Handle
    [void]$process.WaitForExit(60000)
    if ($ExpectRemoval) {
        # The uninstaller removes the folder from a copy of itself in %TEMP%\~nsu*.tmp. Wait for the folder to go, but not for a folder that
        # cannot go: once that copy has run and ended, or never started, the folder is not going to disappear, and waiting out the
        # whole limit for every failed uninstall is what made the fresh job crawl. (The limit stays the last resort.)
        $watch = [Diagnostics.Stopwatch]::StartNew()
        $sawUninstaller = $false
        $gone = $false
        while ($watch.Elapsed.TotalSeconds -lt $Seconds) {
            if (-not (Test-Path -LiteralPath $Folder)) { $gone = $true; break }
            if ((Get-ProcessPathsMatching '~nsu').Count -gt 0) { $sawUninstaller = $true }
            elseif ($sawUninstaller -or $watch.Elapsed.TotalSeconds -gt 20) { break }
            Start-Sleep -Milliseconds 500
        }
        if (-not $gone) { $gone = -not (Test-Path -LiteralPath $Folder) }
        if ($gone) { Start-Sleep -Seconds 3 }
        return $gone
    }
    Start-Sleep -Seconds 15
    return (-not (Test-Path -LiteralPath $Folder))
}

# ---- processes --------------------------------------------------------------------------------------------------------------

# Starts a stub program (installer/build-stub-payload.ps1), which logs to the file in $env:STUB_LOG. Returns the process.
function Start-Stub([string]$Exe, [string]$Arguments = '') {
    if ($Arguments) { return Start-Process -FilePath $Exe -ArgumentList $Arguments -WindowStyle Hidden -PassThru }
    return Start-Process -FilePath $Exe -WindowStyle Hidden -PassThru
}

function Test-Alive([int]$Id) { return [bool](Get-Process -Id $Id -ErrorAction SilentlyContinue) }

# The lines the stubs have logged so far ("<file> <process id> <arguments>"), or none.
function Get-StubLog {
    if (-not $env:STUB_LOG -or -not (Test-Path -LiteralPath $env:STUB_LOG)) { return @() }
    return @(Get-Content -LiteralPath $env:STUB_LOG)
}

# The ids of the processes running from $Folder (and their children): what must be empty once an installer has closed it.
function Get-RunningFrom([string]$Folder) { return @((Select-ProcessesInFolder $Folder).Keys) }

# ---- checks against what T8.1 recorded from R1 ------------------------------------------------------------------------------

function Read-Json([string]$Path) {
    $text = Get-Content -LiteralPath $Path -Raw
    if ($text.Length -gt 0 -and [int][char]$text[0] -eq 0xFEFF) { $text = $text.Substring(1) }
    return $text | ConvertFrom-Json
}

# The Uninstall key and the HKCU\Software\<GUID> key, against R1's: the same value names (no others), the same types, the same
# data once R1's folder and version are replaced by this install's. EstimatedSize is the one value that cannot be equal: it is
# the size of the install, so it is checked to be a positive number.
function Test-InstalledAsR1Did($Facts, [string]$Folder, [string]$Version, [string]$Label = 'install') {
    $recordedFolder = [string]$Facts.install.folderAsFound
    $recordedVersion = [string]$Facts.release.version
    $swap = { param([string]$Text) $Text.Replace($recordedFolder, $Folder).Replace($recordedVersion, $Version) }
    $uninstallFact = $Facts.registry.hkcuUninstallView64[0]
    $installFact = $Facts.registry.hkcuInstallKeys[0]
    $guid = ($uninstallFact.key -split '\\')[-1]

    foreach ($pair in @(@($uninstallFact, 'the Uninstall key'), @($installFact, 'the install key HKCU\Software\<GUID>'))) {
        $fact = $pair[0]
        $name = $pair[1]
        $actual = Get-RegistryKeyFact 'CurrentUser' ([string]$fact.key) '64'
        Assert-Check ($null -ne $actual) "$Label`: $name exists ($($fact.key))"
        if ($null -eq $actual) { continue }
        $actualValues = @($actual['values'])
        $recorded = @($fact.values)
        $actualNames = @($actualValues | ForEach-Object { $_['name'] } | Sort-Object)
        $recordedNames = @($recorded | ForEach-Object { $_.name } | Sort-Object)
        Assert-Check (($actualNames -join '|') -ceq ($recordedNames -join '|')) "$Label`: $name has R1's values and no others" @{ actual = $actualNames; recorded = $recordedNames }
        foreach ($value in $recorded) {
            $now = $actualValues | Where-Object { $_['name'] -ceq $value.name } | Select-Object -First 1
            if ($null -eq $now) { continue }
            if ($value.name -eq 'EstimatedSize') {
                Assert-Check (([string]$now['kind'] -eq 'DWord') -and ([int64]$now['data'] -gt 0)) "$Label`: $name EstimatedSize is a positive DWord" $now['data']
                continue
            }
            $want = if ($value.kind -eq 'String') { & $swap ([string]$value.data) } else { [string]$value.data }
            Assert-Check (([string]$now['kind'] -eq [string]$value.kind) -and ([string]$now['data'] -ceq $want)) "$Label`: $name value $($value.name) is $want" @{ kind = $now['kind']; data = $now['data'] }
        }
    }
    # Nothing outside the user's profile: no all-users keys.
    foreach ($key in @("Software\$guid", (Get-UninstallKeyName $guid))) {
        Assert-Check ($null -eq (Get-RegistryKeyFact 'LocalMachine' $key '64')) "$Label`: HKLM\$key is not written"
    }
}

# The two user shortcuts, against R1's: the same target, arguments, working directory, icon, description, window style and
# AppUserModelID, and no toast activator (R1 sets none). No shortcut for all users.
function Test-ShortcutsAsR1Had($Facts, [string]$Folder, [string]$Product, [string]$Exe, [string]$Aumid, [string]$Label = 'install') {
    $recorded = @($Facts.shortcuts.userStartMenu | Where-Object { $_.path -eq 'Tjommie.lnk' })[0]
    $folders = Get-ShortcutFolders
    $target = Join-Path $Folder $Exe
    foreach ($place in 'userStartMenu', 'userDesktop') {
        $mine = @(Get-ShortcutFacts $folders[$place] | Where-Object { $_['path'] -eq "$Product.lnk" })
        Assert-Check ($mine.Count -eq 1) "$Label`: $place has $Product.lnk" $mine.Count
        if ($mine.Count -ne 1) { continue }
        $link = $mine[0]
        Assert-Check ($link['target'] -ieq $target) "$Label`: $place shortcut targets the installed exe" $link['target']
        Assert-Check ($link['arguments'] -eq [string]$recorded.arguments) "$Label`: $place shortcut has R1's arguments (none)" $link['arguments']
        Assert-Check ($link['workingDirectory'] -ieq $Folder) "$Label`: $place shortcut starts in the install folder" $link['workingDirectory']
        Assert-Check ($link['iconLocation'] -ieq "$target,0") "$Label`: $place shortcut's icon is its exe, index 0" $link['iconLocation']
        Assert-Check ($link['description'] -ceq [string]$recorded.description) "$Label`: $place shortcut has R1's description" $link['description']
        Assert-Check ([int]$link['windowStyle'] -eq [int]$recorded.windowStyle) "$Label`: $place shortcut has R1's window style" $link['windowStyle']
        Assert-Check ($link['appUserModelId'] -ceq $Aumid) "$Label`: $place shortcut carries the AppUserModelID $Aumid" $link['appUserModelId']
        Assert-Check ($link['toastActivatorClsid'] -eq [string]$recorded.toastActivatorClsid) "$Label`: $place shortcut has no toast activator, as R1's" $link['toastActivatorClsid']
    }
    foreach ($place in 'allUsersStartMenu', 'allUsersDesktop') {
        $theirs = @(Get-ShortcutFacts $folders[$place] | Where-Object { $_['path'] -eq "$Product.lnk" })
        Assert-Check ($theirs.Count -eq 0) "$Label`: no $Product.lnk for all users ($place)" $theirs.Count
    }
}
