# Helpers shared by the T8.4 acceptance scripts (accept-update-path.ps1, accept-production-over-r1.ps1). Dot-source this file AFTER accept-lib.ps1;
# it defines functions only and runs nothing. The scripts that use it start with the hosted-runner guard, so nothing here ever runs on a real PC.
#
# Display goes through Write-Host: a function here returns its value, and anything it wrote to the output stream would become part of it.

Set-StrictMode -Version Latest

# Runs scripts/phase3-acceptance.ts with the arguments and shows every line it prints. Returns the exit code, the lines, and the last line that
# is a JSON object, parsed (verify and notice print one), or $null.
function Invoke-Phase3Driver([string[]]$Arguments) {
    $driver = Join-Path (Split-Path -Parent $PSScriptRoot) 'scripts\phase3-acceptance.ts'
    # stderr is merged in, and a native command's stderr must not stop a script that has ErrorActionPreference Stop (Windows PowerShell 5.1 does).
    $previous = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $lines = @(& node --disable-warning=ExperimentalWarning $driver @Arguments 2>&1 | ForEach-Object { "$_" })
        $code = $LASTEXITCODE
    } finally { $ErrorActionPreference = $previous }
    foreach ($line in $lines) { Write-Host "    $line" }
    $json = $null
    $last = @($lines | Where-Object { $_.TrimStart().StartsWith('{') }) | Select-Object -Last 1
    if ($last) { try { $json = $last | ConvertFrom-Json } catch { $json = $null } }
    return [ordered]@{ exitCode = $code; lines = $lines; json = $json }
}

# The processes whose executable is exactly $Exe, with their command lines (CIM is where Windows keeps them). $Exe is taken to its long form,
# because a hosted runner's temp folder is spelled with an 8.3 alias (RUNNER~1) and a process's path is not.
function Get-ProcessesOfExe([string]$Exe) {
    $long = Get-LongPath $Exe
    return @(Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -and ($_.ExecutablePath -ieq $long) })
}

