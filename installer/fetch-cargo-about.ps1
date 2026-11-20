# Fetches cargo-about, the tool that lists the Rust crates' licences for the installer's third-party notices, at the version and SHA-256
# recorded in installer/tool-hashes.json (the "cargo-about" entry). It downloads the archive into a new, empty scratch folder, checks the
# archive's SHA-256 BEFORE it extracts anything, extracts only cargo-about.exe, checks the exe's SHA-256 too (installer/check-tool-hashes.ps1
# -Tool cargo-about), and only then copies it to -Destination. Any mismatch stops with nothing written to -Destination. It never runs
# the tool, and it installs nothing. The scratch folder is removed afterwards.
#
#   ./installer/fetch-cargo-about.ps1 -Destination "$env:RUNNER_TEMP\cargo-about"      # leaves ...\cargo-about\cargo-about.exe
#
# -Url exists for the tests, which serve a stand-in archive from the local machine; the release leaves it out and gets the recorded source.
param(
    [Parameter(Mandatory)][string]$Destination,
    [string]$Manifest = (Join-Path $PSScriptRoot 'tool-hashes.json'),
    [string]$Url = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$entry = (Get-Content -LiteralPath $Manifest -Raw | ConvertFrom-Json).PSObject.Properties['cargo-about']
if ($null -eq $entry) { throw "$Manifest has no entry for cargo-about." }
$entry = $entry.Value
foreach ($field in 'version', 'source', 'archiveSha256', 'sha256') {
    if (-not $entry.$field) { throw "cargo-about's $field is not recorded in $(Split-Path -Leaf $Manifest); a release is not built with a tool nobody has pinned." }
}
if (-not $Url) { $Url = $entry.source }

if ((Test-Path -LiteralPath $Destination) -and @(Get-ChildItem -LiteralPath $Destination -Force).Count -gt 0) { throw "The folder $Destination is not empty; cargo-about is never fetched into a folder that has files in it." }

# Windows' own bsdtar, by its full path: Git's GNU tar, often first on the PATH, reads C:\... as a remote host.
$tar = Join-Path $env:SystemRoot 'System32\tar.exe'
$scratch = Join-Path ([IO.Path]::GetTempPath()) ('tjommie-cargo-about-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $scratch | Out-Null
try {
    $archive = Join-Path $scratch 'cargo-about.tar.gz'
    Invoke-WebRequest -Uri $Url -OutFile $archive
    $actual = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne ([string]$entry.archiveSha256).ToLowerInvariant()) {
        throw "The cargo-about archive has SHA-256 $actual, not the recorded $($entry.archiveSha256). Nothing was extracted."
    }
    # The archive holds one folder, cargo-about-<version>-x86_64-pc-windows-msvc, with the exe in it.
    $inside = "cargo-about-$($entry.version)-x86_64-pc-windows-msvc/cargo-about.exe"
    $unpacked = Join-Path $scratch 'unpacked'
    New-Item -ItemType Directory -Path $unpacked | Out-Null
    & $tar -xzf $archive -C $unpacked $inside
    if ($LASTEXITCODE -ne 0) { throw "Could not extract $inside from the cargo-about archive." }
    $exe = Join-Path $unpacked ($inside -replace '/', '\')
    if (-not (Test-Path -LiteralPath $exe -PathType Leaf)) { throw "The cargo-about archive has no $inside." }
    & (Join-Path $PSScriptRoot 'check-tool-hashes.ps1') -Tool cargo-about -Path $exe -Manifest $Manifest -Release
    if ($LASTEXITCODE -ne 0) { throw 'cargo-about.exe is not the file that was recorded.' }
    New-Item -ItemType Directory -Force -Path $Destination | Out-Null
    Copy-Item -LiteralPath $exe -Destination (Join-Path $Destination 'cargo-about.exe')
    Write-Output (Join-Path (Resolve-Path -LiteralPath $Destination).Path 'cargo-about.exe')
} finally {
    Remove-Item -LiteralPath $scratch -Recurse -Force
}
