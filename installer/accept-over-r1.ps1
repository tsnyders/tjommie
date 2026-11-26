# Installer acceptance over a real R1, on a GitHub-hosted runner (T8.2). install-latest-r1.ps1 has installed the latest published R1
# silently (its result file is -InstallResult, or $env:R1_INSTALL_RESULT); this starts that R1 once, so its Electron processes and its
# daemon are running on a data root that holds files of known content, then runs the Tjommie installer (stub payload) over it with
# /S --updated, as the updater does, and checks the hand-over:
#
#   O1  R1 is installed, as recorded
#   O1b the installer refuses (exit 7) to go near R1 when HKCU\Software\<GUID>\InstallLocation names C:\, a scratch folder or a sibling
#       folder: R1's own uninstaller deletes the folder that value names, so it must never be run. R1's files, those folders and the
#       value itself are untouched, and R1's uninstaller is never started. (R1 is not running yet.)
#   O2  R1 runs (its login-item marker appears), with two programs of R1's kind running elsewhere: one with the same name from another
#       folder, and one from a folder whose path merely starts with R1's (...\Programs\tjommie-sibling)
#   O3  the installer: closes R1's processes by path; runs R1's own uninstaller from a temp copy with the arguments R1's upgrade uses;
#       leaves the folder, keys and shortcuts as R1 had them; removes every R1 file; keeps the start-at-login value R1 wrote, which now
#       starts the new exe; and loses nothing from either data root. It also RECORDS, as notes and never as promises, which other
#       programs were closed (see below).
#   O4  the new uninstaller removes the install and leaves both data roots alone
#
# What R1's own uninstaller does to other programs is R1's, not this installer's, and it is recorded here rather than assumed. In
# electron-builder 26.15.3's template (allowOnlyOneInstallerInstance.nsh), when PowerShell is usable it stops every process whose path
# STARTS WITH the install folder's path, with no trailing separator (so one running from ...\Programs\tjommie-rs or
# ...\tjommie-updatetest too); otherwise it stops every Tjommie.exe of the user by image name (the T8.1 facts recorded this path on a
# runner). This installer's own stop is by path with a trailing separator and leaves both alone. The notes in the evidence file say
# which happened here, and the owner checklist says to close the dev and update-test apps before updating production.
#
# This is the one place the real R1 is used. It refuses to run anywhere but a hosted runner. Evidence: installer-acceptance-over-r1.json.
param(
    [Parameter(Mandatory)][string]$InstallersDir,
    [string]$InstallResult = $env:R1_INSTALL_RESULT,
    [string]$OutDir = '',
    [string]$NewVersion = '0.6.1'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot 'hosted-runner.ps1')
Assert-HostedRunner
. (Join-Path $PSScriptRoot 'accept-lib.ps1')

if (-not $InstallResult) { throw 'No install result: run install-latest-r1.ps1 first, or pass -InstallResult.' }
# The install result says which folder this script starts, stops and uninstalls from. It must be the one install-latest-r1.ps1 made on
# this run, on a fresh runner: a hand-written one, or one from another run, is refused before anything is touched.
$r1 = Read-Json $InstallResult
Assert-InstallResultBoundToThisRun $r1
if (-not $OutDir) { $OutDir = Join-Path $env:RUNNER_TEMP 'installer-acceptance' }
Start-Acceptance 'over-r1' $OutDir
$env:STUB_LOG = Join-Path $OutDir 'stub.log'

$facts = Read-Json (Join-Path $PSScriptRoot 'r1-install-facts.json')
$table = Read-Json (Join-Path $PSScriptRoot 'identities.json')
$prod = $table.identities.production
$removeRun = [bool]((Get-Content -LiteralPath (Join-Path $PSScriptRoot 'tjommie.nsi') -Raw) -match '(?m)^!define REMOVE_RUN_VALUE_ON_UNINSTALL\s*$')

