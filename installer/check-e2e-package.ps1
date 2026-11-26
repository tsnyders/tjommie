# The e2e job's check of the package it tests (T8.4 step 4; check-production-binaries.ps1 turned the other way round). The black-box e2e set
# drives a PACKAGED build through TJOMMIE_E2E_EXE, and that build has to be what the tests need and never what a release is:
#
#   the desktop exe  carries the test driver (`tjommie-ui-` is its pipe name) and NOT the update feed address, which only a production build has
#   the daemon       is the provider-fixtures build (its build marker is in it), has no test driver, and sits BESIDE the desktop exe as
#                    tjommie-daemon.exe, where the tests look for it (TJOMMIE_E2E_EXE's contract)
#
# A test driver in a shipped binary would let any same-user process click and type in the app, so this build is only ever tested, and
# check-production-binaries.ps1 is what proves a release has none. It reads the files and runs nothing. Every failing check is reported.
#
# The absence checks would pass whatever the files held if the search could find nothing, so before they are trusted the search is shown to
# find: a file of its own making, with binary bytes round every string, is planted, read the same way, and each string has to be found.
param(
    [Parameter(Mandatory)][string]$Desktop,
    [Parameter(Mandatory)][string]$Daemon
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$feed = 'https://github.com/tsnyders/tjommie/releases/latest/download/latest.yml'
$driver = 'tjommie-ui-'
$fixturesMarker = 'TJOMMIE-PROVIDER-FIXTURES-BUILD-7f3a9c41'

function Read-Binary([string]$Path, [string]$Label) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "The $Label binary is missing: $Path" }
    # One character per byte, so a string is found wherever it sits (Rust string literals are UTF-8; these ones are ASCII).
    return [Text.Encoding]::GetEncoding(28591).GetString([IO.File]::ReadAllBytes($Path))
}

function Assert-SearchFinds([string[]]$Strings) {
    $folder = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
    $planted = Join-Path $folder ('tjommie-e2e-selftest-' + [Guid]::NewGuid().ToString('N') + '.bin')
    try {
        $bytes = New-Object 'System.Collections.Generic.List[byte]'
        foreach ($string in $Strings) {
            $bytes.AddRange([byte[]](0, 0, 255, 128, 127, 1, 2))
            $bytes.AddRange([Text.Encoding]::ASCII.GetBytes($string))
            $bytes.AddRange([byte[]](255, 0, 195, 40, 0))
        }
        [IO.File]::WriteAllBytes($planted, $bytes.ToArray())
        $text = Read-Binary $planted 'planted'
        foreach ($string in $Strings) {
            if (-not $text.Contains($string)) { throw "The binary search could not find a string planted in a file of its own making ($string), so its absence checks would pass whatever the files hold. Nothing was checked." }
        }
    } finally {
        if (Test-Path -LiteralPath $planted) { Remove-Item -LiteralPath $planted -Force }
    }
}

Assert-SearchFinds @($feed, $driver, $fixturesMarker)

$problems = @()
if (-not [IO.Path]::IsPathRooted($Desktop)) { $problems += "The desktop exe path is not absolute ($Desktop): TJOMMIE_E2E_EXE has to be." }
if ((Split-Path -Leaf $Daemon) -ine 'tjommie-daemon.exe') { $problems += "The daemon has to be named tjommie-daemon.exe, not $(Split-Path -Leaf $Daemon): the tests look for that name." }
if ((Split-Path -Parent ([IO.Path]::GetFullPath($Desktop))) -ine (Split-Path -Parent ([IO.Path]::GetFullPath($Daemon)))) { $problems += 'The daemon has to sit beside the desktop exe, in the same folder: the tests look for it there.' }
$desktopText = Read-Binary $Desktop 'desktop'
if (-not $desktopText.Contains($driver)) { $problems += "The desktop binary has no test driver ($driver): it was built without the test-driver feature, and the e2e tests have nothing to drive." }
if ($desktopText.Contains($feed)) { $problems += "The desktop binary contains the update feed address ($feed): it is a production build, which is never the one the e2e set drives." }
$daemonText = Read-Binary $Daemon 'daemon'
if (-not $daemonText.Contains($fixturesMarker)) { $problems += "The daemon is not the provider-fixtures build ($fixturesMarker is not in it): the tests would start real providers." }
if ($daemonText.Contains($driver)) { $problems += "The daemon binary contains the test driver ($driver)." }

# Each problem on a line of its own, as it is: a `throw` of a long message is printed word-wrapped inside PowerShell's error box, which is hard to
# read in a log and breaks the sentences these are told apart by.
if ($problems.Count -gt 0) {
    [Console]::Error.WriteLine($problems -join [Environment]::NewLine)
    exit 1
}
Write-Output 'The desktop binary has the test driver and no update feed; the daemon is the provider-fixtures build, beside it.'
