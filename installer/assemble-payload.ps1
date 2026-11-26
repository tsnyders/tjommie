# Lays out the installer's payload from what the release workflow built (T8.3): the folder whose contents installer/tjommie.nsi installs.
# It only copies files; it runs nothing it copies. The layout is what R1's install facts (installer/r1-install-facts.json) and the Rust app expect:
#
#   Tjommie.exe                          the desktop binary (tjommie-desktop.exe, built with TJOMMIE_IDENTITY=production), renamed. The name is
#                                        -ExeName (default Tjommie.exe): the acceptance run's updatetest identity is "Tjommie Update Test.exe"
#   third-party-notices.txt              the notices (-ThirdPartyNotices, from scripts/make-third-party-notices.ts), BESIDE Tjommie.exe: the About
#                                        screen reads current_exe().with_file_name("third-party-notices.txt"), so it has to be at this level
#   resources\daemon\tjommie-daemon.exe  where desktop-host's packaged_daemon_path looks, and where R1 kept its daemon
#   resources\speech\<every file>        the verified speech bundle: whisper-cli.exe, three licences, MODEL-NOTICE.txt, manifest.json
#   resources\pdfium.dll                 the pinned pdfium (-Pdfium), for the PDF viewer
#   resources\pdfium-licenses\<files>    pdfium's licence texts (-PdfiumLicenses: its LICENSE and the texts of its licenses folder, 19 files), which
#                                        have to ship with the DLL. They are copied in here, before the manifest is written, so the manifest lists them
#
# A dry run without pdfium or the notices records what is missing as pending; -Release (the tag path) refuses to go without the DLL, its
# licence texts or the notices: a release never ships without its third-party notices.
#
# It never writes into a folder that has anything in it, so pdfium is fetched into a folder of its own first (tjommie-rs/tools/fetch-pdfium.ps1
# -Destination <a>\pdfium.dll -Licenses <a>\licenses), and it refuses any Electron file in the payload (the plan's "no Electron files").
# -ManifestPath (default: payload-manifest.json beside the payload folder, never inside it, so it is not installed) lists every file with
# its size and SHA-256, and what is still pending.
param(
    [Parameter(Mandatory)][string]$OutDir,
    [Parameter(Mandatory)][string]$Desktop,
    [Parameter(Mandatory)][string]$Daemon,
    [Parameter(Mandatory)][string]$SpeechDir,
    [string]$Pdfium = '',
    [string]$PdfiumLicenses = '',
    [string]$ThirdPartyNotices = '',
    [string]$ExeName = 'Tjommie.exe',
    [switch]$Release,
    [string]$ManifestPath = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$speechFiles = @('whisper-cli.exe', 'WHISPER-LICENSE.txt', 'GGML-LICENSE.txt', 'OPENAI-WHISPER-LICENSE.txt', 'MODEL-NOTICE.txt', 'manifest.json')
# Names that mean an Electron app. R1 shipped these; the native app must not.
$electronNames = @('*.asar', '*.pak', 'electron*', 'chrome_*', 'ffmpeg.dll', 'libEGL.dll', 'libGLESv2.dll', 'd3dcompiler_47.dll', 'vk_swiftshader*', 'snapshot_blob.bin', 'v8_context_snapshot.bin', 'locales')

if ($ExeName -notmatch '^[^\\/:*?"<>|]+\.exe$') { throw "The exe name must be a bare file name ending in .exe, not '$ExeName'." }
if (-not (Test-Path -LiteralPath $Desktop -PathType Leaf)) { throw "The desktop binary is missing: $Desktop" }
if (-not (Test-Path -LiteralPath $Daemon -PathType Leaf)) { throw "The daemon binary is missing: $Daemon" }
if (-not (Test-Path -LiteralPath $SpeechDir -PathType Container)) { throw "The speech bundle folder is missing: $SpeechDir" }
foreach ($name in $speechFiles) {
    if (-not (Test-Path -LiteralPath (Join-Path $SpeechDir $name) -PathType Leaf)) { throw "The speech bundle has no $name (in $SpeechDir)." }
}
if ($Pdfium -and -not (Test-Path -LiteralPath $Pdfium -PathType Leaf)) { throw "pdfium.dll was given but is not there: $Pdfium" }
if ($Release -and -not $Pdfium) { throw 'A release needs resources\pdfium.dll and none was given (-Pdfium). Build the payload without -Release for a dry run.' }
# pdfium's licence texts travel with the DLL. They are read here, before anything is written, so a bad folder writes nothing.
$licenseFiles = @()
if ($PdfiumLicenses) {
    if (-not $Pdfium) { throw "pdfium's licence texts were given (-PdfiumLicenses) without the DLL (-Pdfium): they ship together." }
    if (-not (Test-Path -LiteralPath $PdfiumLicenses -PathType Container)) { throw "The pdfium licence folder was given but is not there: $PdfiumLicenses" }
    $licenseRoot = (Get-Item -LiteralPath $PdfiumLicenses).FullName.TrimEnd('\')
    $licenseFiles = @(Get-ChildItem -LiteralPath $licenseRoot -Recurse -File -Force)
    if ($licenseFiles.Count -eq 0) { throw "The pdfium licence folder is empty: $PdfiumLicenses" }
    if (-not (Test-Path -LiteralPath (Join-Path $licenseRoot 'LICENSE') -PathType Leaf)) { throw "The pdfium licence folder has no LICENSE (pdfium's own, at the top of its archive): $PdfiumLicenses" }
}
if ($Release -and -not $PdfiumLicenses) { throw 'A release needs pdfium''s licence texts in resources\pdfium-licenses and none were given (-PdfiumLicenses). Build the payload without -Release for a dry run.' }
if ($ThirdPartyNotices) {
    if (-not (Test-Path -LiteralPath $ThirdPartyNotices -PathType Leaf)) { throw "The third-party notices were given but are not there: $ThirdPartyNotices" }
    $noticeBytes = (Get-Item -LiteralPath $ThirdPartyNotices).Length
    if ($noticeBytes -eq 0) { throw "The third-party notices are empty: $ThirdPartyNotices" }
    # The About screen refuses a notices file larger than its limit (4 MiB) and says they are unavailable, so one past it is not shipped.
    if ($noticeBytes -gt 4MB) { throw "The third-party notices are $noticeBytes bytes, more than the 4 MiB the About screen will show: $ThirdPartyNotices" }
}
if ($Release -and -not $ThirdPartyNotices) { throw 'A release needs third-party-notices.txt beside Tjommie.exe and none was given (-ThirdPartyNotices). Build the payload without -Release for a dry run.' }
if ((Test-Path -LiteralPath $OutDir) -and @(Get-ChildItem -LiteralPath $OutDir -Force).Count -gt 0) { throw "The payload folder $OutDir is not empty; it is never merged into." }

$full = [IO.Path]::GetFullPath($OutDir).TrimEnd('\')
if (-not $ManifestPath) { $ManifestPath = Join-Path (Split-Path -Parent $full) 'payload-manifest.json' }

# Nothing Electron may come in with the speech bundle or anywhere else: checked on the inputs before a single file is copied.
foreach ($pattern in $electronNames) {
    $found = @(Get-ChildItem -LiteralPath $SpeechDir -Filter $pattern -Force -Recurse -ErrorAction SilentlyContinue)
    if ($found.Count -gt 0) { throw "Refusing to build a payload with an Electron file in it: $($found[0].FullName) (matches $pattern)." }
}

New-Item -ItemType Directory -Force -Path (Join-Path $full 'resources\daemon'), (Join-Path $full 'resources\speech') | Out-Null
Copy-Item -LiteralPath $Desktop -Destination (Join-Path $full $ExeName)
Copy-Item -LiteralPath $Daemon -Destination (Join-Path $full 'resources\daemon\tjommie-daemon.exe')
foreach ($name in $speechFiles) { Copy-Item -LiteralPath (Join-Path $SpeechDir $name) -Destination (Join-Path $full "resources\speech\$name") }
$pending = @()
if ($Pdfium) { Copy-Item -LiteralPath $Pdfium -Destination (Join-Path $full 'resources\pdfium.dll') } else { $pending += 'resources\pdfium.dll' }
if ($licenseFiles.Count -gt 0) {
    foreach ($file in $licenseFiles) {
        if (-not $file.FullName.StartsWith($licenseRoot + '\', [StringComparison]::OrdinalIgnoreCase)) { throw "$($file.FullName) is not under $licenseRoot." }
        $target = Join-Path $full ('resources\pdfium-licenses\' + $file.FullName.Substring($licenseRoot.Length + 1))
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $target) | Out-Null
        Copy-Item -LiteralPath $file.FullName -Destination $target
    }
} else {
    $pending += 'resources\pdfium-licenses'
}
if ($ThirdPartyNotices) { Copy-Item -LiteralPath $ThirdPartyNotices -Destination (Join-Path $full 'third-party-notices.txt') } else { $pending += 'third-party-notices.txt' }

$files = @(Get-ChildItem -LiteralPath $full -Recurse -File -Force | Sort-Object FullName | ForEach-Object {
    [ordered]@{ path = $_.FullName.Substring($full.Length + 1); bytes = $_.Length; sha256 = (Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant() }
})
$manifest = [ordered]@{ files = $files; pending = $pending }
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $ManifestPath) | Out-Null
($manifest | ConvertTo-Json -Depth 5) | Set-Content -LiteralPath $ManifestPath -Encoding UTF8
Write-Output "Payload: $($files.Count) files in $full$(if ($pending.Count -gt 0) { "; pending: $($pending -join ', ')" })"