$programs = Join-Path $env:LOCALAPPDATA 'Programs'
$folder = [string]$r1.installFolder
$guid = $prod.guid
$dataRoots = [ordered]@{ local = (Join-Path $env:LOCALAPPDATA 'Tjommie'); roaming = (Join-Path $env:APPDATA 'Tjommie') }
$marker = Join-Path $dataRoots['roaming'] 'login-item-set'
$installer = Join-Path $InstallersDir "production-$NewVersion.exe"
if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) { throw "The installer is not at $installer." }
# R1's install must be where this identity installs, or the check of that is meaningless.
if ((Split-Path -Parent ([IO.Path]::GetFullPath($folder).TrimEnd('\'))) -ine $programs -or (Split-Path -Leaf $folder) -cne $prod.folder) { throw "R1 is installed at $folder, not at $(Join-Path $programs $prod.folder)." }
$state = @{}
# What R1 has in its data folders only while it runs (Electron's lockfile): not data, and gone once it stops. From the T8.1 facts.
$r1RuntimeOnly = @(Get-R1RuntimeOnlyEntries $facts)

# Nothing R1 or the test put in a data root is gone, and the files of known content are byte for byte the same. R1's run-time files
# are the one thing ignored, by name; anything else that is missing is a loss.
function Test-DataRootsKept([string]$Label) {
    # If O2 did not finish there is nothing to compare with: say that, once, instead of failing on a missing table and burying the cause.
    if (-not $state.ContainsKey('dataBefore')) {
        Assert-Check $false "$Label`: there is no snapshot of the data roots to compare with (O2 did not finish)"
        return
    }
    foreach ($name in $dataRoots.Keys) {
        $differences = @(Compare-Snapshot $state['dataBefore'][$name] (Get-TreeSnapshot $dataRoots[$name]) -IgnoreAdded -IgnorePaths $r1RuntimeOnly | Where-Object { $_ -like 'missing:*' -or $_ -like 'changed: sentinel\*' })
        Assert-Check ($differences.Count -eq 0) "$Label`: the $name data root lost nothing, and its sentinel files are unchanged" $differences
    }
}

Invoke-Step 'O1 R1 is installed, as recorded' {
    $files = @(Get-InstalledFiles $folder)
    $state['r1Files'] = $files
    Assert-Check (Test-Path -LiteralPath (Join-Path $folder "Uninstall $($prod.product).exe")) 'O1: R1''s uninstaller is in the install folder'
    Assert-Check (Test-Path -LiteralPath (Join-Path $folder $prod.exe)) 'O1: R1''s Tjommie.exe is in the install folder'
    Assert-Check ($files.Count -gt 100) 'O1: R1 is a full Electron install' $files.Count
    $state['r1Uninstall'] = Get-RegistryKeyFact 'CurrentUser' (Get-UninstallKeyName $guid) '64'
    $state['r1InstallKey'] = Get-RegistryKeyFact 'CurrentUser' "Software\$guid" '64'
    Assert-Check ($null -ne $state['r1Uninstall'] -and $null -ne $state['r1InstallKey']) 'O1: R1 registered the Uninstall key and the install key under the GUID the installer uses'
    $state['r1Location'] = Get-InstallLocation $guid
    Assert-Check ([string]$state['r1Location'] -ceq $folder) 'O1: R1 recorded its own folder as InstallLocation' $state['r1Location']
    Assert-Check (@(Get-RunningFrom $folder).Count -eq 0) 'O1: nothing is running yet'
    Add-Note "O1: R1's own run-time files, ignored when the data roots are compared: $($r1RuntimeOnly -join ', ')" $r1RuntimeOnly
}

Invoke-Step 'O1b the installer refuses to go near R1 when InstallLocation names another folder' {
    # R1's own uninstaller deletes the folder HKCU\Software\<GUID>\InstallLocation names, whatever it is told. The installer must stop
    # before it runs it: not one R1 file, not one byte of the named folder, and not the value itself may change.
    $scratch = Join-Path $env:LOCALAPPDATA 'Temp\tjommie-f1-scratch'
    $sibling = Join-Path $programs 'tjommie-rs'
    $others = [ordered]@{}
    foreach ($path in @($scratch, $sibling)) { [void](Add-Sentinels $path); $others[$path] = Get-TreeSnapshot $path }
    $r1Before = Get-TreeSnapshot $folder
    foreach ($planted in @('C:\', $scratch, $sibling)) {
        Set-InstallLocation $guid $planted
        $update = Invoke-Setup $installer '/S --updated' -Watch
        Assert-Check ($update['exitCode'] -eq 7) "O1b: with InstallLocation $planted the installer refuses (exit 7)" $update['exitCode']
        Assert-Check (@($update['processes'] | Where-Object { $_['name'] -eq 'old-uninstaller.exe' }).Count -eq 0) "O1b: ...and R1's uninstaller is never started (InstallLocation $planted)"
        $differences = @(Compare-Snapshot $r1Before (Get-TreeSnapshot $folder))
        Assert-Check ($differences.Count -eq 0) "O1b: ...every R1 file is untouched (InstallLocation $planted)" $differences
        foreach ($path in $others.Keys) {
            $differences = @(Compare-Snapshot $others[$path] (Get-TreeSnapshot $path))
            Assert-Check ($differences.Count -eq 0) "O1b: ...$path is untouched (InstallLocation $planted)" $differences
        }
        Assert-Check ((Get-InstallLocation $guid) -ceq $planted) "O1b: ...and the value was not corrected (InstallLocation $planted)" (Get-InstallLocation $guid)
    }
    Set-InstallLocation $guid $state['r1Location']
    foreach ($path in @($scratch, $sibling)) { Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction SilentlyContinue }
}

Invoke-Step 'O2 R1 runs, on data roots that hold files of known content' {
    foreach ($name in $dataRoots.Keys) { [void](Add-Sentinels $dataRoots[$name]) }
    $r1Process = Start-Process -FilePath (Join-Path $folder $prod.exe) -PassThru
    $appeared = Wait-Until { Test-Path -LiteralPath $marker } 90
    Assert-Check $appeared 'O2: R1 started and wrote its start-at-login marker' $marker
    Start-Sleep -Seconds 3
    $state['r1Ids'] = @(Get-RunningFrom $folder)
    Assert-Check ($state['r1Ids'].Count -ge 2) 'O2: R1 (the app and its daemon, at least) is running from the install folder' $state['r1Ids'].Count
    $state['runBefore'] = Get-RunValue $prod.appId
    Assert-Check ([string]$state['runBefore'] -ceq "`"$(Join-Path $folder $prod.exe)`" --hidden") 'O2: R1 wrote its start-at-login value' $state['runBefore']
    # Two programs that are not R1's, to see what R1's own uninstaller does to them: the same name from another folder (reached only by
    # image name), and a different name from a folder whose path merely starts with R1's (reached only by a path prefix).
    $sameName = Join-Path $env:LOCALAPPDATA 'Temp\tjommie-decoy'
    $sibling = Join-Path $programs 'tjommie-sibling'
    foreach ($place in @($sameName, $sibling)) { New-Item -ItemType Directory -Force -Path $place | Out-Null }
    Copy-Item -LiteralPath (Join-Path $InstallersDir "payload\production\$($prod.exe)") -Destination (Join-Path $sameName $prod.exe) -Force
    Copy-Item -LiteralPath (Join-Path $InstallersDir 'payload\dev\TjommieRS.exe') -Destination (Join-Path $sibling 'TjommieRS.exe') -Force
    $state['sameName'] = Start-Stub (Join-Path $sameName $prod.exe)
    $state['sibling'] = Start-Stub (Join-Path $sibling 'TjommieRS.exe')
    $state['decoyFolders'] = @($sameName, $sibling)
    # Every process with "tjommie" in its path, by id: after the install, the ones that are gone were closed by something.
    $state['watched'] = Get-ProcessPathsMatching 'tjommie'
    $state['dataBefore'] = [ordered]@{}
    foreach ($name in $dataRoots.Keys) { $state['dataBefore'][$name] = Get-TreeSnapshot $dataRoots[$name] }
}

Invoke-Step 'O3 the installer takes over from R1' {
    # O3 needs what O2 recorded (R1 running, the decoys, the snapshot). Without it the hand-over would run over an R1 that was never
    # started and every check below would say something misleading, so it does not run.
    foreach ($needed in 'r1Ids', 'sameName', 'sibling', 'watched', 'dataBefore', 'decoyFolders') {
        if (-not $state.ContainsKey($needed)) {
            Assert-Check $false "O3 did not run: O2 did not finish, so '$needed' was never recorded"
            return
        }
    }
    $update = Invoke-Setup $installer '/S --updated' -Watch
    Assert-Check ($update['exitCode'] -eq 0) 'O3: the installer exits 0' $update['exitCode']
    # R1's own uninstaller, from a temp copy, with the arguments R1's upgrade uses (T8.1).
    $expectedArguments = "/S /KEEP_APP_DATA /currentuser --keep-shortcuts --updated _?=$folder"
    $old = @($update['processes'] | Where-Object { $_['name'] -eq 'old-uninstaller.exe' -and $_['commandLine'] })
    Assert-Check ($old.Count -ge 1) 'O3: R1''s uninstaller was run, as old-uninstaller.exe' @($update['processes'] | ForEach-Object { $_['name'] })
    if ($old.Count -ge 1) {
        Assert-Check (([string]$old[0]['commandLine']).EndsWith($expectedArguments, [StringComparison]::OrdinalIgnoreCase)) 'O3: with the arguments R1''s own upgrade gives it' $old[0]['commandLine']
        Assert-Check (-not (Test-PathInFolder ([string]$old[0]['executablePath']) $folder)) 'O3: from a temp copy' $old[0]['executablePath']
    }
    # R1 is closed, and only R1 by this installer.
    $alive = @($state['r1Ids'] | Where-Object { Test-Alive $_ })
    Assert-Check ($alive.Count -eq 0) 'O3: every process R1 had running from the install folder is closed' $alive
    Assert-Check (@(Get-RunningFrom $folder).Count -eq 0) 'O3: nothing runs from the install folder'
    # Not asserted (see the top of this file): what R1's own uninstaller closes besides R1. Recorded, process by process.
    foreach ($pair in @(@('the same name from another folder', $state['sameName']), @('a different name from a folder starting with R1''s path', $state['sibling']))) {
        $survived = Test-Alive $pair[1].Id
        Add-Note "O3: $($pair[0]) was $(if ($survived) { 'left running' } else { 'closed, by R1''s own uninstaller (this installer closes by path with a trailing separator and does not reach it)' })" $survived
    }
    $closed = @($state['watched'].Keys | Where-Object { $state['r1Ids'] -notcontains $_ -and -not (Test-Alive $_) } | ForEach-Object { $state['watched'][$_] })
    Add-Note "O3: processes with tjommie in their path, other than R1's own, that were gone afterwards: $(if ($closed.Count -eq 0) { 'none' } else { $closed -join '; ' })" $closed
    # The install: the payload and the uninstaller, and not one R1 file.
    $files = @(Get-InstalledFiles $folder)
    $expected = @(Get-ExpectedFiles $prod.product $prod.exe)
    Assert-Check (($files -join '|') -ceq ($expected -join '|')) 'O3: the install holds the new payload and the uninstaller, and nothing else' @{ files = $files; expected = $expected }
    $leftovers = @($state['r1Files'] | Where-Object { $expected -notcontains $_ -and (Test-Path -LiteralPath (Join-Path $folder $_)) })
    Assert-Check ($leftovers.Count -eq 0) 'O3: no file of R1''s is left' $leftovers
    # The keys and shortcuts, as R1 had them.
    Test-InstalledAsR1Did $facts $folder $NewVersion 'O3'
    Test-ShortcutsAsR1Had $facts $folder $prod.product $prod.exe $prod.appId 'O3'
    $uninstall = Get-RegistryKeyFact 'CurrentUser' (Get-UninstallKeyName $guid) '64'
    $namesBefore = @($state['r1Uninstall']['values'] | ForEach-Object { $_['name'] }) -join '|'
    $namesAfter = @($uninstall['values'] | ForEach-Object { $_['name'] }) -join '|'
    Assert-Check ($namesAfter -ceq $namesBefore) 'O3: the Uninstall key has the same values it had under R1' @{ before = $namesBefore; after = $namesAfter }
    # The start-at-login value R1 wrote is still there, and now starts the new exe.
    $run = Get-RunValue $prod.appId
    Assert-Check ([string]$run -ceq [string]$state['runBefore']) 'O3: the start-at-login value R1 wrote is unchanged' $run
    $runExe = (Get-CommandExecutable ([string]$run))
    Assert-Check ($runExe -and (Test-Path -LiteralPath $runExe)) 'O3: and what it starts is in the install' $runExe
    Test-DataRootsKept 'O3'
    foreach ($key in 'sameName', 'sibling') { Stop-Process -Id $state[$key].Id -Force -ErrorAction SilentlyContinue }
    foreach ($place in $state['decoyFolders']) { Remove-Item -LiteralPath $place -Recurse -Force -ErrorAction SilentlyContinue }
}

Invoke-Step 'O4 the new uninstaller removes the install and leaves the data roots alone' {
    $gone = Invoke-RegisteredUninstall $guid $folder
    Assert-Check $gone 'O4: the install folder is removed'
    Assert-Check (-not (Test-KeyExists (Get-UninstallKeyName $guid))) 'O4: the Uninstall key is gone'
    Assert-Check (-not (Test-KeyExists "Software\$guid")) 'O4: the install key is gone'
    $run = Get-RunValue $prod.appId
    if ($removeRun) { Assert-Check ($null -eq $run) 'O4: the start-at-login value, which pointed into the folder, is removed' $run }
    else { Assert-Check ($null -ne $run) 'O4: the start-at-login value is left, as R1 leaves it' $run }
    Test-DataRootsKept 'O4'
}

Complete-Acceptance
