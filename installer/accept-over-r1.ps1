# Installer acceptance over a real R1, on a GitHub-hosted runner (T8.2). install-latest-r1.ps1 has installed the latest published R1
# silently (its result file is -InstallResult, or $env:R1_INSTALL_RESULT); this starts that R1 once, so its Electron processes and its
# daemon are running on a data root that holds files of known content, then runs the Tjommie installer (stub payload) over it with
# /S --updated, as the updater does, and checks the hand-over:
#
#   O1  R1 is installed, as recorded
#   O2  R1 runs (its login-item marker appears), with a decoy of the same name running from another folder
#   O3  the installer: closes R1's processes by path (and notes whether the decoy survived R1's own uninstaller, which closes by
#       image name); runs R1's own uninstaller from a temp copy with the
#       arguments R1's upgrade uses; leaves the folder, keys and shortcuts as R1 had them; removes every R1 file; keeps the
#       start-at-login value R1 wrote, which now starts the new exe; and loses nothing from either data root
#   O4  the new uninstaller removes the install and leaves both data roots alone
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

Invoke-Step 'O1 R1 is installed, as recorded' {
    $files = @(Get-InstalledFiles $folder)
    $state['r1Files'] = $files
    Assert-Check (Test-Path -LiteralPath (Join-Path $folder "Uninstall $($prod.product).exe")) 'O1: R1''s uninstaller is in the install folder'
    Assert-Check (Test-Path -LiteralPath (Join-Path $folder $prod.exe)) 'O1: R1''s Tjommie.exe is in the install folder'
    Assert-Check ($files.Count -gt 100) 'O1: R1 is a full Electron install' $files.Count
    $state['r1Uninstall'] = Get-RegistryKeyFact 'CurrentUser' (Get-UninstallKeyName $guid) '64'
    $state['r1InstallKey'] = Get-RegistryKeyFact 'CurrentUser' "Software\$guid" '64'
    Assert-Check ($null -ne $state['r1Uninstall'] -and $null -ne $state['r1InstallKey']) 'O1: R1 registered the Uninstall key and the install key under the GUID the installer uses'
    Assert-Check (@(Get-RunningFrom $folder).Count -eq 0) 'O1: nothing is running yet'
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
    # A Tjommie.exe of the same name running from somewhere else: not R1's, and not to be closed.
    $decoyFolder = Join-Path $env:LOCALAPPDATA 'Temp\tjommie-decoy'
    New-Item -ItemType Directory -Force -Path $decoyFolder | Out-Null
    Copy-Item -LiteralPath (Join-Path $InstallersDir "payload\production\$($prod.exe)") -Destination (Join-Path $decoyFolder $prod.exe) -Force
    $state['decoy'] = Start-Stub (Join-Path $decoyFolder $prod.exe)
    $state['decoyFolder'] = $decoyFolder
    $state['dataBefore'] = [ordered]@{}
    foreach ($name in $dataRoots.Keys) { $state['dataBefore'][$name] = Get-TreeSnapshot $dataRoots[$name] }
}

Invoke-Step 'O3 the installer takes over from R1' {
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
    # R1 is closed, and only R1.
    $alive = @($state['r1Ids'] | Where-Object { Test-Alive $_ })
    Assert-Check ($alive.Count -eq 0) 'O3: every process R1 had running from the install folder is closed' $alive
    Assert-Check (@(Get-RunningFrom $folder).Count -eq 0) 'O3: nothing runs from the install folder'
    # Not asserted: R1's own uninstaller, which this installer must run, looks for "Tjommie.exe" by image name (T8.1 recorded its
    # tasklist probe) and electron-builder's template stops what it finds that way. Only this installer's own stop is by path; what
    # R1's uninstaller does to a same-named program elsewhere is R1's, so it is recorded here, not promised.
    $decoyAlive = Test-Alive $state['decoy'].Id
    Add-Note "O3: a Tjommie.exe running from another folder was $(if ($decoyAlive) { 'left running' } else { 'closed (by R1''s own uninstaller, which stops by image name, not by this installer)' })" $decoyAlive
    # The install: the payload, the uninstaller and the helpers, and not one R1 file.
    $files = @(Get-InstalledFiles $folder)
    $expected = @(Get-ExpectedFiles $prod.product $prod.exe)
    Assert-Check (($files -join '|') -ceq ($expected -join '|')) 'O3: the install holds the new payload, the uninstaller and the helpers, and nothing else' @{ files = $files; expected = $expected }
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
    # Both data roots: nothing R1 or the test put there is gone, and the files of known content are byte for byte the same.
    foreach ($name in $dataRoots.Keys) {
        $differences = @(Compare-Snapshot $state['dataBefore'][$name] (Get-TreeSnapshot $dataRoots[$name]) -IgnoreAdded | Where-Object { $_ -like 'missing:*' -or $_ -like 'changed: sentinel\*' })
        Assert-Check ($differences.Count -eq 0) "O3: the $name data root lost nothing, and its sentinel files are unchanged" $differences
    }
    Stop-Process -Id $state['decoy'].Id -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $state['decoyFolder'] -Recurse -Force -ErrorAction SilentlyContinue
}

Invoke-Step 'O4 the new uninstaller removes the install and leaves the data roots alone' {
    $gone = Invoke-RegisteredUninstall $guid $folder
    Assert-Check $gone 'O4: the install folder is removed'
    Assert-Check (-not (Test-KeyExists (Get-UninstallKeyName $guid))) 'O4: the Uninstall key is gone'
    Assert-Check (-not (Test-KeyExists "Software\$guid")) 'O4: the install key is gone'
    $run = Get-RunValue $prod.appId
    if ($removeRun) { Assert-Check ($null -eq $run) 'O4: the start-at-login value, which pointed into the folder, is removed' $run }
    else { Assert-Check ($null -ne $run) 'O4: the start-at-login value is left, as R1 leaves it' $run }
    foreach ($name in $dataRoots.Keys) {
        $differences = @(Compare-Snapshot $state['dataBefore'][$name] (Get-TreeSnapshot $dataRoots[$name]) -IgnoreAdded | Where-Object { $_ -like 'missing:*' -or $_ -like 'changed: sentinel\*' })
        Assert-Check ($differences.Count -eq 0) "O4: the $name data root still holds everything, and its sentinel files are unchanged" $differences
    }
}

Complete-Acceptance
