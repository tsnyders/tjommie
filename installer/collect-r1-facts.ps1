# Records what an installed R1 looks like (T8.1), as JSON, so T8.2's NSIS installer is written to facts and not to guesses.
# It runs after install-latest-r1.ps1, on the same GitHub-hosted runner, and refuses to run anywhere else: it starts the
# app, runs the installer again over itself and then uninstalls, all of which change an install and its registry keys.
#
#   1. The install as the installer left it, before the app has ever run: the folder and its files, the uninstall entry and
#      the install key in the registry, the shortcuts and their AppUserModelID.
#   2. The first start: the app starts once, and the start-at-login entry it writes (HKCU Run and StartupApproved) is read
#      once it has written it, with what the app leaves in its two data folders.
#   3. The upgrade: the same installer runs again over the install, and the processes it starts, with their command lines,
#      show how an installer runs the old uninstaller.
#   4. The uninstall: the registered quiet uninstall command runs, and what is left behind is recorded.
#
# Parts 1 and 2 are required: any gap fails the run. Parts 3 and 4 are best effort: if one fails, the reason is recorded and
# the rest of the file is still written.
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

$install = Get-Content -LiteralPath $InstallResult -Raw | ConvertFrom-Json
$folder = [string]$install.installFolder
if (-not (Test-Path -LiteralPath (Join-Path $folder 'Tjommie.exe'))) { throw "No Tjommie.exe in $folder." }
# Everything this script starts, stops or uninstalls is scoped to this one folder, which must be an install under the user's
# Programs folder: a per-user NSIS install, nowhere else.
if ((Split-Path -Parent ([IO.Path]::GetFullPath($folder).TrimEnd('\'))) -ine (Join-Path $env:LOCALAPPDATA 'Programs')) { throw "$folder is not an install folder under %LOCALAPPDATA%\Programs." }
Assert-NarrowFolder $folder

$productName = 'Tjommie'
$uninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall'
$runKey = 'Software\Microsoft\Windows\CurrentVersion\Run'
$approvedKey = 'Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run'
$userData = Join-Path $env:APPDATA $productName
$dataRoot = Join-Path $env:LOCALAPPDATA $productName

# Stops what runs from this run's install folder, and their children; never anything by its name (a real PC's own Tjommie has
# the same name), and never anything running from anywhere else.
function Stop-Tjommie {
    Stop-ProcessesInFolder $folder | Out-Null
}

# The app's entries in the registry and the shortcuts, as they are right now.
function Get-InstallRegistry {
    $facts = [ordered]@{}
    foreach ($view in '64', '32') {
        $names = @(Find-UninstallEntries 'CurrentUser' $uninstallKey $productName $folder $view)
        $entries = @()
        foreach ($name in $names) { $entries += , (Get-RegistryKeyFact 'CurrentUser' "$uninstallKey\$name" $view) }
        $facts["hkcuUninstallView$view"] = $entries
    }
    $machine = @()
    foreach ($view in '64', '32') { $machine += @(Find-UninstallEntries 'LocalMachine' $uninstallKey $productName $folder $view) }
    $facts['hklmUninstallEntries'] = $machine
    $names = @(Find-UninstallEntries 'CurrentUser' $uninstallKey $productName $folder '64')
    $installKeys = @()
    $hklmInstallKeys = @()
    foreach ($name in $names) {
        $key = Get-RegistryKeyFact 'CurrentUser' "Software\$name" '64'
        if ($null -ne $key) { $installKeys += , $key }
        $other = Get-RegistryKeyFact 'LocalMachine' "Software\$name" '64'
        if ($null -ne $other) { $hklmInstallKeys += , $other }
    }
    $facts['hkcuInstallKeys'] = $installKeys
    $facts['hklmInstallKeys'] = $hklmInstallKeys
    return $facts
}

function Get-InstallShortcuts {
    $folders = Get-ShortcutFolders
    $facts = [ordered]@{}
    foreach ($name in $folders.Keys) { $facts[$name] = @(Get-ShortcutFacts $folders[$name]) }
    return $facts
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

# ---- 1. The install, before the app has ever run ----------------------------------------------------------------------
$facts = [ordered]@{ schema = 1 }
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
$inventory = Get-FileInventory $folder
$facts['install'] = [ordered]@{
    folder              = $folder
    exe                 = 'Tjommie.exe'
    exeVersion          = Get-ExeVersionFact (Join-Path $folder 'Tjommie.exe')
    fileCount           = $inventory['count']
    fileCountWithoutUninstaller = $inventory['countWithoutUninstaller']
    uninstallers        = $inventory.uninstallers
    totalBytes          = $inventory.totalBytes
    files               = $inventory.files
}
$facts['registry'] = Get-InstallRegistry
$facts['shortcuts'] = Get-InstallShortcuts

# What must be there for this to be a record of an install at all.
if (@($facts['registry']['hkcuUninstallView64']).Count -ne 1) { throw "Expected exactly one HKCU uninstall entry for $productName, found $(@($facts['registry']['hkcuUninstallView64']).Count)." }
if (@($facts['registry']['hkcuInstallKeys']).Count -ne 1) { throw 'Expected one HKCU Software\<GUID> install key.' }
$userShortcuts = @($facts['shortcuts']['userStartMenu']).Count + @($facts['shortcuts']['userDesktop']).Count
if ($userShortcuts -eq 0) { throw 'The install made no shortcut in the user''s Start menu or on the desktop.' }
foreach ($place in 'userStartMenu', 'userDesktop') {
    foreach ($shortcut in @($facts['shortcuts'][$place])) {
        if ($shortcut.target -like '*Tjommie.exe' -and -not $shortcut.appUserModelId) { throw "The shortcut $($shortcut.path) in $place has no AppUserModelID." }
    }
}

# ---- 2. The first start: what it writes for start at login ------------------------------------------------------------
# Electron's `setLoginItemSettings` runs once, on the first start of an installed app, and leaves a marker in its user-data
# folder when it has. The entry is read once the marker is there.
$marker = Join-Path $userData 'login-item-set'
$started = Start-Process -FilePath (Join-Path $folder 'Tjommie.exe') -PassThru
$watch = [Diagnostics.Stopwatch]::StartNew()
while (-not (Test-Path -LiteralPath $marker) -and $watch.Elapsed.TotalSeconds -lt $FirstStartSeconds) { Start-Sleep -Milliseconds 500 }
$secondsToMarker = [math]::Round($watch.Elapsed.TotalSeconds, 1)
if (-not (Test-Path -LiteralPath $marker)) {
    $log = Join-Path $dataRoot 'logs\desktop.log'
    if (Test-Path -LiteralPath $log) { Write-Output '--- desktop.log (last lines) ---'; Get-Content -LiteralPath $log -Tail 40 }
    Write-Output "--- $userData ---"; Get-ChildItem -LiteralPath $userData -Force -ErrorAction SilentlyContinue | ForEach-Object { $_.Name }
    Stop-Tjommie
    throw "The first start did not leave $marker within $FirstStartSeconds seconds."
}
Start-Sleep -Seconds 3
$facts['firstStart'] = [ordered]@{
    markerFile       = 'login-item-set'
    secondsToMarker  = $secondsToMarker
    run              = Get-RegistryKeyFact 'CurrentUser' $runKey '64'
    startupApproved  = Get-RegistryKeyFact 'CurrentUser' $approvedKey '64'
    userDataFolder   = $userData
    userDataEntries  = Get-FolderEntries $userData
    dataRootFolder   = $dataRoot
    dataRootEntries  = Get-FolderEntries $dataRoot
}
Stop-Tjommie
Start-Sleep -Seconds 2
if ($null -eq $facts['firstStart']['run']) { throw 'The first start left no HKCU Run key.' }
if (@($facts['firstStart']['run']['values'] | Where-Object { [string]$_.data -like '*Tjommie.exe*--hidden*' }).Count -eq 0) {
    throw 'No Run value of the first start starts Tjommie.exe with --hidden.'
}

# ---- 3. The upgrade: the installer run again over the install -----------------------------------------------------------
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
        ok                    = $true
        installerExitCode     = $second.exitCode
        processes             = $second.processes
        fileCountAfter        = $after['count']
        fileCountUnchanged    = ($after['count'] -eq $inventory['count'])
        registryUnchanged     = ((Get-InstallRegistry | ConvertTo-Json -Depth 10 -Compress) -eq $registryBefore)
        shortcutsUnchanged    = ((Get-InstallShortcuts | ConvertTo-Json -Depth 10 -Compress) -eq $shortcutsBefore)
        runEntryStillThere    = ($null -ne (Get-RegistryKeyFact 'CurrentUser' $runKey '64'))
        error                 = $null
    }
} catch {
    $facts['upgrade'] = [ordered]@{ ok = $false; error = $_.Exception.Message }
}

# ---- 4. The uninstall ----------------------------------------------------------------------------------------------------
try {
    $entry = @($facts['registry']['hkcuUninstallView64'])[0]
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
        exitCode             = $uninstall.exitCode
        processes            = $uninstall.processes
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

$path = Join-Path $OutDir 'r1-install-facts.json'
$facts | ConvertTo-Json -Depth 20 | Set-Content -LiteralPath $path -Encoding UTF8
Copy-Item -LiteralPath (Join-Path (Split-Path -Parent $InstallResult) 'latest.yml') -Destination (Join-Path $OutDir 'latest.yml')
Write-Output "Wrote $path"
Write-Output "R1 $($install.version): $($inventory['count']) files in $folder; upgrade ok: $($facts['upgrade']['ok']); uninstall ok: $($facts['uninstall']['ok'])"
