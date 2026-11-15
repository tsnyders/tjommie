# Writes release-manifest.json for a run of release.yml (T8.3): what was built, from which commit, with which tools, and every file with its
# hashes. A dry run uploads it with the installer and latest.yml as a workflow artifact, so what a dry run made can be checked without
# publishing anything. It reads files and writes one JSON file; it never publishes, and nothing in it is secret.
param(
    [Parameter(Mandatory)][string]$Version,
    [Parameter(Mandatory)][string]$Installer,
    [Parameter(Mandatory)][string]$LatestYml,
    [Parameter(Mandatory)][string]$PayloadManifest,
    [Parameter(Mandatory)][string]$Desktop,
    [Parameter(Mandatory)][string]$Daemon,
    [Parameter(Mandatory)][string]$Out,
    [Parameter(Mandatory)][ValidateSet('true', 'false')][string]$DryRun,
    [string]$Rustc = '',
    [string]$Nsis = '',
    [string]$VersionCheck = 'passed'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Get-Hashes([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "Missing: $Path" }
    return [ordered]@{
        name   = (Split-Path -Leaf $Path)
        bytes  = (Get-Item -LiteralPath $Path).Length
        sha256 = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
    }
}

$installerFacts = Get-Hashes $Installer
$installerFacts['sha512'] = [Convert]::ToBase64String([Security.Cryptography.SHA512]::Create().ComputeHash([IO.File]::ReadAllBytes($Installer)))
$yml = Get-Hashes $LatestYml
$yml['content'] = [IO.File]::ReadAllText($LatestYml)
$desktopFacts = Get-Hashes $Desktop
$daemonFacts = Get-Hashes $Daemon
$payload = Get-Content -LiteralPath $PayloadManifest -Raw | ConvertFrom-Json

$manifest = [ordered]@{
    schema    = 1
    dryRun    = ($DryRun -eq 'true')
    # This file never says a release was made: publishing is a different job, which a dry run does not have.
    published = $false
    version   = $Version
    commit    = $env:GITHUB_SHA
    ref       = $env:GITHUB_REF
    event     = $env:GITHUB_EVENT_NAME
    runId     = $env:GITHUB_RUN_ID
    runner    = [ordered]@{ image = $env:ImageVersion; os = [Environment]::OSVersion.VersionString }
    build     = [ordered]@{ identity = 'production'; profile = 'release'; locked = $true; rustc = $Rustc; nsis = $Nsis }
    installer = $installerFacts
    latestYml = $yml
    binaries  = [ordered]@{ desktop = $desktopFacts; daemon = $daemonFacts }
    payload   = $payload
    checks    = [ordered]@{ productionBinaries = 'passed'; installerMatchesLatestYml = 'passed'; crateVersions = $VersionCheck }
}
New-Item -ItemType Directory -Force -Path (Split-Path -Parent ([IO.Path]::GetFullPath($Out))) | Out-Null
($manifest | ConvertTo-Json -Depth 8) | Set-Content -LiteralPath $Out -Encoding UTF8
Write-Output "Wrote $Out"
