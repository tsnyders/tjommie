# Checks a tool that the release build fetches against the SHA-256 values recorded in installer/tool-hashes.json (T8.3, review F1).
# Every other input of the build is pinned by content (actions by commit, cargo by Cargo.lock, npm by package-lock.json, the speech
# runtime by commit and a hashed download); this is the check for the tools that are pinned by version only. It reads files and
# writes one JSON file; it installs, fetches and publishes nothing.
#
#   -Tool nsis    -Path <the NSIS folder>   every file that can change what makensis writes (the compiler, its stubs, includes and
#                                           plugins) is hashed and compared with the recorded list: a changed, missing or extra file fails.
#   -Tool pdfium  -Path <pdfium.dll>        the one file is compared with the recorded hash (tool-hashes.json pdfium.sha256, the same value
#                                           as PDFIUM_SHA256 in the app). tjommie-rs/tools/fetch-pdfium.ps1 has already checked the archive
#                                           and the DLL inside it; this is the release's own second look at the file it is about to ship.
#
# A value that has not been recorded yet ("pending") passes in a dry run, with a warning and the observed values in -Observed (a
# dry run's artifact), so they can be recorded by a commit. With -Release a pending value fails: a release is never built with a tool
# nobody has pinned. A recorded value that does not match fails in both, and prints what was seen.
param(
    [Parameter(Mandatory)][ValidateSet('nsis', 'pdfium')][string]$Tool,
    [Parameter(Mandatory)][string]$Path,
    [string]$Manifest = (Join-Path $PSScriptRoot 'tool-hashes.json'),
    [string]$Observed,
    [switch]$Release
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Get-Sha256([string]$File) { return (Get-FileHash -LiteralPath $File -Algorithm SHA256).Hash.ToLowerInvariant() }

# A failure is written as it is, on stderr, and ends the script with exit 1. (A `throw` of a long message is printed word-wrapped inside
# PowerShell's error box, which is hard to read in a log and breaks the lines these differences are listed on.)
function Fail([string]$Message) {
    [Console]::Error.WriteLine($Message)
    exit 1
}

$all = Get-Content -LiteralPath $Manifest -Raw | ConvertFrom-Json
$entry = $all.PSObject.Properties[$Tool]
if ($null -eq $entry) { throw "$Manifest has no entry for $Tool." }
$entry = $entry.Value

# What is there, as name -> SHA-256. A tree is read from one root as the file system spells it, so an 8.3 alias in $Path cannot leave
# debris in the names (the same fault as accept-lib's relative paths).
$seen = [ordered]@{}
if ($Tool -eq 'nsis') {
    if (-not (Test-Path -LiteralPath $Path -PathType Container)) { throw "The NSIS folder is missing: $Path" }
    $root = (Get-Item -LiteralPath $Path).FullName.TrimEnd('\')
    $rows = @()
    foreach ($part in @($entry.include)) {
        $full = Join-Path $root $part
        if (Test-Path -LiteralPath $full -PathType Container) { $items = @(Get-ChildItem -LiteralPath $full -Recurse -File -Force) }
        elseif (Test-Path -LiteralPath $full -PathType Leaf) { $items = @(Get-Item -LiteralPath $full -Force) }
        else { throw "NSIS is missing $part (looked in $root)." }
        foreach ($item in $items) {
            if (-not $item.FullName.StartsWith($root + '\', [StringComparison]::OrdinalIgnoreCase)) { throw "$($item.FullName) is not under $root." }
            $rows += [pscustomobject]@{ Name = $item.FullName.Substring($root.Length + 1); Sha256 = Get-Sha256 $item.FullName }
        }
    }
    foreach ($row in ($rows | Sort-Object -Property Name -CaseSensitive)) { $seen[$row.Name] = $row.Sha256 }
} else {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "pdfium.dll is missing: $Path" }
    $seen[(Split-Path -Leaf $Path)] = Get-Sha256 $Path
}

# What is recorded, the same way. An entry with no values is pending.
$recorded = [ordered]@{}
if ($Tool -eq 'nsis') {
    if ($null -ne $entry.files) { foreach ($property in $entry.files.PSObject.Properties) { $recorded[$property.Name] = ([string]$property.Value).ToLowerInvariant() } }
} elseif ($entry.sha256) {
    $recorded[(Split-Path -Leaf $Path)] = ([string]$entry.sha256).ToLowerInvariant()
}

$problems = @()
$status = 'verified'
if ($recorded.Count -eq 0) {
    $status = 'pending'
} else {
    foreach ($name in $recorded.Keys) {
        if (-not $seen.Contains($name)) { $problems += "missing: $name" }
        elseif ($seen[$name] -ne $recorded[$name]) { $problems += "changed: $name`n      recorded $($recorded[$name])`n      observed $($seen[$name])" }
    }
    foreach ($name in $seen.Keys) { if (-not $recorded.Contains($name)) { $problems += "extra: $name (not in the recorded list)" } }
}

if ($Observed) {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent ([IO.Path]::GetFullPath($Observed))) | Out-Null
    ([ordered]@{ tool = $Tool; version = $entry.version; status = $(if ($problems.Count -gt 0) { 'mismatch' } else { $status }); fileCount = $seen.Count; files = $seen } | ConvertTo-Json -Depth 4) | Set-Content -LiteralPath $Observed -Encoding UTF8
}

if ($problems.Count -gt 0) {
    $where = if ($Observed) { " The values seen are in $Observed." } else { '' }
    Fail ("$Tool does not match the hashes recorded in $(Split-Path -Leaf $Manifest) ($($problems.Count) difference(s)):`n  " + ($problems -join "`n  ") + "`nIf this tool was changed on purpose, its new values have to be recorded by a person who has reviewed them.$where")
}
if ($status -eq 'pending') {
    $words = "$Tool has no recorded hashes in $(Split-Path -Leaf $Manifest) yet ($($seen.Count) observed)."
    if ($Release) { Fail "$words A release is not built with a tool nobody has pinned: record them from a dry run's artifact first." }
    Write-Output "::warning::$words A dry run passes; a release will not."
    return
}
Write-Output "$Tool matches the $($seen.Count) recorded SHA-256 value(s)."
