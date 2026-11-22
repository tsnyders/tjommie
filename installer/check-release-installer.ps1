# The release job's check that an installer and its latest.yml agree (T8.3), the successor of the old "Check installer update metadata" step:
# the installer is named for the version, latest.yml names it everywhere it should, and holds its SHA-512 (base64) and size; no block map is
# published (electron-updater then downloads the whole installer, which T8.4 verifies); and the installer's own version resource carries the
# AppId the Rust updater insists on (installer/tjommie.nsi writes it with VIAddVersionKey). It reads the files and runs nothing.
#
# Every failing check is reported, not only the first.
param(
    [Parameter(Mandatory)][string]$Installer,
    [Parameter(Mandatory)][string]$LatestYml,
    [Parameter(Mandatory)][string]$Version,
    [string]$AppId = 'com.tsnyders.tjommie'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$name = "Tjommie-Setup-$Version.exe"
foreach ($path in @($Installer, $LatestYml)) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Missing release asset: $path" }
}
$problems = @()
if ((Split-Path -Leaf $Installer) -cne $name) { $problems += "The installer must be named $name, not $(Split-Path -Leaf $Installer)." }
if (Test-Path -LiteralPath "$Installer.blockmap") { $problems += "A block map sits beside the installer ($Installer.blockmap): none is published, so electron-updater falls back to a full download." }

$sha512 = [Convert]::ToBase64String([Security.Cryptography.SHA512]::Create().ComputeHash([IO.File]::ReadAllBytes($Installer)))
$size = (Get-Item -LiteralPath $Installer).Length
$lines = @(Get-Content -LiteralPath $LatestYml)
$trimmed = @($lines | ForEach-Object { $_.TrimEnd() })
if ($trimmed -notcontains "version: $Version") { $problems += "latest.yml does not say version: $Version." }
if ($trimmed -notcontains "path: $name") { $problems += "latest.yml does not say path: $name." }
if ($trimmed -notcontains "  - url: $name") { $problems += "latest.yml's files entry does not have url: $name." }
if ($trimmed -notcontains "    size: $size") { $problems += "latest.yml's files entry does not have size: $size (the installer's size)." }
# The SHA-512 must be there twice: in the files entry and at the top.
if (@($trimmed | Where-Object { $_ -match "^\s*(- )?sha512: $([regex]::Escape($sha512))$" }).Count -ne 2) { $problems += 'latest.yml does not hold the installer''s SHA-512 in both the files entry and at the top level.' }
if (@($lines | Where-Object { $_ -match 'blockMap' }).Count -gt 0) { $problems += 'latest.yml has a block map field.' }

# The version resource: the key AppId, and then its value, all of it, as a null-terminated UTF-16 string. A prefix of another app's id must
# not pass (com.tsnyders.tjommie is the start of the dev app's id). The resource is in the installer's first bytes, so only those are read.
$stream = [IO.File]::OpenRead($Installer)
try {
    $buffer = New-Object byte[] ([Math]::Min($stream.Length, 4MB))
    [void]$stream.Read($buffer, 0, $buffer.Length)
} finally { $stream.Close() }
$latin1 = [Text.Encoding]::GetEncoding(28591)
$text = $latin1.GetString($buffer)
$wideOf = { param([string]$Value) $latin1.GetString([Text.Encoding]::Unicode.GetBytes($Value)) }
$key = & $wideOf 'AppId'
$value = (& $wideOf $AppId) + "`0`0"
$found = $false
$at = $text.IndexOf($key, [StringComparison]::Ordinal)
while ($at -ge 0 -and -not $found) {
    $next = $at + $key.Length
    while ($next -lt $text.Length -and $text[$next] -eq "`0") { $next++ }
    if ($text.Substring($next).StartsWith($value, [StringComparison]::Ordinal)) { $found = $true }
    $at = $text.IndexOf($key, $at + 1, [StringComparison]::Ordinal)
}
if (-not $found) { $problems += "The installer's version resource has no AppId of $AppId (the Rust updater refuses an installer whose AppId is not its own app's)." }

if ($problems.Count -gt 0) { throw ($problems -join [Environment]::NewLine) }
Write-Output "$name matches its latest.yml ($size bytes, sha512 $sha512) and carries AppId $AppId."
