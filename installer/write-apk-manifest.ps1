# Writes the small manifest a DRY RUN uploads in place of the Android APK. The repository is public, so a build of unreleased code signed with the
# release key is never left downloadable: a dry run proves the exact release path (the real keystore, the signer check) and then keeps only
# the facts about the APK, not the APK.
#
#   ./installer/write-apk-manifest.ps1 -Apk dist-mobile/Tjommie-Android-0.6.0.apk -Out dist-mobile/apk-manifest.json -ExpectedSigner <sha-256>
#
# The manifest holds the APK's file name, size and SHA-256, the signer certificate's SHA-256 (after apksigner has verified the signature and
# exactly one signer is found; with -ExpectedSigner it must also be the recorded one), and the package, versionCode and versionName read from
# the APK itself with aapt. It reads the APK and writes one JSON file; it installs, runs and uploads nothing.
#
# -Apksigner and -Aapt default to build-tools 36.0.0 under ANDROID_HOME, as the release job's own signer check uses.
param(
    [Parameter(Mandatory)][string]$Apk,
    [Parameter(Mandatory)][string]$Out,
    [string]$ExpectedSigner = '',
    [string]$Apksigner = '',
    [string]$Aapt = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not $Apksigner) { $Apksigner = Join-Path $env:ANDROID_HOME 'build-tools/36.0.0/apksigner.bat' }
if (-not $Aapt) { $Aapt = Join-Path $env:ANDROID_HOME 'build-tools/36.0.0/aapt.exe' }

if (-not (Test-Path -LiteralPath $Apk -PathType Leaf)) { throw "The APK is missing: $Apk" }
$name = Split-Path -Leaf $Apk
if ($name -notmatch '^Tjommie-Android-\d+\.\d+\.\d+\.apk$') { throw "The APK has to be named Tjommie-Android-<version>.apk, not $name." }
foreach ($tool in $Apksigner, $Aapt) { if (-not (Test-Path -LiteralPath $tool -PathType Leaf)) { throw "The Android build tool is missing: $tool" } }

# The signature, and who signed it: exactly one signer.
$result = & $Apksigner verify --print-certs $Apk 2>&1
if ($LASTEXITCODE -ne 0) { throw 'apksigner verification failed.' }
$digests = @($result | ForEach-Object { if ("$_" -match '^Signer #\d+ certificate SHA-256 digest:\s*([0-9a-fA-F:]+)\s*$') { $Matches[1] } })
if ($digests.Count -ne 1) { throw 'apksigner must report exactly one SHA-256 signer digest.' }
$signer = ($digests[0] -replace ':', '').ToLowerInvariant()
Write-Output "::add-mask::$signer"
Write-Output "::add-mask::$($digests[0])"
$checked = $false
if ($ExpectedSigner) {
    $expected = ($ExpectedSigner -replace ':', '').ToLowerInvariant()
    if ($expected -notmatch '^[0-9a-f]{64}$') { throw 'The expected signer is not a SHA-256 digest.' }
    if ($signer -ne $expected) { throw 'APK signer does not match the expected signer.' }
    $checked = $true
}

# What the APK says it is.
$badging = @(& $Aapt dump badging $Apk 2>&1)
if ($LASTEXITCODE -ne 0) { throw 'aapt could not read the APK.' }
$package = @($badging | Where-Object { "$_" -match "^package: name='([^']+)' versionCode='(\d+)' versionName='([^']+)'" })
if ($package.Count -lt 1) { throw 'aapt did not report the package, versionCode and versionName.' }
[void]("$($package[0])" -match "^package: name='([^']+)' versionCode='(\d+)' versionName='([^']+)'")
$identity = [ordered]@{ package = $Matches[1]; versionCode = [int]$Matches[2]; versionName = $Matches[3] }

$manifest = [ordered]@{
    schema            = 1
    file              = $name
    bytes             = (Get-Item -LiteralPath $Apk).Length
    sha256            = (Get-FileHash -LiteralPath $Apk -Algorithm SHA256).Hash.ToLowerInvariant()
    signerSha256      = $signer
    signerMatchesExpected = $checked
    package           = $identity.package
    versionCode       = $identity.versionCode
    versionName       = $identity.versionName
}
New-Item -ItemType Directory -Force -Path (Split-Path -Parent ([IO.Path]::GetFullPath($Out))) | Out-Null
($manifest | ConvertTo-Json) | Set-Content -LiteralPath $Out -Encoding UTF8
Write-Output "APK manifest: $name, $($manifest.bytes) bytes, $($identity.package) $($identity.versionName) (versionCode $($identity.versionCode)), signer verified$(if ($checked) { ' and as recorded' })."
