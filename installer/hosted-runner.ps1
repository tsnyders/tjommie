# Guards for the scripts that install or uninstall Tjommie (T8.1). Dot-source this file; it defines functions only.
#
# Those scripts change an install and its registry keys, so they run only on a GitHub-hosted runner, a machine that is
# thrown away after the job. `act` and self-hosted runners also set GITHUB_ACTIONS=true, and either can be a PC with a real
# Tjommie on it; RUNNER_ENVIRONMENT is 'github-hosted' only on GitHub's own disposable runners.

Set-StrictMode -Version Latest

function Assert-HostedRunner {
    if ($env:GITHUB_ACTIONS -ne 'true' -or $env:RUNNER_ENVIRONMENT -ne 'github-hosted') {
        throw 'This script installs and uninstalls Tjommie, so it runs only on a GitHub-hosted runner (RUNNER_ENVIRONMENT=github-hosted). Refusing to run here.'
    }
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
