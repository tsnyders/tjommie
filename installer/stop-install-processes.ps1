# Closes whatever runs from one install folder, for the Tjommie installer and uninstaller (T8.2). They run it before they touch a
# file: a running Tjommie.exe holds its own files open, and a Tjommie of another version running on the same data root as
# the one about to start is what the hand-over from R1 must never allow.
#
# A process is stopped only because its executable is inside -Folder (and its children, however deep), never because of its
# name: a real PC may have another Tjommie.exe running from somewhere else, and that one is left alone. A child of the app that
# lives outside the folder and owns a visible window (a browser the app opened) is left running too. -ExceptProcessId
# names the caller itself; an uninstaller that runs from inside the folder (R1's installer starts it with `_?=<folder>`)
# must not stop itself.
#
# Exit codes: 0 nothing is left running from the folder; 3 something still is, after 15 seconds; anything else (1) the
# folder was refused as too broad, or the script could not run. The installer turns a non-zero exit into "Tjommie is
# running and could not be closed" and installs nothing.
param(
    [Parameter(Mandatory)][string]$Folder,
    [int[]]$ExceptProcessId = @()
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'scoped-processes.ps1')

try {
    # Not trimmed here: "C:\" must reach the check as it is (trimmed it is "C:", the current folder of drive C).
    Assert-NarrowFolder $Folder
    if (-not (Test-Path -LiteralPath $Folder -PathType Container)) { exit 0 }

    $deadline = (Get-Date).AddSeconds(15)
    while ($true) {
        $remaining = Select-ProcessesInFolder $Folder $ExceptProcessId
        if ($remaining.Count -eq 0) {
            # What the app started but is not part of it, and has a window of its own (a browser it opened), is left running.
            $left = @($script:LastSkippedWindowed.Values | ForEach-Object { $_.Name + ' (' + $_.ProcessId + ')' })
            if ($left.Count -gt 0) { Write-Output ('Left running, outside the folder, with a window of its own: ' + ($left -join ', ')) }
            exit 0
        }
        if ((Get-Date) -ge $deadline) {
            Write-Output ('Still running from ' + $Folder + ': ' + (($remaining.Values | ForEach-Object { $_.Name + ' (' + $_.ProcessId + ')' }) -join ', '))
            exit 3
        }
        Stop-ProcessesInFolder $Folder $ExceptProcessId | Out-Null
        Start-Sleep -Milliseconds 300
    }
} catch {
    Write-Output $_.Exception.Message
    exit 1
}
