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
# Exit codes: 0 nothing is left running from the folder; 3 something still is, after 15 seconds; 1 anything else, which is printed as
# "HELPER FAILED: <type>: <message> (log: <file>)" and written, with its stack, to the log (helper-log.ps1), and that includes the
# library failing to load. The installer turns exit 3 into "Tjommie is running and could not be closed" and anything else into "the helper
# could not run", and either way installs, deletes and changes nothing. It fails closed: an unexpected error is never read as "nothing
# is running".
param(
    [Parameter(Mandatory)][string]$Folder,
    [int[]]$ExceptProcessId = @()
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'helper-log.ps1')
Start-HelperLog 'stop-install-processes' ("-Folder '$Folder' -ExceptProcessId $($ExceptProcessId -join ',')")

try {
    # Loaded inside the try: a library that cannot load is an error to report, not a bare exit code.
    . (Join-Path $PSScriptRoot 'scoped-processes.ps1')
    # Not trimmed here: "C:\" must reach the check as it is (trimmed it is "C:", the current folder of drive C).
    Assert-NarrowFolder $Folder
    if (-not (Test-Path -LiteralPath $Folder -PathType Container)) {
        Write-HelperLog 'the folder is not there, so nothing runs from it'
        exit 0
    }

    $deadline = (Get-Date).AddSeconds(15)
    $round = 0
    while ($true) {
        $round++
        $remaining = Select-ProcessesInFolder $Folder $ExceptProcessId
        Write-HelperLog "round ${round}: $($remaining.Count) process(es) run from the folder or were started by one"
        foreach ($id in $remaining.Keys) { Write-HelperLog ("select {0} {1} {2} ({3})" -f $id, $remaining[$id].Name, $remaining[$id].ExecutablePath, $script:LastReasons[$id]) }
        foreach ($skipped in $script:LastSkippedWindowed.Values) { Write-HelperLog ("leave {0} {1} {2} (outside the folder, owns a visible window)" -f $skipped.ProcessId, $skipped.Name, $skipped.ExecutablePath) }
        if ($remaining.Count -eq 0) {
            Write-HelperLog 'nothing runs from the folder any more'
            # What the app started but is not part of it, and has a window of its own (a browser it opened), is left running.
            $left = @($script:LastSkippedWindowed.Values | ForEach-Object { $_.Name + ' (' + $_.ProcessId + ')' })
            if ($left.Count -gt 0) { Write-Output ('Left running, outside the folder, with a window of its own: ' + ($left -join ', ')) }
            exit 0
        }
        if ((Get-Date) -ge $deadline) {
            $still = (($remaining.Values | ForEach-Object { $_.Name + ' (' + $_.ProcessId + ')' }) -join ', ')
            Write-HelperLog "gave up after 15 seconds: still running: $still"
            Write-Output ('Still running from ' + $Folder + ': ' + $still)
            exit 3
        }
        Stop-ProcessesInFolder $Folder $ExceptProcessId | Out-Null
        Write-HelperLog "round ${round}: asked them to stop"
        Start-Sleep -Milliseconds 300
    }
} catch {
    Write-Output (Write-HelperFailure $_)
    exit 1
}
