# Moves every place that carries Tjommie's version to a new one, in one step, so a release is not one forgotten file away from a mismatch:
#
#   ./tools/bump-version.ps1 -Version 0.6.0
#
#   package.json                         the root, and every packages/*/package.json
#   package-lock.json                    the root entry (twice) and the seven workspace packages' entries
#   packages/mobile/android/app/build.gradle   versionName, and versionCode (the next one, from the table below)
#   packages/mobile/scripts/apply-native.mjs   the table of versionCodes gets ["<version>", <next code>] (Android never reuses a code)
#   tjommie-rs/Cargo.toml                [workspace.package] version, which every crate inherits
#   tjommie-rs/crates/*/Cargo.toml       the version in each path dependency on another workspace crate (a pin of 0.5.0 would not accept 0.6.0)
#   tjommie-rs/Cargo.lock                the entries of the workspace's own crates (and no other crate's)
#
# It leaves alone what is not Tjommie's own version: other crates' and packages' versions in the locks, packages/transport/native, and the
# CHANGELOG (renamed at the release step by hand). Every file is read and every replacement counted BEFORE anything is written, so a file that
# has changed shape stops the whole run with nothing changed; line endings and the rest of each file are kept exactly as they are. Running it
# again with the same version changes nothing. It never goes down: a version below the current one is refused.
#
# -Root is for the tests (a copy of these files); the default is the repository this script is in.
param(
    [Parameter(Mandatory)][string]$Version,
    [string]$Root = (Split-Path -Parent $PSScriptRoot)
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw "The version must be x.y.z, not '$Version'." }
$Root = (Resolve-Path -LiteralPath $Root).ProviderPath.TrimEnd('\')
$utf8 = [Text.UTF8Encoding]::new($false)

# Every file, as text, by its path relative to the root. Nothing is written until all of them have been changed in memory.
$texts = [ordered]@{}
function Read-Text([string]$Relative) {
    $path = Join-Path $Root $Relative
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "$Relative is missing (looked in $Root)." }
    if (-not $texts.Contains($Relative)) {
        # A byte order mark would be dropped by the write below, so a file that has one is refused rather than quietly changed.
        $head = [IO.File]::ReadAllBytes($path) | Select-Object -First 3
        if (@($head).Count -eq 3 -and $head[0] -eq 0xEF -and $head[1] -eq 0xBB -and $head[2] -eq 0xBF) { throw "$Relative starts with a byte order mark; this script writes UTF-8 without one." }
        $texts[$Relative] = [IO.File]::ReadAllText($path, $utf8)
    }
    return $texts[$Relative]
}

# Replaces group 2 of every match of $Pattern (group 1 before it and group 3 after it are kept) with $With, and insists on exactly $Expected
# matches ($First: only the first one counts, for a top-level key that a file may also repeat deeper down).
function Set-Group([string]$Relative, [string]$Pattern, [string]$With, [int]$Expected, [switch]$First) {
    $text = Read-Text $Relative
    $regex = [regex]::new($Pattern, [Text.RegularExpressions.RegexOptions]::Multiline)
    $found = $regex.Matches($text).Count
    $changing = if ($First) { [Math]::Min($found, 1) } else { $found }
    if ($changing -ne $Expected) { throw "$Relative has $changing place(s) matching $Pattern, expected $Expected. The file has changed shape: nothing was written." }
    $evaluator = [Text.RegularExpressions.MatchEvaluator]{ param($m) $m.Groups[1].Value + $With + $m.Groups[3].Value }
    $texts[$Relative] = if ($First) { $regex.Replace($text, $evaluator, 1) } else { $regex.Replace($text, $evaluator) }
}

# ---- the current version, and the refusal to go down
$rootPackage = Read-Text 'package.json'
if ($rootPackage -notmatch '(?m)^  "version": "(\d+\.\d+\.\d+)"') { throw 'package.json has no top-level version.' }
$current = $Matches[1]
if ([version]$Version -lt [version]$current) { throw "$Version is lower than the current version, $current; a version only goes up." }

# ---- npm: the root and each workspace package
$packages = @(Get-ChildItem -LiteralPath (Join-Path $Root 'packages') -Directory | Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'package.json') } | Sort-Object Name)
if ($packages.Count -eq 0) { throw "No packages/*/package.json under $Root." }
Set-Group 'package.json' '(?m)^(  "version": ")([^"]+)(")' $Version 1 -First
foreach ($package in $packages) { Set-Group "packages/$($package.Name)/package.json" '(?m)^(  "version": ")([^"]+)(")' $Version 1 -First }
# package-lock.json: the lockfile's own top, the root workspace entry, and one entry per workspace package (named @tjommie/<x>).
Set-Group 'package-lock.json' '\A(\{\r?\n  "name": "[^"]+",\r?\n  "version": ")([^"]+)(")' $Version 1
Set-Group 'package-lock.json' '(?m)("packages": \{\r?\n    "": \{\r?\n      "name": "[^"]+",\r?\n      "version": ")([^"]+)(")' $Version 1
Set-Group 'package-lock.json' '("name": "@tjommie/[a-z-]+",\r?\n      "version": ")([^"]+)(")' $Version $packages.Count

