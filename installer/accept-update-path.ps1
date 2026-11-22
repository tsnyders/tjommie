# T8.4 step 1: the update path, through the real mechanism, on the test identity, on a fresh GitHub-hosted runner.
#
# The update-test build of R1 (the Electron app, identity "updatetest") is installed where the Phase 3 updatetest installer installs by default, on
# a data root that holds a seeded database. It is started with TJOMMIE_UPDATE_FEED pointing at a feed on this machine that serves the Phase 3
# updatetest installer and its latest.yml, and it updates itself: electron-updater downloads the installer, and Restart to update quits R1 and runs the
# installer silently (--updated /S --force-run). Then:
#
#   U1  R1 is installed, as its own updater will find it (InstallLocation, uninstall entry, shortcut with its AppUserModelID)
#   U2  a data root is seeded (the committed database, R1's own daemon started on it once, files R1's browser keeps)
#   U3  R1 finds the update in the feed, downloads it and quits for it; the baseline of the data root is taken just before, with a pending notice planted
#   U4  the new app runs, started with --updated, and what is installed is the new payload and nothing of R1's
#   U5  the uninstall entry points at the new uninstaller (a different file from R1's) and says the new version
#   U6  the shortcut's AppUserModelID is the one R1's shortcut had
#   U7  the data root: user_version and row counts unchanged, nothing gone, nothing of the user's changed, additions only where the apps write
#   U8  the new app acknowledged the pending notice, which it does only after the OS has taken the toast: a notification posted
#
# The data root is TJOMMIE_HOME, a folder under %TEMP%: the test identity never touches %LOCALAPPDATA%\Tjommie. Evidence: installer-acceptance-update-path.json.
# This refuses to run anywhere but a hosted runner, and only on a fresh one.
param(
    [Parameter(Mandatory)][string]$InstallersDir,
    [string]$OutDir = '',
    [string]$NewVersion = '0.6.0',
    [string]$R1TestVersion = '0.5.1-test'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot 'hosted-runner.ps1')
Assert-HostedRunner
Assert-FreshRunner
. (Join-Path $PSScriptRoot 'accept-lib.ps1')
. (Join-Path $PSScriptRoot 'accept-phase3-lib.ps1')

if (-not $OutDir) { $OutDir = Join-Path $env:RUNNER_TEMP 'installer-acceptance' }
Start-Acceptance 'update-path' $OutDir 45

