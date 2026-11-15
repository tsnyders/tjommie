# The release job's check of the production binaries (T8.3; the M0 review's N3, which asks for the same two string checks rust-dev.yml makes for
# a dev build, turned round for a release): the desktop binary carries the update feed address and not the e2e test driver, and the daemon has
# no test driver either. A production build is the only one that has the feed (spec section 3.10), and a test driver in a shipped binary would
# let any same-user process click and type in the app (T0.5: `tjommie-ui-` is its pipe name).
#
# It reads the files and runs nothing. Every failing check is reported, not only the first.
param(
    [Parameter(Mandatory)][string]$Desktop,
    [Parameter(Mandatory)][string]$Daemon
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# identity.rs's PRODUCTION_FEED: GitHub Releases' latest.yml for this repository.
$feed = 'https://github.com/tsnyders/tjommie/releases/latest/download/latest.yml'
$driver = 'tjommie-ui-'

function Read-Binary([string]$Path, [string]$Label) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "The $Label binary is missing: $Path" }
    # One character per byte, so a string is found wherever it sits (Rust string literals are UTF-8; these ones are ASCII).
    return [Text.Encoding]::GetEncoding(28591).GetString([IO.File]::ReadAllBytes($Path))
}

$problems = @()
$desktopText = Read-Binary $Desktop 'desktop'
if (-not $desktopText.Contains($feed)) { $problems += "The desktop binary does not contain the update feed address ($feed): this is not a production build." }
if ($desktopText.Contains($driver)) { $problems += "The desktop binary contains the test driver ($driver): it was built with the test-driver feature." }
$daemonText = Read-Binary $Daemon 'daemon'
if ($daemonText.Contains($driver)) { $problems += "The daemon binary contains the test driver ($driver)." }

if ($problems.Count -gt 0) { throw ($problems -join [Environment]::NewLine) }
Write-Output 'The desktop binary has the update feed address and no test driver; the daemon has no test driver.'
