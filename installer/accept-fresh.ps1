# Installer acceptance on a fresh GitHub-hosted runner, with the stub payload (T8.2). It installs, updates and uninstalls the
# Tjommie installers built by build-acceptance-installers.ps1, and checks what each leaves on the machine against what T8.1
# recorded from R1 (installer/r1-install-facts.json). It refuses to run anywhere but a fresh hosted runner: it installs,
# uninstalls and plants registry values.
#
#   S0  seed both production data roots with files of known content
#   S1  a fresh silent install of production: R1's folder, keys, values and shortcuts; the data roots unchanged
#   S2  its registered uninstall: everything it made is gone, the start-at-login value too (when the switch says so), the data roots
#       unchanged byte for byte
#   S3  a start-at-login value that points elsewhere, or at a sibling folder, is left alone
#   S4  an update (--updated) runs the old uninstaller as R1's upgrade did, keeps the start-at-login value, and replaces the version
#   S5  the uninstaller refuses, and deletes nothing, when InstallLocation is C:\, another folder, or missing; and when it is right but
#       the uninstaller is pointed (_?=) at the data roots, C:\ or another folder
#   S5b an update refuses (exit 7), before it closes, extracts or runs anything, when InstallLocation names C:\, a scratch folder or a
#       sibling folder; none of them loses a byte, the value is not "corrected", and the old uninstaller is never run
#   S6  an update closes what runs from the install folder, and a same-named program running from another folder is left running
#   S7  --force-run starts the app again, with --updated
#   S8  the uninstaller closes the app first
#   S12 a junction inside the install folder is removed as a link by an update and by an uninstall, and its target survives; an install
#       folder that is itself a link is refused (exit 9)
#   S9  production, dev and updatetest installs do not touch each other; a folder an identity may not use is refused
#   S10 /D= cannot move production's install
#
# Every check is recorded in installer-acceptance-fresh.json (written after every step, so a red run still has what it saw).
param(
    [Parameter(Mandatory)][string]$InstallersDir,
    [string]$OutDir = '',
    [string]$OldVersion = '0.6.0',
    [string]$NewVersion = '0.6.1'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot 'hosted-runner.ps1')
Assert-HostedRunner
Assert-FreshRunner
. (Join-Path $PSScriptRoot 'accept-lib.ps1')

if (-not $OutDir) { $OutDir = Join-Path $env:RUNNER_TEMP 'installer-acceptance' }
Start-Acceptance 'fresh' $OutDir
$env:STUB_LOG = Join-Path $OutDir 'stub.log'

$facts = Read-Json (Join-Path $PSScriptRoot 'r1-install-facts.json')
$table = Read-Json (Join-Path $PSScriptRoot 'identities.json')
$prod = $table.identities.production
$dev = $table.identities.dev
$test = $table.identities.updatetest
$removeRun = [bool]((Get-Content -LiteralPath (Join-Path $PSScriptRoot 'tjommie.nsi') -Raw) -match '(?m)^!define REMOVE_RUN_VALUE_ON_UNINSTALL\s*$')

$programs = Join-Path $env:LOCALAPPDATA 'Programs'
$folder = Join-Path $programs $prod.folder
$guid = $prod.guid
$dataRoots = [ordered]@{ local = (Join-Path $env:LOCALAPPDATA 'Tjommie'); roaming = (Join-Path $env:APPDATA 'Tjommie') }
$shortcutFolders = Get-ShortcutFolders
$runData = "`"$(Join-Path $folder $prod.exe)`" --hidden"
$installers = [ordered]@{
    old        = Join-Path $InstallersDir "production-$OldVersion.exe"
    new        = Join-Path $InstallersDir "production-$NewVersion.exe"
    dev        = Join-Path $InstallersDir "dev-$OldVersion.exe"
    updatetest = Join-Path $InstallersDir "updatetest-$OldVersion.exe"
}
foreach ($name in $installers.Keys) { if (-not (Test-Path -LiteralPath $installers[$name] -PathType Leaf)) { throw "The $name installer is not at $($installers[$name])." } }
$state = @{}

function Test-DataUnchanged([string]$When) {
    foreach ($name in $dataRoots.Keys) {
        $differences = @(Compare-Snapshot $state['data'][$name] (Get-TreeSnapshot $dataRoots[$name]))
        Assert-Check ($differences.Count -eq 0) "the $name data root is unchanged $When (file count and hashes)" $differences
    }
}

function Test-Shortcut([string]$Product, [bool]$Exists, [string]$Label) {
    foreach ($place in 'userStartMenu', 'userDesktop') {
        $present = Test-Path -LiteralPath (Join-Path $shortcutFolders[$place] "$Product.lnk")
        Assert-Check ($present -eq $Exists) "$Label`: $Product.lnk is $(if ($Exists) { 'in' } else { 'gone from' }) $place"
    }
}

