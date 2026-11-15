# Guards for the scripts that install or uninstall Tjommie (T8.1, T8.2). Dot-source this file; it defines functions only.
#
# Those scripts change an install and its registry keys, so they run only on a GitHub-hosted runner, a machine that is
# thrown away after the job. `act` and self-hosted runners also set GITHUB_ACTIONS=true, and either can be a PC with a real
# Tjommie on it; RUNNER_ENVIRONMENT is 'github-hosted' only on GitHub's own disposable runners. But two environment variables are
# the easiest thing in the world to set by hand, so the guard also needs what a real hosted runner is, as T8.1 recorded it from
# one (installer/r1-install-facts.json and the install result): the user runneradmin, a RUNNER_TEMP under D:\a\_temp, and a run
# id and attempt. A real PC fails at least the user name and the temp folder, whatever its environment says.

Set-StrictMode -Version Latest

# Whether $Path is $Folder or inside it, spelled however Windows would resolve it. False for a path that cannot be resolved.
function Test-UnderFolder([string]$Path, [string]$Folder) {
    if (-not $Path) { return $false }
    try {
        $full = [IO.Path]::GetFullPath($Path).TrimEnd('\')
        $root = [IO.Path]::GetFullPath($Folder).TrimEnd('\')
    } catch { return $false }
    return ($full -ieq $root) -or $full.StartsWith($root + '\', [StringComparison]::OrdinalIgnoreCase)
}

# Why this is not a GitHub-hosted runner: one sentence for each condition that fails, or none. $Environment is a hashtable of the
# variables that matter (a missing one reads as empty); $UserName is the user the script runs as. Takes its facts as arguments so a
# test can try every missing condition without being another user.
function Get-HostedRunnerProblems([string]$UserName, [hashtable]$Environment) {
    $problems = @()
    if ($Environment['GITHUB_ACTIONS'] -ne 'true') { $problems += 'GITHUB_ACTIONS is not true' }
    if ($Environment['RUNNER_ENVIRONMENT'] -ne 'github-hosted') { $problems += 'RUNNER_ENVIRONMENT is not github-hosted' }
    if ($UserName -ne 'runneradmin') { $problems += 'the user is not runneradmin' }
    if (-not (Test-UnderFolder ([string]$Environment['RUNNER_TEMP']) 'D:\a\_temp')) { $problems += 'RUNNER_TEMP is not under D:\a\_temp' }
    if ([string]$Environment['GITHUB_RUN_ID'] -notmatch '^\d+$') { $problems += 'GITHUB_RUN_ID is not a run id' }
    if ([string]$Environment['GITHUB_RUN_ATTEMPT'] -notmatch '^\d+$') { $problems += 'GITHUB_RUN_ATTEMPT is not an attempt number' }
    return $problems
}

function Assert-HostedRunner {
    $environment = @{}
    foreach ($name in 'GITHUB_ACTIONS', 'RUNNER_ENVIRONMENT', 'RUNNER_TEMP', 'GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT') { $environment[$name] = [Environment]::GetEnvironmentVariable($name) }
    $problems = @(Get-HostedRunnerProblems ([Environment]::UserName) $environment)
    if ($problems.Count -gt 0) {
        throw "This script installs and uninstalls Tjommie, so it runs only on a GitHub-hosted runner. Refusing to run here: $($problems -join '; ')."
    }
}

# Why an install result (the JSON install-latest-r1.ps1 writes) is not this run's: it must carry this run's id and attempt, and say
# it was made on a fresh runner. $RunId and $RunAttempt default to this run's. A result that is hand-written, or left by another
# run, says which R1 to act on without having been checked, and is refused.
function Get-InstallResultBindingProblems($Result, [string]$RunId = $env:GITHUB_RUN_ID, [string]$RunAttempt = $env:GITHUB_RUN_ATTEMPT) {
    $problems = @()
    foreach ($pair in @(@('githubRunId', $RunId), @('githubRunAttempt', $RunAttempt))) {
        $name = $pair[0]
        $current = [string]$pair[1]
        $recorded = ''
        if ($null -ne $Result -and $null -ne $Result.PSObject.Properties[$name]) { $recorded = [string]$Result.PSObject.Properties[$name].Value }
        if (-not $recorded) { $problems += "the install result does not carry $name" }
        elseif (-not $current -or $recorded -ne $current) { $problems += "the install result is from another run ($name is $recorded, this run's is $current)" }
    }
    $fresh = $null -ne $Result -and $null -ne $Result.PSObject.Properties['fresh'] -and $Result.PSObject.Properties['fresh'].Value -eq $true
    if (-not $fresh) { $problems += 'the install result was not made on a fresh runner' }
    return $problems
}

function Assert-InstallResultBoundToThisRun($Result) {
    $problems = @(Get-InstallResultBindingProblems $Result)
    if ($problems.Count -gt 0) { throw "Refusing to act on this install result: $($problems -join '; ')." }
}

# A fresh runner has never seen Tjommie. Refuses if any trace of an install or of data is found, so a hosted runner that
# somehow was not fresh is not touched either.
function Assert-FreshRunner {
    $found = @()
    foreach ($path in @(
        (Join-Path $env:LOCALAPPDATA 'Tjommie'),
        (Join-Path $env:LOCALAPPDATA 'Programs\Tjommie'),
        (Join-Path $env:APPDATA 'Tjommie')
    )) {
        if (Test-Path -LiteralPath $path) { $found += $path }
    }
    $uninstall = [Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\Microsoft\Windows\CurrentVersion\Uninstall')
    if ($null -ne $uninstall) {
        foreach ($name in $uninstall.GetSubKeyNames()) {
            $entry = $uninstall.OpenSubKey($name)
            $display = $entry.GetValue('DisplayName')
            $entry.Close()
            if ($display -like 'Tjommie*') { $found += "HKCU Uninstall\$name" }
        }
        $uninstall.Close()
    }
    if ($found.Count -gt 0) { throw "This runner already has Tjommie on it, so it is not fresh: $($found -join ', ')" }
}
