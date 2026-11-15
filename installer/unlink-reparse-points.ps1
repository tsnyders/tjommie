# Removes the directory links (junctions and directory symbolic links) inside one install folder, as links, before the folder is
# deleted, for the Tjommie installer and uninstaller (T8.2). NSIS's `RMDir /r`, and R1's own uninstaller, descend into a junction
# and delete what it points at; a developer who has run `mklink /J resources\daemon <their build folder>` inside the install
# folder would lose that folder's contents. Deleting the link itself (Directory.Delete without recursion) never touches the target.
#
# It walks the folder without ever following a link, removes every directory link it finds, and leaves files and real folders alone.
# A file symbolic link is not removed: deleting a file link deletes the link, not its target. If the install folder is itself a link,
# nothing is removed and it exits 4: the folder that would be deleted is somewhere else.
#
# Exit codes: 0 no links left in the folder (it says how many it removed); 4 the folder is itself a link; 5 a link could not be
# removed; anything else (1) the folder was refused as too broad, or the script could not run. The caller treats any non-zero exit
# as "the folder cannot be deleted safely" and deletes nothing.
param([Parameter(Mandatory)][string]$Folder)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'scoped-processes.ps1')

function Test-Link([string]$Path) {
    return (([IO.File]::GetAttributes($Path) -band [IO.FileAttributes]::ReparsePoint) -ne 0)
}

# Removes every directory link under $Directory, never entering one. Returns the paths it removed.
function Remove-LinksUnder([string]$Directory) {
    $removed = @()
    foreach ($child in [IO.Directory]::GetDirectories($Directory)) {
        if (Test-Link $child) {
            [IO.Directory]::Delete($child)
            $removed += $child
        } else {
            $removed += Remove-LinksUnder $child
        }
    }
    return $removed
}

try {
    Assert-NarrowFolder $Folder
    if (-not [IO.Directory]::Exists($Folder)) { exit 0 }
    if (Test-Link $Folder) {
        Write-Output "$Folder is itself a link, so the folder that would be deleted is somewhere else. Nothing was removed."
        exit 4
    }
    $removed = @(Remove-LinksUnder $Folder)
    if ($removed.Count -gt 0) { Write-Output "Removed $($removed.Count) link(s) from ${Folder}: $($removed -join '; ')" }
    exit 0
} catch {
    Write-Output $_.Exception.Message
    # A link that is still there is the one thing that must not be passed over.
    if ($_.Exception -is [IO.IOException] -or $_.Exception -is [UnauthorizedAccessException]) { exit 5 }
    exit 1
}
