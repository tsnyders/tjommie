# T8.4 steps 2 and 3: the real Phase 3 installer, with the real binaries, over the real published R1, and back again, on a GitHub-hosted runner.
# install-latest-r1.ps1 has installed the published R1 (v0.5.1 here: the 0.5.x release that carries the T1.6 export) on this fresh runner and written its
# result (-InstallResult, or $env:R1_INSTALL_RESULT). The production identity is used: a hosted runner is thrown away, so its folders and keys may be used.
#
#   P1  R1 is installed, as recorded
#   P2  a data root is seeded in %LOCALAPPDATA%\Tjommie (the committed database, R1's own daemon started on it once, files R1's browser keeps), and
#       files of known content in %APPDATA%\Tjommie
#   P3  R1 runs, is given the fixture's appearance and paired PCs in its page, and writes its export (T1.6); it registers start at login; it is
#       stopped, and the baseline of the data root is taken
#   P4  the Phase 3 installer runs over it with /S --updated: the payload is the build job's and nothing of R1's is left; the keys and shortcuts are
#       R1's (same values, same AppUserModelID); the start-at-login value is unchanged; the roaming data root lost nothing
#   P5  the start-at-login entry starts the new app, as Windows would at sign-in; it starts its daemon on the same data root; the paired PCs and
#       the appearance R1 exported are imported (T1.6); user_version and row counts are unchanged and nothing of the user's has changed
#   P6  rollback: the published R1 installer runs over Phase 3 with /S; R1's files, keys and shortcuts are back, R1 starts and opens the database,
#       the Electron browser profiles are intact byte for byte, and no migration ran (user_version and tables unchanged)
#
# Evidence: installer-acceptance-production-over-r1.json. This refuses to run anywhere but a hosted runner, and acts only on an install result made
# on this run on a fresh runner.
param(
    [Parameter(Mandatory)][string]$InstallersDir,
    [string]$InstallResult = $env:R1_INSTALL_RESULT,
    [string]$OutDir = '',
    [string]$NewVersion = '0.6.0'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot 'hosted-runner.ps1')
Assert-HostedRunner
. (Join-Path $PSScriptRoot 'accept-lib.ps1')
. (Join-Path $PSScriptRoot 'accept-phase3-lib.ps1')

if (-not $InstallResult) { throw 'No install result: run install-latest-r1.ps1 first, or pass -InstallResult.' }
$r1 = Read-Json $InstallResult
Assert-InstallResultBoundToThisRun $r1
if (-not $OutDir) { $OutDir = Join-Path $env:RUNNER_TEMP 'installer-acceptance' }
Start-Acceptance 'production-over-r1' $OutDir 60

$facts = Read-Json (Join-Path $PSScriptRoot 'r1-install-facts.json')
$table = Read-Json (Join-Path $PSScriptRoot 'identities.json')
$prod = $table.identities.production
$programs = Join-Path $env:LOCALAPPDATA 'Programs'
$folder = [string]$r1.installFolder
$localRoot = Join-Path $env:LOCALAPPDATA 'Tjommie'
$roamingRoot = Join-Path $env:APPDATA 'Tjommie'
$installer = Join-Path $InstallersDir "production\Tjommie-Setup-$NewVersion.exe"
$manifest = Join-Path $InstallersDir 'production\payload-manifest.json'
$fixture = Join-Path (Split-Path -Parent $PSScriptRoot) 'tjommie-rs\crates\desktop-host\tests\fixtures\desktop-ui-export.json'
$baseline = Join-Path $OutDir 'baseline-production-over-r1.json'
$state = @{}
$r1RuntimeOnly = @(Get-R1RuntimeOnlyEntries $facts)

