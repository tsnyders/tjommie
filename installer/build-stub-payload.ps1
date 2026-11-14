# Builds the stand-in payload the installer acceptance runs use until the real desktop binary is ready (T8.2): for each identity in
# installer/identities.json, a folder holding its exe and resources\daemon\tjommie-daemon.exe, all one tiny program.
#
# The stub does what the acceptance tests need and nothing else:
#   - appends "<its file name> <process id> <arguments>" to the file named by the STUB_LOG environment variable, if set, so a test
#     can see that the installer started it, and with which arguments (--updated);
#   - with --spawn-daemon as its first argument, starts the daemon stub next to it (resources\daemon\tjommie-daemon.exe), as the
#     real app starts its daemon, so a test has a child running from the install folder;
#   - then sleeps until it is stopped. With --exit as its first argument it returns at once instead.
# It reads and writes nothing else: it never touches a data root.
#
# Needs Windows PowerShell (powershell.exe): Add-Type -OutputAssembly, which makes the .exe, is not in PowerShell 7. Compiling only
# writes files; nothing it writes is run.
param([Parameter(Mandatory)][string]$OutDir)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if ($PSVersionTable.PSEdition -ne 'Desktop') { throw 'Run this with Windows PowerShell (powershell.exe), not PowerShell 7: it compiles the stub with Add-Type -OutputAssembly.' }

$source = @'
using System;
using System.Diagnostics;
using System.IO;
using System.Threading;

public static class TjommieStub
{
    public static int Main(string[] args)
    {
        string self = Process.GetCurrentProcess().MainModule.FileName;
        string log = Environment.GetEnvironmentVariable("STUB_LOG");
        if (!string.IsNullOrEmpty(log))
        {
            File.AppendAllText(log, Path.GetFileName(self) + " " + Process.GetCurrentProcess().Id + " " + string.Join(" ", args) + Environment.NewLine);
        }
        if (args.Length > 0 && args[0] == "--exit") return 0;
        if (args.Length > 0 && args[0] == "--spawn-daemon")
        {
            Process.Start(Path.Combine(Path.GetDirectoryName(self), "resources", "daemon", "tjommie-daemon.exe"));
        }
        Thread.Sleep(Timeout.Infinite);
        return 0;
    }
}
'@

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$out = (Resolve-Path -LiteralPath $OutDir).ProviderPath
$stub = Join-Path $out 'stub.exe'
Add-Type -TypeDefinition $source -OutputAssembly $stub -OutputType ConsoleApplication
if (-not (Test-Path -LiteralPath $stub -PathType Leaf)) { throw "The stub was not compiled to $stub." }

$table = Get-Content -LiteralPath (Join-Path $PSScriptRoot 'identities.json') -Raw | ConvertFrom-Json
foreach ($name in $table.identities.PSObject.Properties.Name) {
    $row = $table.identities.$name
    $payload = Join-Path $out $name
    New-Item -ItemType Directory -Force -Path (Join-Path $payload 'resources\daemon') | Out-Null
    Copy-Item -LiteralPath $stub -Destination (Join-Path $payload $row.exe) -Force
    Copy-Item -LiteralPath $stub -Destination (Join-Path $payload 'resources\daemon\tjommie-daemon.exe') -Force
    Write-Output "payload for ${name}: $payload"
}
