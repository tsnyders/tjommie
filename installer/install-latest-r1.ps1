# Installs the latest published R1 on a fresh GitHub-hosted runner, silently (T8.1), and says where it went. The facts
# collector then records what that install looks like. This script never runs anywhere else: the first thing it does is
# refuse on anything but a hosted runner, and then on one that already has a Tjommie on it.
#
# Environment: GH_TOKEN and GH_REPO (the repository the release is in), as `gh` needs them. R1_TAG, optional: a release tag to
# use instead of the latest one.
param([string]$OutDir = '')

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

. (Join-Path $PSScriptRoot 'hosted-runner.ps1')
. (Join-Path $PSScriptRoot 'scoped-processes.ps1')
Assert-HostedRunner
Assert-FreshRunner

if (-not $env:GH_REPO) { throw 'GH_REPO is not set.' }
if (-not $OutDir) { $OutDir = Join-Path $env:RUNNER_TEMP 'r1-latest' }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

# The release R1 is: the latest one that is neither a draft nor a pre-release, unless a tag was asked for.
$tag = $env:R1_TAG
if (-not $tag) {
    $latest = gh release view --json tagName,isPrerelease,isDraft | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Could not look up the latest release.' }
    if ($latest.isPrerelease -or $latest.isDraft) { throw "The latest release $($latest.tagName) is not a published release." }
    $tag = $latest.tagName
}
gh release download $tag --dir $OutDir --pattern 'Tjommie-Setup-*.exe' --pattern 'latest.yml'
if ($LASTEXITCODE -ne 0) { throw "Could not download release $tag." }

# latest.yml names the installer and its checksum, and must agree with the tag and with the file that came down.
$lines = Get-Content -LiteralPath (Join-Path $OutDir 'latest.yml')
$version = ($lines | Where-Object { $_ -match '^version:\s*(.+)$' } | ForEach-Object { $Matches[1].Trim().Trim("'", '"') } | Select-Object -First 1)
if (-not $version) { throw 'latest.yml has no version.' }
if ($tag -ne "v$version") { throw "Release $tag holds a latest.yml for version $version." }
$name = "Tjommie-Setup-$version.exe"
$installer = Join-Path $OutDir $name
if ($lines -notcontains "path: $name") { throw "latest.yml does not name $name." }
if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) { throw "$name was not in release $tag." }
$sha512 = [Convert]::ToBase64String([Convert]::FromHexString((Get-FileHash -LiteralPath $installer -Algorithm SHA512).Hash))
if (@($lines | Where-Object { $_.Trim() -eq "sha512: $sha512" }).Count -eq 0) { throw 'latest.yml SHA-512 does not match the downloaded installer.' }

# A silent NSIS install can return before its last files are written, so the folder counts as installed once its file count
# stops changing.
function Find-InstalledFolder {
    $programs = Join-Path $env:LOCALAPPDATA 'Programs'
    if (-not (Test-Path -LiteralPath $programs)) { return $null }
    $found = @(Get-ChildItem -LiteralPath $programs -Directory | Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'Tjommie.exe') })
    if ($found.Count -gt 1) { throw "More than one Tjommie install under ${programs}: $($found.Name -join ', ')" }
    if ($found.Count -eq 1) { return $found[0].FullName }
    return $null
}

$run = Start-Process -FilePath $installer -ArgumentList '/S' -Wait -PassThru
if ($run.ExitCode -ne 0) { throw "The installer failed ($($run.ExitCode))." }
$folder = $null
$previous = -1
for ($attempt = 0; $attempt -lt 30; $attempt++) {
    Start-Sleep -Seconds 2
    $folder = Find-InstalledFolder
    if ($folder) {
        $count = @(Get-ChildItem -LiteralPath $folder -Recurse -File -Force).Count
        if ($count -eq $previous) { break }
        $previous = $count
    }
    $folder = $null
}
if (-not $folder) { throw 'The install did not settle.' }
# The installer may have started the app; the collector starts it itself, once, when it is ready. Only what runs from this
# install folder is stopped (and its children), never anything by name.
if ((Split-Path -Parent ([IO.Path]::GetFullPath($folder).TrimEnd('\'))) -ine (Join-Path $env:LOCALAPPDATA 'Programs')) { throw "$folder is not an install folder under %LOCALAPPDATA%\Programs." }
Stop-ProcessesInFolder $folder | Out-Null

$result = [ordered]@{
    tag                 = $tag
    version             = $version
    installerName       = $name
    installerPath       = $installer
    installerBytes      = (Get-Item -LiteralPath $installer).Length
    installerSha512     = $sha512
    installerExitCode   = $run.ExitCode
    installFolder       = $folder
    latestYml           = ($lines -join "`n")
}
$resultPath = Join-Path $OutDir 'install-result.json'
$result | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath $resultPath -Encoding UTF8
if ($env:GITHUB_ENV) { "R1_INSTALL_RESULT=$resultPath" | Out-File -FilePath $env:GITHUB_ENV -Append -Encoding utf8 }
Write-Output "R1 $version is installed in $folder."
