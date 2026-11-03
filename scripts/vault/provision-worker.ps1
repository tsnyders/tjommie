[CmdletBinding()]
param([Parameter(Mandatory=$true)][string]$OwnerDataRoot)
$ErrorActionPreference = 'Stop'
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Run this reviewed setup in an elevated PowerShell window as the Tjommie owner.' }
$ownerRoot = [IO.Path]::GetFullPath($OwnerDataRoot).TrimEnd('\')
if ($ownerRoot -eq [IO.Path]::GetPathRoot($ownerRoot) -or $ownerRoot -eq $env:USERPROFILE -or $ownerRoot.StartsWith($env:SystemRoot,[StringComparison]::OrdinalIgnoreCase)) { throw 'Choose the dedicated Tjommie data folder.' }
if (Test-Path -LiteralPath (Join-Path $ownerRoot 'worker.json')) { throw 'This host already has worker configuration. Do not overwrite it.' }
$hash = [Security.Cryptography.SHA256]::Create()
try { $namespace = ([BitConverter]::ToString($hash.ComputeHash([Text.Encoding]::UTF8.GetBytes($ownerRoot.ToLowerInvariant())))).Replace('-','').ToLowerInvariant().Substring(0,24) } finally { $hash.Dispose() }
$workerName = 'TjWorker-' + $namespace.Substring(0,8)
if (Get-LocalUser -Name $workerName -ErrorAction SilentlyContinue) { throw 'The generated worker account already exists. Reconcile it before setup.' }
$base = Join-Path $env:ProgramData ('Tjommie\Worker-' + $namespace)
$workerRoot = Join-Path $base 'work'
$runtimeRoot = Join-Path $base 'runtime'
foreach ($path in @($ownerRoot,$base,$workerRoot,$runtimeRoot)) {
  [void](New-Item -ItemType Directory -Path $path -Force)
  if ((Get-Item -LiteralPath $path).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Setup folders must not be links.' }
}
. (Join-Path $PSScriptRoot 'worker-credential.ps1')
$credentialTarget = 'Tjommie/' + $namespace + '/isolated-worker'
$random = New-Object byte[] 48
$rng = [Security.Cryptography.RandomNumberGenerator]::Create()
try { $rng.GetBytes($random) } finally { $rng.Dispose() }
$password = 'Aa1!' + [Convert]::ToBase64String($random)
[Array]::Clear($random,0,$random.Length)
$secure = ConvertTo-SecureString $password -AsPlainText -Force
$created = $false
try {
  $account = New-LocalUser -Name $workerName -Password $secure -AccountNeverExpires -PasswordNeverExpires -UserMayNotChangePassword -Description 'Tjommie isolated provider worker'
  $created = $true
  $users = Get-LocalGroup -SID 'S-1-5-32-545'
  Add-LocalGroupMember -Group $users -Member $account
  $ownerSid = $identity.User.Value
  foreach ($path in @($ownerRoot,$base,$workerRoot,$runtimeRoot)) {
    $acl = New-Object Security.AccessControl.DirectorySecurity
    $acl.SetAccessRuleProtection($true,$false)
    $acl.SetOwner($identity.User)
    foreach ($sid in @($ownerSid,'S-1-5-18','S-1-5-32-544')) { $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule((New-Object Security.Principal.SecurityIdentifier($sid)),'FullControl','ContainerInherit,ObjectInherit','None','Allow'))) }
    if ($path -eq $workerRoot) { $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($account.SID,'Modify','ContainerInherit,ObjectInherit','None','Allow'))) }
    if ($path -eq $runtimeRoot) { $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($account.SID,'ReadAndExecute','ContainerInherit,ObjectInherit','None','Allow'))) }
    Set-Acl -LiteralPath $path -AclObject $acl
  }
  foreach ($child in @('tmp','accounts','AppData\Roaming','AppData\Local')) { [void](New-Item -ItemType Directory -Path (Join-Path $workerRoot $child) -Force) }
  $credential = @{ username=$workerName; password=$password } | ConvertTo-Json -Compress
  [TjWorkerCredential]::Put($credentialTarget,$credential)
  $config = [ordered]@{ version=1; ownerSid=$ownerSid; workerSid=$account.SID.Value; workerName=$workerName; workerRoot=$workerRoot; runtimeRoot=$runtimeRoot; ownerDataRoot=$ownerRoot; provisionedAt=[DateTime]::UtcNow.ToString('o'); credentialTarget=$credentialTarget }
  [IO.File]::WriteAllText((Join-Path $ownerRoot 'vault-protection-required'),'Worker isolation remains required until all Logins secrets are removed.',(New-Object Text.UTF8Encoding($false)))
  [IO.File]::WriteAllText((Join-Path $ownerRoot 'worker.json'),($config | ConvertTo-Json -Compress),(New-Object Text.UTF8Encoding($false)))
  Write-Output ('Created standard worker ' + $workerName + '. Install provider runtimes in ' + $runtimeRoot + ', restart Windows, then run the acceptance command. Sign in to fresh worker provider accounts; owner logins are not copied.')
} catch {
  if ($created) { try { Remove-LocalUser -Name $workerName } catch {} }
  try { [TjWorkerCredential]::Remove($credentialTarget) } catch {}
  throw 'Worker setup failed. No Logins feature was enabled. Review the generated folders before retrying.'
} finally {
  $password=$null; $credential=$null; $secure.Dispose()
}
