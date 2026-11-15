# Records what an installed R1 looks like (T8.1), as JSON, so T8.2's NSIS installer is written to facts and not to guesses.
# It runs after install-latest-r1.ps1, on the same GitHub-hosted runner, and refuses to run anywhere else: it starts the
# app, runs the installer again over itself and then uninstalls, all of which change an install and its registry keys.
#
#   install      The install as the installer left it, before the app has ever run: the folder and its files, a survey of every
#                uninstall entry in HKCU and HKLM (both views), the entry that points into this install and the install key,
#                the shortcuts and their AppUserModelID.
#   firstStart   The app starts once, and the start-at-login entry it writes (HKCU Run and StartupApproved) is read once it has
#                written it, with what the app leaves in its two data folders.
#   upgrade      The same installer runs again over the install; the processes it starts, with their command lines, show how
#                an installer runs the old uninstaller.
#   uninstall    The registered quiet uninstall command runs, and what is left behind is recorded.
#
# The facts file is written after the install facts are gathered and BEFORE anything is checked, and again after every phase.
# A phase that fails is recorded in the file ("problems") and the next phase still runs; only at the very end, with the file
# written, does a failed required phase (install, firstStart) fail the run. So a red run still has what it saw, and the
# workflow uploads it. upgrade and uninstall are best effort and never fail the run.
param(
    [string]$InstallResult = '',
    [string]$OutDir = '',
    [int]$FirstStartSeconds = 120
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot 'hosted-runner.ps1')
Assert-HostedRunner
. (Join-Path $PSScriptRoot 'r1-facts-lib.ps1')
. (Join-Path $PSScriptRoot 'scoped-processes.ps1')

if (-not $InstallResult) { $InstallResult = $env:R1_INSTALL_RESULT }
if (-not $InstallResult) { throw 'No install result: run install-latest-r1.ps1 first, or pass -InstallResult.' }
if (-not $OutDir) { $OutDir = Join-Path $env:RUNNER_TEMP 'r1-install-facts' }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$factsPath = Join-Path $OutDir 'r1-install-facts.json'

$install = Get-Content -LiteralPath $InstallResult -Raw | ConvertFrom-Json
# The install result says which folder this script starts, stops and uninstalls from: only one made on this run, on a fresh runner.
Assert-InstallResultBoundToThisRun $install
$folder = [string]$install.installFolder

$productName = 'Tjommie'
$uninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall'
$runKey = 'Software\Microsoft\Windows\CurrentVersion\Run'
$approvedKey = 'Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run'
$userData = Join-Path $env:APPDATA $productName
$dataRoot = Join-Path $env:LOCALAPPDATA $productName

$facts = [ordered]@{ schema = 1; complete = $false; problems = @() }
$facts['recorded'] = [ordered]@{
    at          = (Get-Date).ToUniversalTime().ToString('o')
    runId       = $env:GITHUB_RUN_ID
    commit      = $env:GITHUB_SHA
    runnerImage = $env:ImageVersion
    os          = [Environment]::OSVersion.VersionString
    powershell  = $PSVersionTable.PSVersion.ToString()
}
$facts['tokens'] = Get-PathTokens
$facts['release'] = [ordered]@{
    tag                = [string]$install.tag
    version            = [string]$install.version
    installer          = [string]$install.installerName
    installerBytes     = [long]$install.installerBytes
    installerSha512    = [string]$install.installerSha512
    installerExitCode  = [int]$install.installerExitCode
    latestYml          = [string]$install.latestYml
}
$state = @{ folderOk = $false }
$problems = New-Object System.Collections.Generic.List[string]

# Writes everything known so far, with the files the install script saved, to the folder the workflow uploads.
function Save-Facts {
    $facts | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $factsPath -Encoding UTF8
    Copy-Item -LiteralPath $InstallResult -Destination (Join-Path $OutDir 'install-result.json') -Force
    $yml = Join-Path (Split-Path -Parent $InstallResult) 'latest.yml'
    if (Test-Path -LiteralPath $yml) { Copy-Item -LiteralPath $yml -Destination (Join-Path $OutDir 'latest.yml') -Force }
}

