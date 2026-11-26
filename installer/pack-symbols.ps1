# Packs the release's debug symbols into one zip, Tjommie-<version>-symbols.zip: the PDB of the desktop app and of the daemon, the files that
# turn a crash's addresses into function names and lines. They are a separate release asset, never part of the installer (they are big, and
# the app does not need them to run). It reads the PDBs and writes one zip; it runs and installs nothing.
#
#   ./installer/pack-symbols.ps1 -OutFile dist-desktop\installer\Tjommie-0.6.0-symbols.zip -Pdb <desktop.pdb>, <daemon.pdb> [-Release]
#
# The zip holds each PDB at its top level, under its own name, and nothing else. A release (-Release, the tag path) needs every PDB it was
# given; a dry run packs the ones that exist and warns about the others, listing what the folder does hold so the real names can be seen. No
# PDB at all is an error either way: an empty symbols asset is worse than none.
param(
    [Parameter(Mandatory)][string]$OutFile,
    [Parameter(Mandatory)][string[]]$Pdb,
    [switch]$Release
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ((Split-Path -Leaf $OutFile) -notmatch '^Tjommie-.+-symbols\.zip$') { throw "The symbols zip has to be named Tjommie-<version>-symbols.zip, not $(Split-Path -Leaf $OutFile)." }
if (Test-Path -LiteralPath $OutFile) { throw "$OutFile is already there; it is never overwritten." }

$present = @()
$missing = @()
foreach ($file in $Pdb) {
    if ($file -notmatch '\.pdb$') { throw "$file is not a .pdb file." }
    if (Test-Path -LiteralPath $file -PathType Leaf) { $present += (Get-Item -LiteralPath $file).FullName } else { $missing += $file }
}
if (@($present | ForEach-Object { Split-Path -Leaf $_ } | Select-Object -Unique).Count -ne $present.Count) { throw 'Two of the PDBs have the same name, so they would collide in the zip.' }

if ($missing.Count -gt 0) {
    $seen = @($Pdb | ForEach-Object { Split-Path -Parent $_ } | Where-Object { $_ -and (Test-Path -LiteralPath $_ -PathType Container) } | Select-Object -Unique |
        ForEach-Object { Get-ChildItem -LiteralPath $_ -Filter '*.pdb' -File -ErrorAction SilentlyContinue | ForEach-Object { $_.FullName } })
    $held = if ($seen.Count -gt 0) { "The folder holds: $($seen -join ', ')." } else { 'No .pdb file was found beside them.' }
    $words = "No PDB at $($missing -join ', '). $held"
    if ($Release) { throw "A release ships the symbols of every binary. $words" }
    if ($present.Count -eq 0) { throw $words }
    Write-Output "::warning::$words A dry run packs what exists; a release will not."
}

New-Item -ItemType Directory -Force -Path (Split-Path -Parent ([IO.Path]::GetFullPath($OutFile))) | Out-Null
# Windows' own bsdtar by its full path (Git's GNU tar, often first on the PATH, reads C:\... as a remote host); -a makes the .zip a zip. One
# -C per file, so the entries are bare names and the PDBs need not share a folder.
$tar = Join-Path $env:SystemRoot 'System32\tar.exe'
$arguments = @('-a', '-c', '-f', [IO.Path]::GetFullPath($OutFile))
foreach ($file in $present) { $arguments += '-C', (Split-Path -Parent $file), (Split-Path -Leaf $file) }
& $tar @arguments
if ($LASTEXITCODE -ne 0) { throw 'Could not write the symbols zip.' }

# Read it back: every PDB is in it, and nothing else.
$entries = @(& $tar -tf ([IO.Path]::GetFullPath($OutFile)))
if ($LASTEXITCODE -ne 0) { throw 'Could not read the symbols zip back.' }
$expected = @($present | ForEach-Object { Split-Path -Leaf $_ } | Sort-Object)
$actual = @($entries | ForEach-Object { $_.Trim() } | Where-Object { $_ } | Sort-Object)
if (($expected -join '|') -ne ($actual -join '|')) { throw "The symbols zip holds [$($actual -join ', ')], not [$($expected -join ', ')]." }
$zip = Get-Item -LiteralPath $OutFile
Write-Output "Symbols: $($present.Count) PDB(s) in $($zip.Name), $($zip.Length) bytes."