# ---- Android: versionName, versionCode, and the table the code comes from
$native = Read-Text 'packages/mobile/scripts/apply-native.mjs'
$tablePattern = '(?m)^(const versionCodes = new Map\(\[)(.*)(\]\);)'
if ($native -notmatch $tablePattern) { throw 'apply-native.mjs has no versionCodes table.' }
$tableBody = $Matches[2]
$entries = @([regex]::Matches($tableBody, '\["(\d+\.\d+\.\d+)", (\d+)\]') | ForEach-Object { [pscustomobject]@{ Version = $_.Groups[1].Value; Code = [int]$_.Groups[2].Value } })
if ($entries.Count -eq 0) { throw 'The versionCodes table in apply-native.mjs has no entries.' }
$known = @($entries | Where-Object { $_.Version -eq $Version })
if ($known.Count -gt 0) { $code = $known[0].Code } else { $code = (($entries | Measure-Object Code -Maximum).Maximum) + 1 }
if ($known.Count -eq 0) { Set-Group 'packages/mobile/scripts/apply-native.mjs' $tablePattern "$tableBody, [`"$Version`", $code]" 1 }
Set-Group 'packages/mobile/android/app/build.gradle' '(?m)^(\s+versionCode )(\d+)()' "$code" 1
Set-Group 'packages/mobile/android/app/build.gradle' '(?m)^(\s+versionName ")([^"]+)(")' $Version 1

# ---- Rust: the workspace version, the pins on workspace crates, and the lock file's entries for them
Set-Group 'tjommie-rs/Cargo.toml' '(?m)^(version = ")([^"]+)(")' $Version 1 -First
$crates = @(Get-ChildItem -LiteralPath (Join-Path $Root 'tjommie-rs\crates') -Directory | Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'Cargo.toml') } | Sort-Object Name)
$names = foreach ($crate in $crates) {
    $manifest = Read-Text "tjommie-rs/crates/$($crate.Name)/Cargo.toml"
    if ($manifest -notmatch '(?m)^name = "([^"]+)"') { throw "tjommie-rs/crates/$($crate.Name)/Cargo.toml has no package name." }
    $Matches[1]
}
$alternation = ($names | ForEach-Object { [regex]::Escape($_) }) -join '|'
foreach ($crate in $crates) {
    $relative = "tjommie-rs/crates/$($crate.Name)/Cargo.toml"
    $text = Read-Text $relative
    # { version = "x", path = "../y" } and { path = "../y", version = "x" }, for a dependency that is one of the workspace's own crates.
    foreach ($pattern in "(\b(?:$alternation)\s*=\s*\{\s*version\s*=\s*`")([^`"]+)(`",\s*path\s*=)", "(\b(?:$alternation)\s*=\s*\{\s*path\s*=\s*`"[^`"]+`",\s*version\s*=\s*`")([^`"]+)(`")") {
        $count = [regex]::Matches($text, $pattern).Count
        if ($count -gt 0) { Set-Group $relative $pattern $Version $count }
        $text = $texts[$relative]
    }
}
# Cargo.lock: only a package with one of those names and no `source` line (the workspace's own crates are path packages; a crate of the
# same name from a registry has a source and is somebody else's).
Set-Group 'tjommie-rs/Cargo.lock' "(?m)(^name = `"(?:$alternation)`"\r?\nversion = `")([^`"]+)(`")(?=\r?\n(?!source ))" $Version @($names).Count

# ---- write everything, byte for byte except the versions
$changed = @()
foreach ($relative in $texts.Keys) {
    $path = Join-Path $Root $relative
    if ([IO.File]::ReadAllText($path, $utf8) -ne $texts[$relative]) {
        [IO.File]::WriteAllText($path, $texts[$relative], $utf8)
        $changed += $relative
    }
}
if ($changed.Count -eq 0) { Write-Output "Already at $Version (Android versionCode $code): nothing to change." }
else { Write-Output "Now at $Version (was $current), Android versionCode $code. Changed $($changed.Count) file(s):`n  $($changed -join "`n  ")" }
