# A log for the installer's helper scripts (stop-install-processes.ps1, unlink-reparse-points.ps1). Dot-source this file first; it
# defines functions only and depends on nothing, so it loads even when the rest of a helper cannot.
#
# Why: on its first real run the stop helper failed with an exit code and no reason, and the installer could only say "could not run".
# Each helper now writes what it saw and decided to a file, and the installer appends the helper's exit code and output to
# tjommie-installer-logs\installer.log in the same folder, so a failed run leaves the exception, its stack and the decision list.
#
# The folder is %TEMP%\tjommie-installer-logs (TJOMMIE_HELPER_LOG_DIR overrides it, for tests). Nothing in this file ever throws: a
# helper that cannot write its log still does its work.

Set-StrictMode -Version Latest

$script:HelperLog = $null

function Get-HelperLogFolder {
    if ($env:TJOMMIE_HELPER_LOG_DIR) { return $env:TJOMMIE_HELPER_LOG_DIR }
    return (Join-Path ([IO.Path]::GetTempPath()) 'tjommie-installer-logs')
}

# Opens this run's log, named for the helper, the time and the process, and writes what it runs as.
function Start-HelperLog([string]$Name, [string]$Arguments) {
    try {
        $folder = Get-HelperLogFolder
        [void][IO.Directory]::CreateDirectory($folder)
        $script:HelperLog = Join-Path $folder ('{0}-{1:yyyyMMdd-HHmmss}-{2}.log' -f $Name, (Get-Date), $PID)
        Write-HelperLog ('{0} started: PowerShell {1} ({2}-bit process, {3}), user {4}, script in {5}' -f $Name, $PSVersionTable.PSVersion, $(if ([Environment]::Is64BitProcess) { 64 } else { 32 }), [Environment]::OSVersion.VersionString, [Environment]::UserName, $PSScriptRoot)
        Write-HelperLog "arguments: $Arguments"
    } catch { $script:HelperLog = $null }
}

function Write-HelperLog([string]$Message) {
    if (-not $script:HelperLog) { return }
    try { [IO.File]::AppendAllText($script:HelperLog, ('{0:HH:mm:ss.fff} {1}{2}' -f (Get-Date), $Message, [Environment]::NewLine)) } catch { }
}

# Logs an error with its type, message, position and stack, and returns the one line the helper prints for the installer to keep.
function Write-HelperFailure($ErrorRecord) {
    $type = ''
    $message = ''
    try {
        $type = $ErrorRecord.Exception.GetType().FullName
        $message = $ErrorRecord.Exception.Message
        Write-HelperLog "FAILED: ${type}: $message"
        if ($ErrorRecord.InvocationInfo) { Write-HelperLog ('at ' + ($ErrorRecord.InvocationInfo.PositionMessage -replace '\s+', ' ')) }
        if ($ErrorRecord.ScriptStackTrace) { Write-HelperLog ('stack: ' + ($ErrorRecord.ScriptStackTrace -replace '\r?\n', ' | ')) }
    } catch { }
    return ('HELPER FAILED: {0}: {1} (log: {2})' -f $type, $message, $script:HelperLog)
}