# What a stub-payload install of production leaves, against R1's facts.
function Test-ProductionInstalled([string]$Version, [string]$Label) {
    $names = @(Get-ChildItem -LiteralPath $programs -Directory | ForEach-Object { $_.Name })
    Assert-Check ($names -ccontains $prod.folder) "$Label`: the install folder is $($prod.folder), in lowercase" $names
    $files = @(Get-InstalledFiles $folder)
    $expected = @(Get-ExpectedFiles $prod.product $prod.exe)
    Assert-Check (($files -join '|') -ceq ($expected -join '|')) "$Label`: the install holds the payload and the uninstaller, and nothing else" @{ files = $files; expected = $expected }
    Test-InstalledAsR1Did $facts $folder $Version $Label
    Test-ShortcutsAsR1Had $facts $folder $prod.product $prod.exe $prod.appId $Label
}

# A fresh install writes no start-at-login value: the app does that, once, on its first start. (An update keeps the one it finds.)
function Test-NoRunValue([string]$Label) {
    Assert-Check ($null -eq (Get-RunValue $prod.appId)) "$Label`: the installer writes no start-at-login value (the app does, once)" (Get-RunValue $prod.appId)
}

function Test-ProductionRemoved([string]$Label) {
    Assert-Check (-not (Test-Path -LiteralPath $folder)) "$Label`: the install folder is gone"
    Assert-Check (-not (Test-KeyExists (Get-UninstallKeyName $guid))) "$Label`: the Uninstall key is gone"
    Assert-Check (-not (Test-KeyExists "Software\$guid")) "$Label`: the install key is gone"
    Test-Shortcut $prod.product $false $Label
}

Invoke-Step 'S0 seed the data roots' {
    foreach ($name in $dataRoots.Keys) { [void](Add-Sentinels $dataRoots[$name]) }
    $state['data'] = [ordered]@{}
    foreach ($name in $dataRoots.Keys) { $state['data'][$name] = Get-TreeSnapshot $dataRoots[$name] }
    Assert-Check (@($state['data']['local'].Keys).Count -gt 0 -and @($state['data']['roaming'].Keys).Count -gt 0) 'both data roots hold files' @{ local = @($state['data']['local'].Keys).Count; roaming = @($state['data']['roaming'].Keys).Count }
}

Invoke-Step 'S1 a fresh silent install of production' {
    $run = Invoke-Setup $installers['old'] '/S'
    Assert-Check ($run['exitCode'] -eq 0) 'the silent installer exits 0' $run['exitCode']
    Test-ProductionInstalled $OldVersion 'S1'
    Test-NoRunValue 'S1'
    Assert-Check (@(Get-RunningFrom $folder).Count -eq 0) 'S1: a silent install without --force-run starts nothing'
    Test-DataUnchanged 'by the install'
}

Invoke-Step 'S2 the registered uninstall removes the install and leaves the data roots alone' {
    Set-RunValue $prod.appId $runData
    $gone = Invoke-RegisteredUninstall $guid $folder
    Assert-Check $gone 'S2: the install folder is removed by the registered QuietUninstallString'
    Test-ProductionRemoved 'S2'
    $run = Get-RunValue $prod.appId
    if ($removeRun) { Assert-Check ($null -eq $run) 'S2: the start-at-login value, which pointed into the folder, is removed' $run }
    else { Assert-Check ($run -ceq $runData) 'S2: the start-at-login value is left, as R1 leaves it' $run }
    Test-DataUnchanged 'by the uninstall'
    Set-RunValue $prod.appId $null
}