# Runs one phase. Its failure is recorded in the facts file, not thrown, so the phases after it still run and the file is
# written either way; a failed required phase fails the run at the end.
function Invoke-Phase([string]$Name, [bool]$Required, [scriptblock]$Body) {
    Write-Output "--- $Name ---"
    try { & $Body } catch {
        $message = $_.Exception.Message
        $facts['problems'] += , ([ordered]@{ phase = $Name; required = $Required; message = $message; where = [string]$_.ScriptStackTrace })
        Write-Output "PHASE FAILED ($Name): $message"
        if ($Required) { $problems.Add("${Name}: $message") }
    }
    Save-Facts
}

# What an install must be before anything is started, stopped or uninstalled here: a per-user NSIS install, directly under the
# user's Programs folder, holding Tjommie.exe. Everything this script does to processes is scoped to this one folder.
try {
    if (-not (Test-Path -LiteralPath $folder -PathType Container)) { throw "The install folder $folder does not exist." }
    if (-not (Test-Path -LiteralPath (Join-Path $folder 'Tjommie.exe'))) { throw "No Tjommie.exe in $folder." }
    if ((Split-Path -Parent ([IO.Path]::GetFullPath($folder).TrimEnd('\'))) -ine (Join-Path $env:LOCALAPPDATA 'Programs')) { throw "$folder is not an install folder under %LOCALAPPDATA%\Programs." }
    Assert-NarrowFolder $folder
    $state['folderOk'] = $true
} catch {
    $facts['problems'] += , ([ordered]@{ phase = 'folder'; required = $true; message = $_.Exception.Message; where = [string]$_.ScriptStackTrace })
    Save-Facts
    throw
}

# Stops what runs from this run's install folder, and their children; never anything by its name (a real PC's own Tjommie has
# the same name), and never anything running from anywhere else.
function Stop-Tjommie {
    if ($state['folderOk']) { Stop-ProcessesInFolder $folder | Out-Null }
}

# The app's entries in the registry, as they are right now: the uninstall entry that points into the install folder and the
# install key that names it as its InstallLocation.
function Get-InstallRegistry {
    $registry = [ordered]@{}
    foreach ($view in '64', '32') {
        $entries = @()
        foreach ($name in @(Find-UninstallEntries 'CurrentUser' $uninstallKey $folder $view)) { $entries += , (Get-RegistryKeyFact 'CurrentUser' "$uninstallKey\$name" $view) }
        $registry["hkcuUninstallView$view"] = $entries
    }
    $machine = @()
    foreach ($view in '64', '32') { $machine += @(Find-UninstallEntries 'LocalMachine' $uninstallKey $folder $view) }
    $registry['hklmUninstallEntries'] = $machine
    foreach ($hive in @(@('CurrentUser', 'hkcuInstallKeys'), @('LocalMachine', 'hklmInstallKeys'))) {
        $keys = @()
        foreach ($candidate in @(Get-InstallLocationKeys $hive[0] 'Software' '64')) {
            if (Test-SameFolder ([string]$candidate['installLocation']) $folder) { $keys += , (Get-RegistryKeyFact $hive[0] ([string]$candidate['key']) '64') }
        }
        $registry[$hive[1]] = $keys
    }
    return $registry
}

function Get-InstallShortcuts {
    $folders = Get-ShortcutFolders
    $shortcuts = [ordered]@{}
    foreach ($name in $folders.Keys) { $shortcuts[$name] = @(Get-ShortcutFacts $folders[$name]) }
    return $shortcuts
}

# Runs `$Action` (which starts something and returns its Process) and records every process that appears while it runs, and for
# a few seconds after it ends (an NSIS uninstaller copies itself and lets the first one exit).
function Watch-Processes([scriptblock]$Action, [int]$GraceSeconds = 8) {
    $before = @(Get-Process | ForEach-Object { $_.Id })
    $process = & $Action
    $seen = [ordered]@{}
    $grace = $null
    while ($true) {
        foreach ($fact in (Get-NewProcessFacts $before)) { $seen["$($fact['name'])|$($fact['commandLine'])"] = $fact }
        if ($process.HasExited) {
            if ($null -eq $grace) { $grace = [Diagnostics.Stopwatch]::StartNew() }
            if ($grace.Elapsed.TotalSeconds -ge $GraceSeconds) { break }
        }
        Start-Sleep -Milliseconds 20
    }
    return [ordered]@{ exitCode = $process.ExitCode; processes = @($seen.Values) }
}

# ---- install: the install, before the app has ever run ---------------------------------------------------------------------
Invoke-Phase 'install' $true {
    $inventory = Get-FileInventory $folder
    $state['inventory'] = $inventory
    $programs = Join-Path $env:LOCALAPPDATA 'Programs'
    $facts['install'] = [ordered]@{
        folderAsFound               = $folder
        folderOnDisk                = (Get-Item -LiteralPath $folder).FullName
        programsFolderEntries       = @(Get-ChildItem -LiteralPath $programs -Directory -Force | ForEach-Object { $_.Name })
        exe                         = 'Tjommie.exe'
        exeVersion                  = Get-ExeVersionFact (Join-Path $folder 'Tjommie.exe')
        fileCount                   = $inventory['count']
        fileCountWithoutUninstaller = $inventory['countWithoutUninstaller']
        uninstallers                = $inventory['uninstallers']
        totalBytes                  = $inventory['totalBytes']
        files                       = $inventory['files']
    }
    # Everything that is registered as an uninstall entry, whether or not it is this install's, and every key that records an
    # InstallLocation: taken before anything is looked for, so a run that cannot find the app's entry still shows what is there.
    $facts['uninstallSurvey'] = [ordered]@{
        hkcuView64 = @(Get-UninstallSurvey 'CurrentUser' $uninstallKey '64')
        hkcuView32 = @(Get-UninstallSurvey 'CurrentUser' $uninstallKey '32')
        hklmView64 = @(Get-UninstallSurvey 'LocalMachine' $uninstallKey '64')
        hklmView32 = @(Get-UninstallSurvey 'LocalMachine' $uninstallKey '32')
    }
    $facts['installLocationKeys'] = [ordered]@{
        hkcu = @(Get-InstallLocationKeys 'CurrentUser' 'Software' '64')
        hklm = @(Get-InstallLocationKeys 'LocalMachine' 'Software' '64')
    }
    $facts['registry'] = Get-InstallRegistry
    $facts['shortcuts'] = Get-InstallShortcuts
    foreach ($view in 'hkcuView64', 'hkcuView32', 'hklmView64', 'hklmView32') {
        foreach ($entry in @($facts['uninstallSurvey'][$view])) {
            Write-Output "uninstall entry ($view): $($entry['subKey']) | $($entry['displayName']) | location: $($entry['installLocation']) | $($entry['uninstallString'])"
        }
    }
    Save-Facts

    # What must be there for this to be a record of an install at all.
    $entries = @($facts['registry']['hkcuUninstallView64'])
    if ($entries.Count -ne 1) { throw "Expected exactly one HKCU uninstall entry that points into $folder, found $($entries.Count)." }
    if (@($facts['registry']['hkcuInstallKeys']).Count -ne 1) { throw "Expected exactly one HKCU Software\<GUID> key whose InstallLocation is $folder, found $(@($facts['registry']['hkcuInstallKeys']).Count)." }
    $userShortcuts = @($facts['shortcuts']['userStartMenu']).Count + @($facts['shortcuts']['userDesktop']).Count
    if ($userShortcuts -eq 0) { throw 'The install made no shortcut in the user''s Start menu or on the desktop.' }
    foreach ($place in 'userStartMenu', 'userDesktop') {
        foreach ($shortcut in @($facts['shortcuts'][$place])) {
            if ($shortcut['target'] -like '*Tjommie.exe' -and -not $shortcut['appUserModelId']) { throw "The shortcut $($shortcut['path']) in $place has no AppUserModelID." }
        }
    }
}

# ---- firstStart: what it writes for start at login -------------------------------------------------------------------------
# Electron's `setLoginItemSettings` runs once, on the first start of an installed app, and leaves a marker in its user-data
# folder when it has. The entry is read once the marker is there.
Invoke-Phase 'firstStart' $true {
    $marker = Join-Path $userData 'login-item-set'
    try {
        $null = Start-Process -FilePath (Join-Path $folder 'Tjommie.exe') -PassThru
        $watch = [Diagnostics.Stopwatch]::StartNew()
        while (-not (Test-Path -LiteralPath $marker) -and $watch.Elapsed.TotalSeconds -lt $FirstStartSeconds) { Start-Sleep -Milliseconds 500 }
        $appeared = Test-Path -LiteralPath $marker
        if ($appeared) { Start-Sleep -Seconds 3 }
        $log = Join-Path $dataRoot 'logs\desktop.log'
        $facts['firstStart'] = [ordered]@{
            markerFile      = 'login-item-set'
            markerAppeared  = $appeared
            secondsToMarker = [math]::Round($watch.Elapsed.TotalSeconds, 1)
            run             = Get-RegistryKeyFact 'CurrentUser' $runKey '64'
            startupApproved = Get-RegistryKeyFact 'CurrentUser' $approvedKey '64'
            userDataFolder  = $userData
            userDataEntries = Get-FolderEntries $userData
            dataRootFolder  = $dataRoot
            dataRootEntries = Get-FolderEntries $dataRoot
            desktopLogTail  = $(if (Test-Path -LiteralPath $log) { @(Get-Content -LiteralPath $log -Tail 40) } else { $null })
        }
        Save-Facts
        if (-not $appeared) { throw "The first start did not leave $marker within $FirstStartSeconds seconds." }
        if ($null -eq $facts['firstStart']['run']) { throw 'The first start left no HKCU Run key.' }
        if (@($facts['firstStart']['run']['values'] | Where-Object { [string]$_['data'] -like '*Tjommie.exe*--hidden*' }).Count -eq 0) {
            throw 'No Run value of the first start starts Tjommie.exe with --hidden.'
        }
    } finally {
        Stop-Tjommie
        Start-Sleep -Seconds 2
    }
}

# ---- upgrade: the installer run again over the install ---------------------------------------------------------------------
Invoke-Phase 'upgrade' $false {
    try {
        $registryBefore = Get-InstallRegistry | ConvertTo-Json -Depth 10 -Compress
        $shortcutsBefore = Get-InstallShortcuts | ConvertTo-Json -Depth 10 -Compress
        $installerPath = [string]$install.installerPath
        $second = Watch-Processes { Start-Process -FilePath $installerPath -ArgumentList '/S' -PassThru }
        $previous = -1
        for ($attempt = 0; $attempt -lt 30; $attempt++) {
            Start-Sleep -Seconds 2
            $count = @(Get-ChildItem -LiteralPath $folder -Recurse -File -Force -ErrorAction SilentlyContinue).Count
            if ($count -eq $previous) { break }
            $previous = $count
        }
        Stop-Tjommie
        $after = Get-FileInventory $folder
        $facts['upgrade'] = [ordered]@{
            ok                 = $true
            installerExitCode  = $second['exitCode']
            processes          = $second['processes']
            fileCountAfter     = $after['count']
            fileCountUnchanged = ($after['count'] -eq $state['inventory']['count'])
            registryUnchanged  = ((Get-InstallRegistry | ConvertTo-Json -Depth 10 -Compress) -eq $registryBefore)
            shortcutsUnchanged = ((Get-InstallShortcuts | ConvertTo-Json -Depth 10 -Compress) -eq $shortcutsBefore)
            runEntryStillThere = ($null -ne (Get-RegistryKeyFact 'CurrentUser' $runKey '64'))
            error              = $null
        }
    } catch {
        $facts['upgrade'] = [ordered]@{ ok = $false; error = $_.Exception.Message }
    }
}

# ---- uninstall -------------------------------------------------------------------------------------------------------------
Invoke-Phase 'uninstall' $false {
    try {
        $entries = @($facts['registry']['hkcuUninstallView64'])
        if ($entries.Count -ne 1) { throw 'There is no single uninstall entry to read the uninstall command from.' }
        $entry = $entries[0]
        $quietValue = @($entry['values'] | Where-Object { $_['name'] -eq 'QuietUninstallString' })
        if ($quietValue.Count -eq 0 -or -not $quietValue[0]['data']) { throw 'The uninstall entry has no QuietUninstallString.' }
        $quiet = [string]$quietValue[0]['data']
        # The registry says what to run; it is run only if it is this install's own uninstaller.
        $uninstaller = Get-CommandExecutable $quiet
        if (-not $uninstaller -or -not (Test-PathInFolder $uninstaller $folder)) { throw "The registered uninstaller ($uninstaller) is not inside the install folder, so it is not run." }
        $uninstall = Watch-Processes { Start-Process -FilePath "$env:SystemRoot\System32\cmd.exe" -ArgumentList '/d', '/s', '/c', "`"$quiet`"" -PassThru }
        for ($attempt = 0; $attempt -lt 30; $attempt++) {
            Start-Sleep -Seconds 2
            if (-not (Test-Path -LiteralPath (Join-Path $folder 'Tjommie.exe'))) { break }
        }
        Start-Sleep -Seconds 3
        $left = if (Test-Path -LiteralPath $folder) { Get-FileInventory $folder } else { $null }
        $afterRegistry = Get-InstallRegistry
        $facts['uninstall'] = [ordered]@{
            ok                   = $true
            command              = $quiet
            exitCode             = $uninstall['exitCode']
            processes            = $uninstall['processes']
            installFolderLeft    = (Test-Path -LiteralPath $folder)
            filesLeft            = $(if ($null -eq $left) { 0 } else { $left['count'] })
            uninstallEntriesLeft = @($afterRegistry['hkcuUninstallView64']).Count
            installKeysLeft      = @($afterRegistry['hkcuInstallKeys']).Count
            shortcutsLeft        = (Get-InstallShortcuts)
            run                  = Get-RegistryKeyFact 'CurrentUser' $runKey '64'
            startupApproved      = Get-RegistryKeyFact 'CurrentUser' $approvedKey '64'
            userDataEntries      = Get-FolderEntries $userData
            dataRootEntries      = Get-FolderEntries $dataRoot
            error                = $null
        }
    } catch {
        $facts['uninstall'] = [ordered]@{ ok = $false; error = $_.Exception.Message }
    }
}

$facts['complete'] = ($problems.Count -eq 0)
Save-Facts
Write-Output "Wrote $factsPath"
$upgradeOk = if ($facts.Contains('upgrade')) { $facts['upgrade']['ok'] } else { 'not run' }
$uninstallOk = if ($facts.Contains('uninstall')) { $facts['uninstall']['ok'] } else { 'not run' }
$fileCount = if ($state.ContainsKey('inventory')) { $state['inventory']['count'] } else { 'unknown' }
Write-Output "R1 $($install.version): $fileCount files in $folder; upgrade ok: $upgradeOk; uninstall ok: $uninstallOk"
if ($problems.Count -gt 0) { throw "The facts were written, but a required phase failed: $($problems -join '; ')" }