# Whether two paths are the same folder, spelled however: the long form of each is compared (a hosted runner's temp folder has an 8.3 alias, and an
# installer writes the long form). An empty path is no folder.
function Test-SameLongFolder([string]$A, [string]$B) {
    if (-not $A -or -not $B) { return $false }
    return ((Get-LongPath $A).TrimEnd('\') -ieq (Get-LongPath $B).TrimEnd('\'))
}

# The value of a registry fact's named value (what Get-RegistryKeyFact returns), or $null.
function Get-FactValue($Fact, [string]$Name) {
    if ($null -eq $Fact) { return $null }
    $value = @($Fact['values'] | Where-Object { $_['name'] -ceq $Name }) | Select-Object -First 1
    if ($null -eq $value) { return $null }
    return $value['data']
}

# What an install of the real payload holds: every file in the payload manifest (assemble-payload.ps1 writes it) and the uninstaller NSIS adds.
# Relative, backslashes, sorted.
function Get-ExpectedPayloadFiles([string]$ManifestPath, [string]$Product) {
    $manifest = Read-Json $ManifestPath
    $files = @($manifest.files | ForEach-Object { [string]$_.path })
    return @($files + "Uninstall $Product.exe" | Sort-Object)
}

# Checks an install folder against that: the payload and the uninstaller and nothing else, and no Electron file among them.
function Test-InstalledPayload([string]$Folder, [string]$ManifestPath, [string]$Product, [string]$Label) {
    $files = @(Get-InstalledFiles $Folder)
    $expected = @(Get-ExpectedPayloadFiles $ManifestPath $Product)
    Assert-Check (($files -join '|') -ceq ($expected -join '|')) "$Label`: the install holds the new payload and the uninstaller, and nothing else" @{ files = $files.Count; expected = $expected.Count; extra = @($files | Where-Object { $expected -notcontains $_ }); missing = @($expected | Where-Object { $files -notcontains $_ }) }
    $electron = @($files | Where-Object { $_ -match '\.asar$|\.pak$|^locales\\|^resources\\app' -or $_ -match '^(ffmpeg|libEGL|libGLESv2|d3dcompiler_47)\.dll$' })
    Assert-Check ($electron.Count -eq 0) "$Label`: no Electron file is left in the install" $electron
    # Names are not enough: R1 and the new payload have paths in common (the daemon, the speech files), and a file of R1's left in place under one of
    # them would pass a check of names. So every file is compared with the build's own SHA-256 from the payload manifest: the daemon that is there is
    # the one that was built, never R1's.
    $built = @((Read-Json $ManifestPath).files)
    $different = @($built | Where-Object {
        $installed = Join-Path $Folder ([string]$_.path)
        -not (Test-Path -LiteralPath $installed -PathType Leaf) -or ((Get-FileHash -LiteralPath $installed -Algorithm SHA256).Hash.ToLowerInvariant() -ne ([string]$_.sha256).ToLowerInvariant())
    } | ForEach-Object { [string]$_.path })
    Assert-Check ($different.Count -eq 0) "$Label`: every installed file is byte for byte the one the build made ($($built.Count) files compared by SHA-256), so none is a file of R1's left in place" $different
    Add-Note "$Label`: the install holds $($files.Count) files" $files.Count
}

# The new app wrote its device settings from R1's export (T1.6): theme, text size, density, paired PCs and pending ones as R1 exported them, and zoom
# at 100% (Electron keeps zoom out of the export). $FixturePath is the file R1 was given the values from.
function Test-CarryOver([string]$Root, [string]$FixturePath, [string]$Label) {
    $file = Join-Path $Root 'data\desktop-ui.json'
    $present = Wait-Until { Test-Path -LiteralPath $file } 120
    Assert-Check $present "$Label`: the new app wrote data\desktop-ui.json (it imports R1's export when that file is missing)" $file
    if (-not $present) { return }
    $ui = Read-Json $file
    $fixture = Read-Json $FixturePath
    foreach ($name in 'theme', 'textSize', 'density') {
        Assert-Check ([string]$ui.$name -ceq [string]$fixture.$name) "$Label`: $name carried over ($($fixture.$name))" $ui.$name
    }
    foreach ($list in 'hosts', 'pending') {
        $want = @($fixture.$list | ForEach-Object { "$($_.id)|$($_.name)|$($_.deviceId)|$($_.hostKey)" })
        $have = @($ui.$list | ForEach-Object { "$($_.id)|$($_.name)|$($_.deviceId)|$($_.hostKey)" })
        Assert-Check (($have -join ';') -ceq ($want -join ';')) "$Label`: the $list carried over ($($want.Count))" @{ want = $want; have = $have }
    }
    Assert-Check ([int]$ui.zoomPercent -eq 100) "$Label`: zoom starts at 100%" $ui.zoomPercent
}

# Records what verify saw (counts, files, the additions and the app files that changed) as a note, and fails the run if it found a problem.
function Test-DataRootAsBaseline([string]$Root, [string]$Baseline, [string]$ReportPath, [string]$Label, [string[]]$AllowDecrease = @()) {
    $arguments = @('verify', '--root', $Root, '--baseline', $Baseline, '--report', $ReportPath)
    if ($AllowDecrease.Count -gt 0) { $arguments += '--allow-decrease'; $arguments += ($AllowDecrease -join ',') }
    $result = Invoke-Phase3Driver $arguments
    $report = if (Test-Path -LiteralPath $ReportPath) { Read-Json $ReportPath } else { $null }
    Assert-Check ($result['exitCode'] -eq 0) "$Label`: the data root is as the baseline left it (counts, user_version, every file)" @($result['lines'] | Select-Object -Last 12)
    if ($null -ne $report) {
        Assert-Check ([int]$report.userVersion.after -eq [int]$report.userVersion.before) "$Label`: user_version is unchanged ($($report.userVersion.before))" $report.userVersion
        Add-Note "$Label`: user_version $($report.userVersion.before) -> $($report.userVersion.after); rows $($report.rows.before) -> $($report.rows.after); files $($report.files.before) -> $($report.files.after)" $report
    }
}

# A copy of the app's own logs next to the evidence, whatever happened: the first place to look when a step fails. Never fails the run.
function Save-AppLogs([string]$Root, [string]$OutDir, [string]$Label) {
    try {
        $logs = Join-Path $Root 'logs'
        if (Test-Path -LiteralPath $logs -PathType Container) { Copy-Item -LiteralPath $logs -Destination (Join-Path $OutDir "app-logs-$Label") -Recurse -Force }
    } catch { }
}

# What to look at when an app that should be running is not doing what it should (a daemon that never starts): the processes that run from its install
# folder with their parents and command lines, the data root's most recently written files, anything that looks like a crash log, the Application
# event log's recent errors, and the app's own log. Recorded as notes, never as passes or failures, and never fails the run.
function Add-AppDiagnostics([string]$InstallFolder, [string]$Root, [string]$Label) {
    try {
        $running = @(Get-CimInstance Win32_Process | Where-Object { $_.ExecutablePath -and (Test-PathInFolder $_.ExecutablePath $InstallFolder) })
        Add-Note "$Label`: the processes running from the install folder ($($running.Count))" @($running | ForEach-Object {
            [ordered]@{ id = $_.ProcessId; parent = $_.ParentProcessId; started = [string]$_.CreationDate; exe = $_.ExecutablePath; commandLine = $_.CommandLine }
        })
    } catch { Add-Note "$Label`: the process list could not be read: $($_.Exception.Message)" }
    try {
        $recent = @(Get-ChildItem -LiteralPath $Root -Recurse -Force -File -ErrorAction SilentlyContinue | Sort-Object LastWriteTime | Select-Object -Last 60)
        Add-Note "$Label`: the data root's most recently written files" @($recent | ForEach-Object { '{0} {1,10} {2}' -f $_.LastWriteTime.ToString('o'), $_.Length, $_.FullName.Substring($Root.Length) })
        $crash = @(Get-ChildItem -LiteralPath $Root -Recurse -Force -File -Include '*crash*', '*.dmp', '*panic*' -ErrorAction SilentlyContinue | ForEach-Object { $_.FullName.Substring($Root.Length) })
        Add-Note "$Label`: files that look like crash logs ($($crash.Count))" $crash
        $log = Join-Path $Root 'logs\desktop.log'
        if (Test-Path -LiteralPath $log) { Add-Note "$Label`: the app's log, last lines" @(Get-Content -LiteralPath $log -Tail 80) }
    } catch { Add-Note "$Label`: the data root could not be read: $($_.Exception.Message)" }
    try {
        $events = @(Get-WinEvent -FilterHashtable @{ LogName = 'Application'; Level = 1, 2 } -MaxEvents 20 -ErrorAction Stop | ForEach-Object {
            $text = ([string]$_.Message -replace '\s+', ' ')
            '{0} {1}: {2}' -f $_.TimeCreated.ToString('o'), $_.ProviderName, $text.Substring(0, [Math]::Min(300, $text.Length))
        })
        Add-Note "$Label`: the Application event log's last errors ($($events.Count))" $events
    } catch { Add-Note "$Label`: the Application event log could not be read: $($_.Exception.Message)" }
}
