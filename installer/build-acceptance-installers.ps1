# Builds every installer the acceptance runs need, from the stub payload (T8.2): production at two versions (so an update can be
# told from a reinstall), dev, and updatetest. Compiles only; run with Windows PowerShell (powershell.exe) after NSIS is installed.
#
#   <OutDir>\payload\...            the stub payloads (build-stub-payload.ps1)
#   <OutDir>\production-0.6.0.exe, production-0.6.1.exe, dev-0.6.0.exe, updatetest-0.6.0.exe
param(
    [Parameter(Mandatory)][string]$OutDir,
    [string]$OldVersion = '0.6.0',
    [string]$NewVersion = '0.6.1'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$payloads = Join-Path $OutDir 'payload'
& (Join-Path $PSScriptRoot 'build-stub-payload.ps1') -OutDir $payloads
$builds = @(
    @{ identity = 'production'; version = $OldVersion },
    @{ identity = 'production'; version = $NewVersion },
    @{ identity = 'dev'; version = $OldVersion },
    @{ identity = 'updatetest'; version = $OldVersion }
)
foreach ($build in $builds) {
    & (Join-Path $PSScriptRoot 'build-installer.ps1') -Identity $build.identity -Version $build.version -PayloadDir (Join-Path $payloads $build.identity) -OutFile (Join-Path $OutDir "$($build.identity)-$($build.version).exe")
}
