# The release job's check of the production binaries (T8.3; the M0 review's N3, which asks for the same two string checks rust-dev.yml makes for
# a dev build, turned round for a release): the desktop binary carries the update feed address and not the e2e test driver, and the daemon has
# no test driver either and no provider-fixtures build marker. A production build is the only one that has the feed (spec section 3.10), and a
# test driver in a shipped binary would let any same-user process click and type in the app (T0.5: `tjommie-ui-` is its pipe name). The
# provider-fixtures build (the daemon's `provider-fixtures` feature) swaps the real claude and codex for scripted fixtures and is never shipped:
# a compile_error! refuses it for the production identity, and this is the check, on the finished file, that it is not in the daemon (the
# provider-fixtures review's F2).
#
# It is run on the raw executables, straight after cargo builds them and before NSIS compresses anything into an installer, so a string is
# where the compiler put it. It reads the files and runs nothing. Every failing check is reported, not only the first.
#
# The absence checks would pass whatever the binaries held if the search could find nothing, so before they are trusted the search is shown
# to find: a file of its own making, with binary bytes round every string looked for here, is planted in the job's temp folder, read the same
# way, and each string has to be found in it. If one is not, nothing is checked and the script fails.
param(
    [Parameter(Mandatory)][string]$Desktop,
    [Parameter(Mandatory)][string]$Daemon
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# identity.rs's PRODUCTION_FEED: GitHub Releases' latest.yml for this repository.
$feed = 'https://github.com/tsnyders/tjommie/releases/latest/download/latest.yml'
$driver = 'tjommie-ui-'
# provider_fixtures.rs's BUILD_MARKER, present in a daemon built with the provider-fixtures feature and in no other.
$fixturesMarker = 'TJOMMIE-PROVIDER-FIXTURES-BUILD-7f3a9c41'

function Read-Binary([string]$Path, [string]$Label) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "The $Label binary is missing: $Path" }
    # One character per byte, so a string is found wherever it sits (Rust string literals are UTF-8; these ones are ASCII).
    return [Text.Encoding]::GetEncoding(28591).GetString([IO.File]::ReadAllBytes($Path))
}

# Plants a file holding each string among binary bytes (NULs, 0xFF, bytes that are not valid text), reads it the way the binaries are read, and
# fails unless every string is found. The planted file is removed whatever happens.
function Assert-SearchFinds([string[]]$Strings) {
    $folder = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
    $planted = Join-Path $folder ('tjommie-n3-selftest-' + [Guid]::NewGuid().ToString('N') + '.bin')
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
            if (-not $text.Contains($string)) {
                throw "The binary search could not find a string planted in a file of its own making ($string), so its absence checks would pass whatever the binaries hold. Nothing was checked."
            }
        }
    } finally {
        if (Test-Path -LiteralPath $planted) { Remove-Item -LiteralPath $planted -Force }
    }
}

Assert-SearchFinds @($feed, $driver, $fixturesMarker)

$problems = @()
$desktopText = Read-Binary $Desktop 'desktop'
if (-not $desktopText.Contains($feed)) { $problems += "The desktop binary does not contain the update feed address ($feed): this is not a production build." }
if ($desktopText.Contains($driver)) { $problems += "The desktop binary contains the test driver ($driver): it was built with the test-driver feature." }
$daemonText = Read-Binary $Daemon 'daemon'
if ($daemonText.Contains($driver)) { $problems += "The daemon binary contains the test driver ($driver)." }
if ($daemonText.Contains($fixturesMarker)) { $problems += "The daemon binary contains the provider-fixtures build marker ($fixturesMarker): it was built with the provider-fixtures feature, which is never shipped." }

if ($problems.Count -gt 0) { throw ($problems -join [Environment]::NewLine) }
Write-Output 'The desktop binary has the update feed address and no test driver; the daemon has no test driver and no provider fixtures.'
