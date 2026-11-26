# Compiles installer/tjommie.nsi into an installer for one identity (T8.2). It only compiles: it writes one .exe and runs nothing
# it wrote. The release workflow (T8.3) and the desktop-acceptance workflow call it on a GitHub-hosted runner, with NSIS
# installed there; a local run needs a makensis too and only produces a file.
#
#   -Identity    production | dev | updatetest: which row of installer/identities.json (appId, GUID, product, exe, folder).
#   -Version     the app's version, x.y.z; it names the Apps & features entry and fills the installer's version resource.
#   -PayloadDir  the folder whose contents are installed. It must hold the identity's exe.
#   -OutFile     where the installer is written.
#   -Makensis    the compiler; else $env:MAKENSIS, else makensis on PATH, else NSIS under Program Files.
#   -AllowElectronBuilderCache  also look in electron-builder's cache (an old NSIS, for a local compile check only; releases
#                never use it).
#   -RemoveRunValueOnUninstall  build the variant whose uninstaller removes the start-at-login value (see REMOVE_RUN_VALUE_ON_UNINSTALL
#                in tjommie.nsi). Not for a release while the owner's default (leave it, as R1 does) stands: the acceptance builds it
#                to keep that code tested.
param(
    [Parameter(Mandatory)][ValidateSet('production', 'dev', 'updatetest')][string]$Identity,
    [Parameter(Mandatory)][string]$Version,
    [Parameter(Mandatory)][string]$PayloadDir,
    [Parameter(Mandatory)][string]$OutFile,
    [string]$Makensis = $env:MAKENSIS,
    [switch]$AllowElectronBuilderCache,
    [switch]$RemoveRunValueOnUninstall
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Find-Makensis {
    if ($Makensis) {
        if (-not (Test-Path -LiteralPath $Makensis -PathType Leaf)) { throw "makensis was not found at $Makensis." }
        return (Resolve-Path -LiteralPath $Makensis).ProviderPath
    }
    $onPath = Get-Command makensis -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    $candidates = @()
    foreach ($base in $env:ProgramFiles, ${env:ProgramFiles(x86)}) { if ($base) { $candidates += Join-Path $base 'NSIS\makensis.exe' } }
    if ($AllowElectronBuilderCache -and $env:LOCALAPPDATA) {
        $candidates += @(Get-ChildItem -Path (Join-Path $env:LOCALAPPDATA 'electron-builder\Cache\nsis') -Filter makensis.exe -Recurse -ErrorAction SilentlyContinue | Sort-Object FullName -Descending | ForEach-Object { $_.FullName })
    }
    foreach ($candidate in $candidates) { if (Test-Path -LiteralPath $candidate -PathType Leaf) { return $candidate } }
    throw 'makensis was not found. Install NSIS, or pass -Makensis.'
}

if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "The version must be x.y.z, not '$Version'." }

$table = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'identities.json') -Raw | ConvertFrom-Json
$row = $table.identities.$Identity
if (-not $row) { throw "installer/identities.json has no identity '$Identity'." }

if (-not (Test-Path -LiteralPath $PayloadDir -PathType Container)) { throw "The payload folder $PayloadDir does not exist." }
$payload = (Resolve-Path -LiteralPath $PayloadDir).ProviderPath.TrimEnd('\')
if (-not (Test-Path -LiteralPath (Join-Path $payload $row.exe) -PathType Leaf)) { throw "The payload has no $($row.exe), the exe of the $Identity identity." }

$out = [IO.Path]::GetFullPath($OutFile)
New-Item -ItemType Directory -Force -Path (Split-Path -Parent $out) | Out-Null
if (Test-Path -LiteralPath $out) { Remove-Item -LiteralPath $out -Force }

$compiler = Find-Makensis
$arguments = @(
    '/V2', '/WX',
    "/DIDENTITY=$Identity",
    "/DAPP_ID=$($row.appId)",
    "/DAPP_GUID=$($row.guid)",
    "/DPRODUCT_NAME=$($row.product)",
    "/DEXE_NAME=$($row.exe)",
    "/DFOLDER_NAME=$($row.folder)",
    "/DROOT_POLICY=$($row.root)",
    "/DVERSION=$Version",
    "/DPUBLISHER=$($table.publisher)",
    "/DCOMMENTS=$($table.comments)",
    "/DPAYLOAD_DIR=$payload",
    "/DOUT_FILE=$out"
)
if ($RemoveRunValueOnUninstall) { $arguments += '/DREMOVE_RUN_VALUE_ON_UNINSTALL' }
$arguments += (Join-Path $PSScriptRoot 'tjommie.nsi') # the script is last
& $compiler @arguments
if ($LASTEXITCODE -ne 0) { throw "makensis failed with exit code $LASTEXITCODE." }
if (-not (Test-Path -LiteralPath $out -PathType Leaf)) { throw "makensis did not write $out." }
$file = Get-Item -LiteralPath $out
Write-Output ("{0}  {1} bytes  sha256 {2}" -f $file.FullName, $file.Length, (Get-FileHash -LiteralPath $out -Algorithm SHA256).Hash.ToLowerInvariant())