# The value the app writes is "<exe>" --hidden, quoted; only that form, pointing into the folder, is removed.
Invoke-Step 'S3 a start-at-login value that points elsewhere is left alone' {
    foreach ($other in @('"C:\elsewhere\Tjommie\Tjommie.exe" --hidden', "`"$(Join-Path $programs 'tjommie-old')\Tjommie.exe`" --hidden")) {
        $run = Invoke-Setup $installers['old'] '/S'
        Assert-Check ($run['exitCode'] -eq 0) 'S3: the installer exits 0' $run['exitCode']
        Set-RunValue $prod.appId $other
        $gone = Invoke-RegisteredUninstall $guid $folder
        Assert-Check $gone "S3: the install is removed (value: $other)"
        Assert-Check ((Get-RunValue $prod.appId) -ceq $other) "S3: the value is still there: $other" (Get-RunValue $prod.appId)
        Set-RunValue $prod.appId $null
    }
    Test-DataUnchanged 'by these uninstalls'
}

Invoke-Step 'S4 an update runs the old uninstaller as R1 did, keeps the start-at-login value, and replaces the version' {
    $first = Invoke-Setup $installers['old'] '/S'
    Assert-Check ($first['exitCode'] -eq 0) 'S4: the first install exits 0'
    Set-RunValue $prod.appId $runData
    $update = Invoke-Setup $installers['new'] '/S --updated' -Watch
    Assert-Check ($update['exitCode'] -eq 0) 'S4: the update exits 0' $update['exitCode']
    $expectedArguments = "/S /KEEP_APP_DATA /currentuser --keep-shortcuts --updated _?=$folder"
    $old = @($update['processes'] | Where-Object { $_['name'] -eq 'old-uninstaller.exe' -and $_['commandLine'] })
    Assert-Check ($old.Count -ge 1) 'S4: the old uninstaller was run, as old-uninstaller.exe' @($update['processes'] | ForEach-Object { $_['name'] })
    if ($old.Count -ge 1) {
        Assert-Check (([string]$old[0]['commandLine']).EndsWith($expectedArguments, [StringComparison]::OrdinalIgnoreCase)) 'S4: with the arguments R1''s own upgrade gives it' $old[0]['commandLine']
        Assert-Check (-not (Test-PathInFolder ([string]$old[0]['executablePath']) $folder)) 'S4: from a temp copy, not from the install folder' $old[0]['executablePath']
    }
    Test-ProductionInstalled $NewVersion 'S4'
    $key = Get-RegistryKeyFact 'CurrentUser' (Get-UninstallKeyName $guid) '64'
    $display = @($key['values'] | Where-Object { $_['name'] -eq 'DisplayName' })[0]['data']
    Assert-Check ($display -ceq "$($prod.product) $NewVersion") 'S4: the Apps & features entry names the new version' $display
    Assert-Check ((Get-RunValue $prod.appId) -ceq $runData) 'S4: the start-at-login value is kept through the update' (Get-RunValue $prod.appId)
    Test-DataUnchanged 'by the update'
}

Invoke-Step 'S5 the uninstaller refuses when InstallLocation is not its own install folder' {
    # S4 left the new version installed, with a start-at-login value; this removes nothing it should keep.
    $before = Get-TreeSnapshot $folder
    foreach ($planted in @('C:\', (Join-Path $programs 'other'), (Join-Path $programs 'tjommie-rs'), (Join-Path $env:TEMP 'tjommie'), $null)) {
        $label = if ($null -eq $planted) { '(no InstallLocation)' } else { $planted }
        Set-InstallLocation $guid $planted
        $inPlace = Invoke-Setup (Join-Path $folder "Uninstall $($prod.product).exe") "/S _?=$folder"
        Assert-Check ($inPlace['exitCode'] -ne 0) "S5: run in place with InstallLocation $label, the uninstaller exits non-zero" $inPlace['exitCode']
        $differences = @(Compare-Snapshot $before (Get-TreeSnapshot $folder))
        Assert-Check ($differences.Count -eq 0) "S5: with InstallLocation $label, nothing in the install folder is touched" $differences
        Assert-Check (Test-KeyExists (Get-UninstallKeyName $guid)) "S5: with InstallLocation $label, the Uninstall key is kept"
        Test-Shortcut $prod.product $true "S5 with InstallLocation $label"
        Assert-Check ((Get-RunValue $prod.appId) -ceq $runData) "S5: with InstallLocation $label, the start-at-login value is kept"
    }
    # And as Windows runs it, from a temp copy: the same refusal, seen as a folder that is still there.
    foreach ($planted in @('C:\', (Join-Path $programs 'other'))) {
        Set-InstallLocation $guid $planted
        $gone = Invoke-RegisteredUninstall $guid $folder -ExpectRemoval:$false
        Assert-Check (-not $gone) "S5: run as Windows runs it, with InstallLocation $planted, the install folder is still there"
        $differences = @(Compare-Snapshot $before (Get-TreeSnapshot $folder))
        Assert-Check ($differences.Count -eq 0) "S5: ...and nothing in it was touched (InstallLocation $planted)" $differences
    }
    Set-InstallLocation $guid $folder
    Test-DataUnchanged 'by the refused uninstalls'

    # The other half of the guard: the right InstallLocation, but the uninstaller is pointed (_?=) at some other folder, as R1's
    # uninstaller can be. It must refuse that too (exit 5), and the folders it was pointed at, the data roots among them, must be whole.
    foreach ($target in @($dataRoots['local'], $dataRoots['roaming'], 'C:\', (Join-Path $programs 'other'), (Join-Path $programs 'tjommie-rs'))) {
        $redirected = Invoke-Setup (Join-Path $folder "Uninstall $($prod.product).exe") "/S _?=$target"
        # A drive root is refused by NSIS's own start-up code, before this script's un.onInit runs: the stub validates the _?= path and,
        # with AllowRootDirInstall left at its default (false, and installer/tjommie.nsi never turns it on), rejects a root such as C:\ and
        # exits 2 ("uninstaller initialisation error"). That is a stricter, earlier refusal than this script's guard (exit 5), and run 3
        # saw exactly that. Every other target reaches the guard, which exits 5. Both must leave everything untouched, checked below.
        $isDriveRoot = ($target.TrimEnd('\').Length -le 2)
        $expected = if ($isDriveRoot) { @(2, 5) } else { @(5) }
        Assert-Check ($expected -contains $redirected['exitCode']) "S5: with the right InstallLocation but _?=$target, the uninstaller refuses (exit $($expected -join ' or '))" $redirected['exitCode']
        $differences = @(Compare-Snapshot $before (Get-TreeSnapshot $folder))
        Assert-Check ($differences.Count -eq 0) "S5: ...and nothing in the install folder is touched (_?=$target)" $differences
        Test-DataUnchanged "by an uninstaller pointed at $target"
    }
}

Invoke-Step 'S5b an update refuses when InstallLocation names another folder, and changes nothing' {
    # The old uninstaller that an update runs deletes the folder InstallLocation names (R1's does, and ignores the folder it is given),
    # so the installer must stop first, before it closes, extracts or runs anything. Each folder below holds files of known content.
    $scratch = Join-Path $env:LOCALAPPDATA 'Temp\tjommie-f1-scratch'
    $siblings = @((Join-Path $programs 'tjommie-rs'), (Join-Path $programs 'tjommie-old'))
    $others = [ordered]@{}
    foreach ($path in (@($scratch) + $siblings)) { [void](Add-Sentinels $path); $others[$path] = Get-TreeSnapshot $path }
    $installBefore = Get-TreeSnapshot $folder
    $app = Start-Stub (Join-Path $folder $prod.exe)
    Assert-Check (Wait-Until { @(Get-RunningFrom $folder).Count -ge 1 } 20) 'S5b: the app stub is running from the install folder'
    foreach ($planted in @('C:\', $scratch, $siblings[0], $siblings[1])) {
        Set-InstallLocation $guid $planted
        $update = Invoke-Setup $installers['new'] '/S --updated' -Watch
        Assert-Check ($update['exitCode'] -eq 7) "S5b: with InstallLocation $planted the update refuses (exit 7)" $update['exitCode']
        Assert-Check (@($update['processes'] | Where-Object { $_['name'] -eq 'old-uninstaller.exe' }).Count -eq 0) "S5b: ...and the old uninstaller is never run (InstallLocation $planted)"
        $differences = @(Compare-Snapshot $installBefore (Get-TreeSnapshot $folder))
        Assert-Check ($differences.Count -eq 0) "S5b: ...the install folder is untouched (InstallLocation $planted)" $differences
        foreach ($path in $others.Keys) {
            $differences = @(Compare-Snapshot $others[$path] (Get-TreeSnapshot $path))
            Assert-Check ($differences.Count -eq 0) "S5b: ...$path is untouched (InstallLocation $planted)" $differences
        }
        Assert-Check (Test-Alive $app.Id) "S5b: ...nothing was closed (InstallLocation $planted)"
        Assert-Check ((Get-InstallLocation $guid) -ceq $planted) "S5b: ...and the value was not corrected (InstallLocation $planted)" (Get-InstallLocation $guid)
    }
    Set-InstallLocation $guid $folder
    [void](Stop-ProcessesInFolder $folder)
    Test-DataUnchanged 'by the refused updates'
    $ok = Invoke-Setup $installers['new'] '/S --updated'
    Assert-Check ($ok['exitCode'] -eq 0) 'S5b: with InstallLocation as it should be, the same update goes through' $ok['exitCode']
    foreach ($path in (@($scratch) + $siblings)) { Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction SilentlyContinue }
}

Invoke-Step 'S6 an update closes what runs from the install folder, and nothing else' {
    $decoyFolder = Join-Path $env:TEMP 'tjommie-decoy'
    New-Item -ItemType Directory -Force -Path $decoyFolder | Out-Null
    Copy-Item -LiteralPath (Join-Path $folder $prod.exe) -Destination (Join-Path $decoyFolder $prod.exe) -Force
    $decoy = Start-Stub (Join-Path $decoyFolder $prod.exe)
    $app = Start-Stub (Join-Path $folder $prod.exe) '--spawn-daemon'
    $running = Wait-Until { @(Get-RunningFrom $folder).Count -ge 2 } 30
    Assert-Check $running 'S6: the app stub and its daemon stub are running from the install folder' @(Get-RunningFrom $folder)
    $ids = @(Get-RunningFrom $folder)
    $update = Invoke-Setup $installers['new'] '/S --updated'
    Assert-Check ($update['exitCode'] -eq 0) 'S6: the update exits 0' $update['exitCode']
    Assert-Check (@(Get-RunningFrom $folder).Count -eq 0) 'S6: nothing runs from the install folder afterwards'
    Assert-Check (@($ids | Where-Object { Test-Alive $_ }).Count -eq 0) 'S6: every process that ran from it, and its children, was closed' @($ids | Where-Object { Test-Alive $_ })
    Assert-Check (Test-Alive $decoy.Id) 'S6: a Tjommie.exe of the same name running from another folder was left running'
    Stop-Process -Id $decoy.Id -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $decoyFolder -Recurse -Force -ErrorAction SilentlyContinue
    $started = @(Get-StubLog | Where-Object { $_ -match '--updated' })
    Assert-Check ($started.Count -eq 0) 'S6: an update without --force-run started nothing' $started
}

Invoke-Step 'S7 --force-run starts the app again, with --updated' {
    $run = Invoke-Setup $installers['new'] '/S --updated --force-run'
    Assert-Check ($run['exitCode'] -eq 0) 'S7: the installer exits 0' $run['exitCode']
    $logged = Wait-Until { @(Get-StubLog | Where-Object { $_ -match ('^' + [regex]::Escape($prod.exe) + ' \d+ --updated$') }).Count -ge 1 } 30
    Assert-Check $logged "S7: $($prod.exe) was started with --updated" @(Get-StubLog)
    Assert-Check (@(Get-RunningFrom $folder).Count -ge 1) 'S7: it is running from the install folder'
    [void](Stop-ProcessesInFolder $folder)
    Assert-Check (Wait-Until { @(Get-RunningFrom $folder).Count -eq 0 } 20) 'S7: it was stopped again (by path)'
}

Invoke-Step 'S8 the uninstaller closes the app before it removes anything' {
    $app = Start-Stub (Join-Path $folder $prod.exe) '--spawn-daemon'
    Assert-Check (Wait-Until { @(Get-RunningFrom $folder).Count -ge 2 } 30) 'S8: the app and its daemon are running'
    Set-RunValue $prod.appId $runData
    $gone = Invoke-RegisteredUninstall $guid $folder
    Assert-Check $gone 'S8: the install folder is removed although the app was running'
    Assert-Check (-not (Test-Alive $app.Id)) 'S8: the app was closed'
    Test-ProductionRemoved 'S8'
    Assert-Check ($removeRun -eq ($null -eq (Get-RunValue $prod.appId))) 'S8: the start-at-login value follows the switch' (Get-RunValue $prod.appId)
    Set-RunValue $prod.appId $null
    Test-DataUnchanged 'by the uninstall of a running app'
}

Invoke-Step 'S12 a junction inside the install folder is removed as a link, and what it points at survives' {
    # NSIS's RMDir /r (and R1's uninstaller) would follow a junction into its target and empty it. A developer who has made one inside
    # the install folder must not lose the folder it points at, to an update or to an uninstall.
    $target = Join-Path $env:LOCALAPPDATA 'Temp\tjommie-link-target'
    [void](Add-Sentinels $target)
    $targetBefore = Get-TreeSnapshot $target
    $links = @((Join-Path $folder 'linked-a'), (Join-Path $folder 'resources\linked-b'))
    $first = Invoke-Setup $installers['old'] '/S'
    Assert-Check ($first['exitCode'] -eq 0) 'S12: production is installed' $first['exitCode']
    foreach ($link in $links) { New-Junction $link $target }
    $update = Invoke-Setup $installers['new'] '/S --updated'
    Assert-Check ($update['exitCode'] -eq 0) 'S12: an update over an install with junctions in it exits 0' $update['exitCode']
    foreach ($link in $links) { Assert-Check (-not (Test-Path -LiteralPath $link)) "S12: the update removed the link $link" }
    $differences = @(Compare-Snapshot $targetBefore (Get-TreeSnapshot $target))
    Assert-Check ($differences.Count -eq 0) 'S12: what the junctions pointed at is untouched by the update' $differences
    Test-ProductionInstalled $NewVersion 'S12 after the update'
    foreach ($link in $links) { New-Junction $link $target }
    $gone = Invoke-RegisteredUninstall $guid $folder
    Assert-Check $gone 'S12: the uninstall of an install with junctions in it removes the folder'
    $differences = @(Compare-Snapshot $targetBefore (Get-TreeSnapshot $target))
    Assert-Check ($differences.Count -eq 0) 'S12: what the junctions pointed at is untouched by the uninstall' $differences
    Test-ProductionRemoved 'S12'

    # An install folder that is itself a link: the folder that would be deleted is somewhere else, so nothing may be deleted (exit 9).
    $again = Invoke-Setup $installers['old'] '/S'
    Assert-Check ($again['exitCode'] -eq 0) 'S12: production is installed again' $again['exitCode']
    $real = Join-Path $programs 'tjommie-real'
    Rename-Item -LiteralPath $folder -NewName 'tjommie-real'
    New-Junction $folder $real
    $realBefore = Get-TreeSnapshot $real
    $redirected = Invoke-Setup (Join-Path $folder "Uninstall $($prod.product).exe") "/S _?=$folder"
    Assert-Check ($redirected['exitCode'] -eq 9) 'S12: an uninstaller whose folder is itself a link refuses (exit 9)' $redirected['exitCode']
    $blocked = Invoke-Setup $installers['new'] '/S --updated'
    Assert-Check ($blocked['exitCode'] -eq 9) 'S12: an update over an install folder that is itself a link refuses (exit 9)' $blocked['exitCode']
    $differences = @(Compare-Snapshot $realBefore (Get-TreeSnapshot $real))
    Assert-Check ($differences.Count -eq 0) 'S12: the folder the link pointed at is untouched' $differences
    [IO.Directory]::Delete($folder)
    Rename-Item -LiteralPath $real -NewName $prod.folder
    $gone = Invoke-RegisteredUninstall $guid $folder
    Assert-Check $gone 'S12: with the link gone, the install is removed'
    Remove-Item -LiteralPath $target -Recurse -Force -ErrorAction SilentlyContinue
    Test-DataUnchanged 'by the junction tests'
}

Invoke-Step 'S9 production, dev and updatetest do not touch each other' {
    # %TEMP% on a runner is spelled with an 8.3 name (RUNNER~1); the installer records the long form, so this uses it.
    $tempLong = Join-Path $env:LOCALAPPDATA 'Temp'
    $testFolder = Join-Path $tempLong "tjommie-acceptance\$($test.product)"
    $devFolder = Join-Path $programs $dev.folder
    foreach ($pair in @(@('production', $installers['old'], '/S'), @('dev', $installers['dev'], '/S'), @('updatetest', $installers['updatetest'], "/S /D=$testFolder"))) {
        $run = Invoke-Setup $pair[1] $pair[2]
        Assert-Check ($run['exitCode'] -eq 0) "S9: the $($pair[0]) installer exits 0" $run['exitCode']
    }
    $productionBefore = Get-TreeSnapshot $folder
    foreach ($pair in @(@($dev, $devFolder, $OldVersion), @($test, $testFolder, $OldVersion))) {
        $row = $pair[0]
        Assert-Check (Test-Path -LiteralPath (Join-Path $pair[1] $row.exe)) "S9: $($row.product) is installed in $($pair[1])"
        $uninstall = Get-RegistryKeyFact 'CurrentUser' (Get-UninstallKeyName $row.guid) '64'
        Assert-Check ($null -ne $uninstall) "S9: $($row.product) has its own Uninstall key $($row.guid)"
        Assert-Check ((Get-RegistryKeyFact 'CurrentUser' "Software\$($row.guid)" '64')['values'][0]['data'] -ieq $pair[1]) "S9: $($row.product)'s install key records its own folder"
        Test-Shortcut $row.product $true 'S9'
        $link = @(Get-ShortcutFacts $shortcutFolders['userStartMenu'] | Where-Object { $_['path'] -eq "$($row.product).lnk" })[0]
        Assert-Check ($link['appUserModelId'] -ceq $row.appId) "S9: $($row.product).lnk carries its own AppUserModelID" $link['appUserModelId']
    }
    Test-ProductionInstalled $OldVersion 'S9 production, with dev and updatetest installed'
    Test-NoRunValue 'S9'
    # Removing dev, then updatetest, leaves production exactly as it was.
    foreach ($row in @($dev, $test)) {
        $path = if ($row.guid -eq $dev.guid) { $devFolder } else { $testFolder }
        $gone = Invoke-RegisteredUninstall $row.guid $path
        Assert-Check $gone "S9: $($row.product) is removed by its own uninstaller"
        $differences = @(Compare-Snapshot $productionBefore (Get-TreeSnapshot $folder))
        Assert-Check ($differences.Count -eq 0) "S9: production's install folder is untouched by removing $($row.product)" $differences
        Assert-Check (Test-KeyExists (Get-UninstallKeyName $guid)) "S9: production's Uninstall key is kept after removing $($row.product)"
        Test-Shortcut $prod.product $true "S9 after removing $($row.product)"
    }
    # A folder this identity may not use is refused, and nothing is created.
    $bad = Join-Path $env:SystemDrive 'tjommie-bad'
    foreach ($arguments in @("/S /D=$bad\$($test.product)", "/S /D=$(Join-Path $tempLong 'x')\wrong-name")) {
        $refused = Invoke-Setup $installers['updatetest'] $arguments
        Assert-Check ($refused['exitCode'] -ne 0) "S9: updatetest with '$arguments' is refused (exit code is not 0)" $refused['exitCode']
        if ($refused['exitCode'] -ne 6) { Add-Note "S9: the refusal's exit code was $($refused['exitCode']), not the 6 the script sets with SetErrorLevel" $refused['exitCode'] }
    }
    Assert-Check (-not (Test-Path -LiteralPath $bad)) 'S9: the refused installs created nothing'
    Assert-Check (-not (Test-KeyExists (Get-UninstallKeyName $test.guid))) 'S9: ...and registered nothing'
    $gone = Invoke-RegisteredUninstall $guid $folder
    Assert-Check $gone 'S9: production is removed last'
    Test-DataUnchanged 'by the identities test'
}

Invoke-Step 'S10 /D= cannot move production''s install' {
    $bad = Join-Path $env:SystemDrive 'tjommie-bad'
    $run = Invoke-Setup $installers['old'] "/S /D=$bad\tjommie"
    Assert-Check ($run['exitCode'] -eq 0) 'S10: the installer exits 0' $run['exitCode']
    Assert-Check (Test-Path -LiteralPath (Join-Path $folder $prod.exe)) 'S10: production is installed in its own folder'
    Assert-Check (-not (Test-Path -LiteralPath $bad)) 'S10: nothing was created where /D= pointed'
    $gone = Invoke-RegisteredUninstall $guid $folder
    Assert-Check $gone 'S10: and removed'
    Test-DataUnchanged 'by the /D= test'
}

Complete-Acceptance
