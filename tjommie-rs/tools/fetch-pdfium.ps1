<#
Fetches pdfium.dll for the PDF viewer (Phase 3 plan T4.4): pdfium-win-x64.tgz from the pinned bblanchon/pdfium-binaries
release, checked by SHA-256, then its bin/pdfium.dll, checked by SHA-256 too, copied to -Destination. The download and
the extraction happen in a new, empty temporary folder that is removed afterwards. Any mismatch throws and leaves
-Destination as it was. On success it prints the DLL's full path.

    .\tools\fetch-pdfium.ps1                                         # to tjommie-rs\resources\pdfium\pdfium.dll
    .\tools\fetch-pdfium.ps1 -Destination "$env:RUNNER_TEMP\pdfium\pdfium.dll"

$release and $dllSha256 are PDFIUM_RELEASE and PDFIUM_SHA256 in crates\desktop-host\src\pdf.rs: change them together.
To pin a new release, run with the new $release: the error names the archive's hash; pin it, and the next run names
the DLL's.
#>
param([string]$Destination = (Join-Path (Split-Path $PSScriptRoot -Parent) 'resources\pdfium\pdfium.dll'))
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$release = 'chromium/8125'
# The digest GitHub publishes for the release asset (2026-10-26).
$archiveSha256 = '9a964261760cff52570aabf038ab346d3a5034dd8b30d0d5fe8861235d356b8b'
$dllSha256 = '4b0dec92555d69437bef0df37be630181ab3e83a6823ca264ede355a4f2ef994'
$url = "https://github.com/bblanchon/pdfium-binaries/releases/download/$([Uri]::EscapeDataString($release))/pdfium-win-x64.tgz"
# Windows' own bsdtar, by its full path: Git's GNU tar, often first on the PATH, reads C:\... as a remote host.
$tar = Join-Path $env:SystemRoot 'System32\tar.exe'

$scratch = Join-Path ([IO.Path]::GetTempPath()) ('tjommie-pdfium-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $scratch | Out-Null
try {
    $archive = Join-Path $scratch 'pdfium-win-x64.tgz'
    Invoke-WebRequest -Uri $url -OutFile $archive
    $actual = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $archiveSha256) { throw "pdfium-win-x64.tgz ($release) has SHA-256 $actual, not the pinned $archiveSha256." }
    & $tar -xzf $archive -C $scratch bin/pdfium.dll
    if ($LASTEXITCODE -ne 0) { throw 'Could not extract bin/pdfium.dll from the pdfium archive.' }
    $dll = Join-Path $scratch 'bin\pdfium.dll'
    if (-not (Test-Path -LiteralPath $dll -PathType Leaf)) { throw 'The pdfium archive has no bin/pdfium.dll.' }
    $actual = (Get-FileHash -LiteralPath $dll -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $dllSha256) { throw "bin/pdfium.dll ($release) has SHA-256 $actual, not the pinned $dllSha256." }
    $folder = Split-Path $Destination -Parent
    if ($folder) { New-Item -ItemType Directory -Path $folder -Force | Out-Null }
    Copy-Item -LiteralPath $dll -Destination $Destination -Force
    (Resolve-Path -LiteralPath $Destination).Path
} finally {
    Remove-Item -LiteralPath $scratch -Recurse -Force
}