$table = Read-Json (Join-Path $PSScriptRoot 'identities.json')
$test = $table.identities.updatetest
# %TEMP% exactly as the environment spells it: R1's update-test installer holds the install folder's name as the build spelled it (from the same
# variable), and compares it with the folder it is run for. The workflow sets it to the long form, so the installer script, which resolves
# paths to the long form, agrees with it.
$temp = $env:TEMP.TrimEnd('\')
# Where R1's update test installs, and where the Phase 3 updatetest installer installs by default: two levels under %TEMP%, named for the product.
$testRoot = Join-Path $temp 'tjommie-updatetest'
$installDir = Join-Path $testRoot $test.product
$dataRoot = Join-Path $temp 'tjommie-updatetest-data'
$userData = Join-Path $temp 'tjommie-updatetest-electron'
$r1Installer = Join-Path $InstallersDir "r1-updatetest\Tjommie-Setup-$R1TestVersion.exe"
$feedDir = Join-Path $InstallersDir 'update'
$manifest = Join-Path $InstallersDir 'payload-manifest-updatetest.json'
$baseline = Join-Path $OutDir 'baseline-update-path.json'
$report = Join-Path $OutDir 'data-root-update-path.json'
$newExe = Join-Path $installDir $test.exe
$newUninstaller = Join-Path $installDir "Uninstall $($test.product).exe"
$state = @{}

Invoke-Step 'U0 the build job''s output is here' {
    Assert-Check ($temp -ieq (Get-LongPath $temp)) 'U0: %TEMP% is spelled in its long form, as the build job spelled it (no 8.3 alias for R1''s guards to trip on)' $temp
    foreach ($needed in $r1Installer, (Join-Path $feedDir 'latest.yml'), (Join-Path $feedDir "Tjommie-Setup-$NewVersion.exe"), $manifest) {
        Assert-Check (Test-Path -LiteralPath $needed -PathType Leaf) "U0: $needed exists" $needed
    }
    $yml = Get-Content -LiteralPath (Join-Path $feedDir 'latest.yml') -Raw
    Assert-Check ($yml -match "(?m)^version: $([regex]::Escape($NewVersion))\r?$") "U0: the feed's latest.yml is for version $NewVersion" $yml
}

Invoke-Step 'U1 R1 (the update-test build) is installed where its updater will find it' {
    # R1's own installer insists that the folder above the install folder exists.
    New-Item -ItemType Directory -Force -Path $testRoot | Out-Null
    $installed = Invoke-Setup $r1Installer "/S /D=$installDir"
    Assert-Check ($installed['exitCode'] -eq 0) 'U1: R1''s update-test installer exits 0' $installed['exitCode']
    $there = Wait-Until { Test-Path -LiteralPath (Join-Path $installDir $test.exe) } 60
    Assert-Check $there 'U1: R1''s exe is in the install folder' $installDir
    # A one-click install may start the app; only what runs from this folder is stopped, by path.
    Stop-ProcessesInFolder $installDir | Out-Null
    $state['r1Files'] = @(Get-InstalledFiles $installDir)
    Assert-Check ($state['r1Files'].Count -gt 100) 'U1: R1 is a full Electron install' $state['r1Files'].Count
    Assert-Check (Test-Path -LiteralPath $newUninstaller) 'U1: R1''s uninstaller is in the install folder'
    $state['r1UninstallerHash'] = (Get-FileHash -LiteralPath $newUninstaller -Algorithm SHA256).Hash
    Assert-Check (Test-SameLongFolder ([string](Get-InstallLocation $test.guid)) $installDir) 'U1: R1 recorded its folder as InstallLocation' (Get-InstallLocation $test.guid)
    $state['uninstallBefore'] = Get-RegistryKeyFact 'CurrentUser' (Get-UninstallKeyName $test.guid) '64'
    Assert-Check ($null -ne $state['uninstallBefore']) 'U1: R1 registered its uninstall entry under the identity''s GUID'
    $folders = Get-ShortcutFolders
    $state['shortcutBefore'] = @(Get-ShortcutFacts $folders['userStartMenu'] | Where-Object { $_['path'] -eq "$($test.product).lnk" })
    if ($state['shortcutBefore'].Count -ge 1) { Add-Note "U1: R1's shortcut carries the AppUserModelID $($state['shortcutBefore'][0]['appUserModelId'])" $state['shortcutBefore'][0]['appUserModelId'] }
    else { Add-Note 'U1: R1''s update-test install made no Start-menu shortcut, so there is no AppUserModelID of R1''s to compare (U6 still checks the new one)' }
}

Invoke-Step 'U2 seed a data root, which R1''s own daemon starts on once' {
    $seeded = Invoke-Phase3Driver @('seed', '--root', $dataRoot, '--install', $installDir)
    Assert-Check ($seeded['exitCode'] -eq 0) 'U2: the data root was seeded' @($seeded['lines'] | Select-Object -Last 8)
    Assert-Check (Test-Path -LiteralPath (Join-Path $dataRoot 'data\tjommie.db')) 'U2: the database is there'
}

Invoke-Step 'U3 R1 finds the update in the local feed, downloads it and quits to install it' {
    $updated = Invoke-Phase3Driver @('r1-update', '--install', $installDir, '--feed', $feedDir, '--root', $dataRoot, '--user-data', $userData, '--baseline', $baseline, '--expect-version', $NewVersion)
    Assert-Check ($updated['exitCode'] -eq 0) 'U3: R1 downloaded the update from the feed and quit for it' @($updated['lines'] | Select-Object -Last 10)
    Assert-Check (Test-Path -LiteralPath $baseline) 'U3: the baseline of the data root was taken just before the restart' $baseline
}

Invoke-Step 'U4 the new app runs, started with --updated, and R1 is gone' {
    # The installer's hand-over: R1's processes closed, its uninstaller run, the payload copied, the app started with --updated.
    $started = Wait-Until { @(Get-ProcessesOfExe $newExe | Where-Object { [string]$_.CommandLine -match '--updated' -and [string]$_.CommandLine -notmatch '--type=' }).Count -ge 1 } 240
    Assert-Check $started 'U4: the new exe runs from the install folder, started with --updated' @(Get-ProcessesOfExe $newExe | ForEach-Object { [string]$_.CommandLine })
    $state['newMain'] = @(Get-ProcessesOfExe $newExe | Where-Object { [string]$_.CommandLine -match '--updated' })
    # Telling it from R1: the payload has no Electron file, and is exactly the build job's.
    Test-InstalledPayload $installDir $manifest $test.product 'U4'
    $leftovers = @($state['r1Files'] | Where-Object { (Test-Path -LiteralPath (Join-Path $installDir $_)) -and $_ -notmatch '^Uninstall ' -and $_ -ne $test.exe })
    Assert-Check ($leftovers.Count -eq 0) 'U4: no file of R1''s is left' @($leftovers | Select-Object -First 10)
}

Invoke-Step 'U5 the uninstall entry points at the new uninstaller' {
    $after = Get-RegistryKeyFact 'CurrentUser' (Get-UninstallKeyName $test.guid) '64'
    Assert-Check ($null -ne $after) 'U5: the uninstall entry is there'
    if ($null -eq $after) { return }
    # The installer writes the folder in its long form, and this runner's temp folder is spelled with an 8.3 alias, so both spellings are tried.
    $spellings = @($newUninstaller, (Get-LongPath $newUninstaller))
    foreach ($name in 'UninstallString', 'QuietUninstallString') {
        $value = [string](Get-FactValue $after $name)
        $names = @($spellings | Where-Object { $value.IndexOf($_, [StringComparison]::OrdinalIgnoreCase) -ge 0 })
        Assert-Check ($names.Count -ge 1) "U5: $name runs the new uninstaller in the install folder" $value
    }
    Assert-Check ([string](Get-FactValue $after 'DisplayVersion') -ceq $NewVersion) 'U5: DisplayVersion is the new version' (Get-FactValue $after 'DisplayVersion')
    Assert-Check (Test-SameLongFolder ([string](Get-FactValue $after 'InstallLocation')) ([string](Get-FactValue $state['uninstallBefore'] 'InstallLocation'))) 'U5: InstallLocation is where R1 had it' (Get-FactValue $after 'InstallLocation')
    Assert-Check (Test-SameLongFolder ([string](Get-InstallLocation $test.guid)) $installDir) 'U5: the install key still names the install folder' (Get-InstallLocation $test.guid)
    Assert-Check ((Test-Path -LiteralPath $newUninstaller) -and ((Get-FileHash -LiteralPath $newUninstaller -Algorithm SHA256).Hash -ne $state['r1UninstallerHash'])) 'U5: the uninstaller in the folder is the new one, not R1''s' $state['r1UninstallerHash']
}

Invoke-Step 'U6 the shortcut carries the same AppUserModelID' {
    $folders = Get-ShortcutFolders
    $now = @(Get-ShortcutFacts $folders['userStartMenu'] | Where-Object { $_['path'] -eq "$($test.product).lnk" })
    Assert-Check ($now.Count -eq 1) "U6: the Start menu has $($test.product).lnk" $now.Count
    if ($now.Count -ne 1) { return }
    Assert-Check ($now[0]['appUserModelId'] -ceq $test.appId) "U6: its AppUserModelID is $($test.appId)" $now[0]['appUserModelId']
    if ($state['shortcutBefore'].Count -ge 1) { Assert-Check ($now[0]['appUserModelId'] -ceq $state['shortcutBefore'][0]['appUserModelId']) 'U6: it is the AppUserModelID R1''s shortcut had' @{ before = $state['shortcutBefore'][0]['appUserModelId']; after = $now[0]['appUserModelId'] } }
    Assert-Check ($now[0]['target'] -ieq (Get-LongPath $newExe) -or $now[0]['target'] -ieq $newExe) 'U6: and it starts the new exe' $now[0]['target']
}

Invoke-Step 'U7 the data root is what R1 left, and the database is as it was' {
    # The new app's own daemon opens the database; wait for it so what is compared is what the new app does, not nothing.
    $daemon = Join-Path $installDir 'resources\daemon\tjommie-daemon.exe'
    $running = Wait-Until { @(Get-ProcessesOfExe $daemon).Count -ge 1 } 120
    Assert-Check $running 'U7: the new app''s daemon runs from the install folder' $daemon
    Start-Sleep -Seconds 5
    Test-DataRootAsBaseline $dataRoot $baseline $report 'U7' @('routine_failure_notifications')
}

Invoke-Step 'U8 the new app showed the pending notice, and only then acknowledged it' {
    # A routine failure stays in the daemon until it is acknowledged, and the app acknowledges only after the OS has taken the toast with
    # notifications enabled (T6.2). So the notice being gone is what "a notification posted" looks like from here.
    $gone = $false
    for ($attempt = 0; $attempt -lt 30 -and -not $gone; $attempt++) {
        $notice = Invoke-Phase3Driver @('notice', '--root', $dataRoot)
        $gone = ($null -ne $notice['json']) -and ([int]$notice['json'].pending -eq 0)
        if (-not $gone) { Start-Sleep -Seconds 3 }
    }
    Assert-Check $gone 'U8: the planted routine-failure notice was acknowledged by the new app, after the OS took the toast' $null
    if (-not $gone) {
        $log = Join-Path $dataRoot 'logs\desktop.log'
        if (Test-Path -LiteralPath $log) { Add-Note 'U8: the new app''s log, last lines (look for "notification")' @(Get-Content -LiteralPath $log -Tail 60) }
    }
}

Invoke-Step 'U9 keep the evidence, and stop what this run started' {
    Save-AppLogs $dataRoot $OutDir 'update-path'
    Stop-ProcessesInFolder $installDir | Out-Null
    Assert-Check (@(Get-RunningFrom $installDir).Count -eq 0) 'U9: nothing runs from the install folder afterwards'
}

Complete-Acceptance