# R1's install must be where this identity installs, or what follows means nothing.
if ((Split-Path -Parent ([IO.Path]::GetFullPath($folder).TrimEnd('\'))) -ine $programs -or (Split-Path -Leaf $folder) -cne $prod.folder) { throw "R1 is installed at $folder, not at $(Join-Path $programs $prod.folder)." }

# Nothing R1 put in the roaming data root is gone, and the sentinel files are byte for byte the same (R1's run-time files are the one thing ignored).
function Test-RoamingKept([string]$Label) {
    if (-not $state.ContainsKey('roamingBefore')) { Assert-Check $false "$Label`: there is no snapshot of the roaming data root to compare with (P3 did not finish)"; return }
    $differences = @(Compare-Snapshot $state['roamingBefore'] (Get-TreeSnapshot $roamingRoot) -IgnoreAdded -IgnorePaths $r1RuntimeOnly | Where-Object { $_ -like 'missing:*' -or $_ -like 'changed: sentinel\*' })
    Assert-Check ($differences.Count -eq 0) "$Label`: the roaming data root lost nothing, and its sentinel files are unchanged" $differences
}

# Starts an app the way Windows starts a Run value at sign-in: through cmd, with the value's own text.
function Start-RunValue([string]$Command) {
    $process = Start-Process -FilePath "$env:SystemRoot\System32\cmd.exe" -ArgumentList '/d', '/s', '/c', "`"$Command`"" -PassThru -WindowStyle Hidden
    $null = $process.Handle
    [void]$process.WaitForExit(30000)
}

Invoke-Step 'P0 the build job''s output is here' {
    foreach ($needed in $installer, $manifest, $fixture, [string]$r1.installerPath) { Assert-Check (Test-Path -LiteralPath $needed -PathType Leaf) "P0: $needed exists" $needed }
}

Invoke-Step 'P1 R1 is installed, as recorded' {
    $state['r1Files'] = @(Get-InstalledFiles $folder)
    Assert-Check (Test-Path -LiteralPath (Join-Path $folder "Uninstall $($prod.product).exe")) 'P1: R1''s uninstaller is in the install folder'
    Assert-Check (Test-Path -LiteralPath (Join-Path $folder $prod.exe)) 'P1: R1''s Tjommie.exe is in the install folder'
    Assert-Check ($state['r1Files'].Count -gt 100) 'P1: R1 is a full Electron install' $state['r1Files'].Count
    Assert-Check ([string](Get-InstallLocation $prod.guid) -ceq $folder) 'P1: R1 recorded its own folder as InstallLocation' (Get-InstallLocation $prod.guid)
    Assert-Check (@(Get-RunningFrom $folder).Count -eq 0) 'P1: nothing is running yet'
    Add-Note "P1: R1 $($r1.version) is installed in $folder with $($state['r1Files'].Count) files" $state['r1Files'].Count
}

Invoke-Step 'P2 seed the data roots' {
    $seeded = Invoke-Phase3Driver @('seed', '--root', $localRoot, '--install', $folder)
    Assert-Check ($seeded['exitCode'] -eq 0) 'P2: the local data root was seeded by R1''s own daemon' @($seeded['lines'] | Select-Object -Last 8)
    [void](Add-Sentinels $roamingRoot)
    Assert-Check (Test-Path -LiteralPath (Join-Path $localRoot 'data\tjommie.db')) 'P2: the database is there'
    Assert-Check (Test-Path -LiteralPath (Join-Path $localRoot 'data\native-browser')) 'P2: the Electron browser profiles are there (data\native-browser)'
}

Invoke-Step 'P3 R1 runs and exports the paired PCs and the appearance (T1.6)' {
    $exported = Invoke-Phase3Driver @('r1-export', '--install', $folder, '--fixture', $fixture)
    Assert-Check ($exported['exitCode'] -eq 0) 'P3: R1 wrote an export that says what its page held' @($exported['lines'] | Select-Object -Last 8)
    Assert-Check (Test-Path -LiteralPath (Join-Path $localRoot 'data\desktop-ui-export.json')) 'P3: data\desktop-ui-export.json exists'
    # R1 sets start at login on its first packaged run.
    $marker = Join-Path $roamingRoot 'login-item-set'
    Assert-Check (Wait-Until { Test-Path -LiteralPath $marker } 30) 'P3: R1 wrote its start-at-login marker' $marker
    $state['runBefore'] = Get-RunValue $prod.appId
    Assert-Check ([string]$state['runBefore'] -ceq "`"$(Join-Path $folder $prod.exe)`" --hidden") 'P3: R1 wrote its start-at-login value' $state['runBefore']
    Stop-ProcessesInFolder $folder | Out-Null
    Assert-Check (Wait-Until { @(Get-RunningFrom $folder).Count -eq 0 } 30) 'P3: R1 is stopped'
    $made = Invoke-Phase3Driver @('snapshot', '--root', $localRoot, '--out', $baseline)
    Assert-Check ($made['exitCode'] -eq 0) 'P3: the baseline of the data root is taken, with R1 stopped' @($made['lines'] | Select-Object -Last 4)
    $state['roamingBefore'] = Get-TreeSnapshot $roamingRoot
}

Invoke-Step 'P4 the Phase 3 installer takes over from R1' {
    foreach ($needed in 'roamingBefore') {
        if (-not $state.ContainsKey($needed)) { Assert-Check $false "P4 did not run: P3 did not finish, so '$needed' was never recorded"; return }
    }
    $update = Invoke-Setup $installer '/S --updated' -Watch
    Assert-Check ($update['exitCode'] -eq 0) 'P4: the installer exits 0' $update['exitCode']
    $old = @($update['processes'] | Where-Object { $_['name'] -eq 'old-uninstaller.exe' -and $_['commandLine'] })
    Assert-Check ($old.Count -ge 1) 'P4: R1''s own uninstaller was run, as old-uninstaller.exe' @($update['processes'] | ForEach-Object { $_['name'] })
    Assert-Check (@(Get-RunningFrom $folder).Count -eq 0) 'P4: nothing runs from the install folder'
    Test-InstalledPayload $folder $manifest $prod.product 'P4'
    $leftovers = @($state['r1Files'] | Where-Object { (Test-Path -LiteralPath (Join-Path $folder $_)) -and $_ -notmatch '^Uninstall ' -and $_ -ne $prod.exe })
    Assert-Check ($leftovers.Count -eq 0) 'P4: no file of R1''s is left' @($leftovers | Select-Object -First 10)
    Test-InstalledAsR1Did $facts $folder $NewVersion 'P4'
    Test-ShortcutsAsR1Had $facts $folder $prod.product $prod.exe $prod.appId 'P4'
    $run = Get-RunValue $prod.appId
    Assert-Check ([string]$run -ceq [string]$state['runBefore']) 'P4: the start-at-login value R1 wrote is unchanged' $run
    Test-RoamingKept 'P4'
}

Invoke-Step 'P5 the start-at-login entry starts the new app, which carries R1''s data over' {
    $run = [string](Get-RunValue $prod.appId)
    $exe = Get-CommandExecutable $run
    Assert-Check ($exe -and (Test-SameLongFolder (Split-Path -Parent $exe) $folder)) 'P5: the start-at-login value starts the exe in the install folder' $run
    Start-RunValue $run
    $newExe = Join-Path $folder $prod.exe
    $started = Wait-Until { @(Get-ProcessesOfExe $newExe | Where-Object { [string]$_.CommandLine -match '--hidden' -and [string]$_.CommandLine -notmatch '--type=' }).Count -ge 1 } 120
    Assert-Check $started 'P5: the new app runs, started the way sign-in starts it (--hidden)' @(Get-ProcessesOfExe $newExe | ForEach-Object { [string]$_.CommandLine })
    $daemon = Join-Path $folder 'resources\daemon\tjommie-daemon.exe'
    Assert-Check (Wait-Until { @(Get-ProcessesOfExe $daemon).Count -ge 1 } 120) 'P5: the new app''s daemon runs from the install folder' $daemon
    Test-CarryOver $localRoot $fixture 'P5'
    Start-Sleep -Seconds 5
    Test-DataRootAsBaseline $localRoot $baseline (Join-Path $OutDir 'data-root-production-over-r1-new-app.json') 'P5'
    Save-AppLogs $localRoot $OutDir 'production-new-app'
}

Invoke-Step 'P6 rolled back: R1 over Phase 3' {
    Stop-ProcessesInFolder $folder | Out-Null
    Assert-Check (Wait-Until { @(Get-RunningFrom $folder).Count -eq 0 } 60) 'P6: the new app is stopped first'
    $state['phase3Files'] = @(Get-InstalledFiles $folder)
    $back = Invoke-Setup ([string]$r1.installerPath) '/S' -Watch
    Assert-Check ($back['exitCode'] -eq 0) 'P6: R1''s installer exits 0 over Phase 3' $back['exitCode']
    Add-Note 'P6: the programs R1''s installer started while it ran' @($back['processes'] | ForEach-Object { "$($_['name']) $($_['commandLine'])" })
    $files = @(Get-InstalledFiles $folder)
    Assert-Check (($files -join '|') -ceq ($state['r1Files'] -join '|')) 'P6: the install holds the files R1 had, and none of Phase 3''s' @{ files = $files.Count; r1 = $state['r1Files'].Count; extra = @($files | Where-Object { $state['r1Files'] -notcontains $_ }); missing = @($state['r1Files'] | Where-Object { $files -notcontains $_ }) }
    Assert-Check (-not (Test-Path -LiteralPath (Join-Path $folder 'resources\pdfium.dll'))) 'P6: Phase 3''s pdfium.dll is gone'
    Assert-Check (-not (Test-Path -LiteralPath (Join-Path $folder 'third-party-notices.txt'))) 'P6: Phase 3''s notices are gone'
    Test-InstalledAsR1Did $facts $folder ([string]$r1.version) 'P6'
    Test-ShortcutsAsR1Had $facts $folder $prod.product $prod.exe $prod.appId 'P6'
    Assert-Check ([string](Get-RunValue $prod.appId) -ceq [string]$state['runBefore']) 'P6: the start-at-login value is R1''s' (Get-RunValue $prod.appId)
    # R1 starts and opens the database: its own daemon, on the data root Phase 3 used.
    $log = Join-Path $localRoot 'logs\desktop.log'
    $listeningBefore = if (Test-Path -LiteralPath $log) { @(Select-String -LiteralPath $log -Pattern 'Tjommie daemon listening on').Count } else { 0 }
    $r1Exe = Join-Path $folder $prod.exe
    $app = Start-Process -FilePath $r1Exe -PassThru
    $null = $app.Handle
    $opened = Wait-Until { (Test-Path -LiteralPath $log) -and (@(Select-String -LiteralPath $log -Pattern 'Tjommie daemon listening on').Count -gt $listeningBefore) } 120
    Assert-Check $opened 'P6: R1 started and its daemon opened the database (a new "daemon listening" line in the log)' $log
    Assert-Check (@(Get-RunningFrom $folder).Count -ge 2) 'P6: R1 (the app and its daemon, at least) is running from the install folder' @(Get-RunningFrom $folder).Count
    Start-Sleep -Seconds 5
    # No migration ran: user_version and every table are as they were, and R1's browser profiles are byte for byte what they were.
    Test-DataRootAsBaseline $localRoot $baseline (Join-Path $OutDir 'data-root-production-over-r1-rollback.json') 'P6'
    Test-RoamingKept 'P6'
    Save-AppLogs $localRoot $OutDir 'production-rollback'
}

Invoke-Step 'P7 stop what this run started' {
    Stop-ProcessesInFolder $folder | Out-Null
    Assert-Check (Wait-Until { @(Get-RunningFrom $folder).Count -eq 0 } 60) 'P7: nothing runs from the install folder afterwards'
}

Complete-Acceptance
