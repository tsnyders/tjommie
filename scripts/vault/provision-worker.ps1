[CmdletBinding()]
param([Parameter(Mandatory=$true)][string]$OwnerDataRoot)
$ErrorActionPreference = 'Stop'
$identity = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($identity)
if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) { throw 'Run this reviewed setup in an elevated PowerShell window as the Tjommie owner.' }
$ownerRoot = [IO.Path]::GetFullPath($OwnerDataRoot).TrimEnd('\')
if ($ownerRoot -eq [IO.Path]::GetPathRoot($ownerRoot) -or $ownerRoot -eq $env:USERPROFILE -or $ownerRoot.StartsWith($env:SystemRoot,[StringComparison]::OrdinalIgnoreCase)) { throw 'Choose the dedicated Tjommie data folder.' }
foreach ($name in @('worker.json','review-worker.json')) { if (Test-Path -LiteralPath (Join-Path $ownerRoot $name)) { throw 'This host already has worker configuration. Do not overwrite it.' } }
$hash = [Security.Cryptography.SHA256]::Create()
try { $namespace = ([BitConverter]::ToString($hash.ComputeHash([Text.Encoding]::UTF8.GetBytes($ownerRoot.ToLowerInvariant())))).Replace('-','').ToLowerInvariant().Substring(0,24) } finally { $hash.Dispose() }
$base = Join-Path $env:ProgramData ('Tjommie\Worker-' + $namespace)
$runtimeRoot = Join-Path $base 'runtime'
$workers = @(
  @{ purpose='provider'; name=('TjWorker-'+$namespace.Substring(0,8)); root=(Join-Path $base 'work'); target=('Tjommie/'+$namespace+'/isolated-worker'); file='worker.json' },
  @{ purpose='review'; name=('TjReview-'+$namespace.Substring(0,8)); root=(Join-Path $base 'review'); target=('Tjommie/'+$namespace+'/isolated-review'); file='review-worker.json' }
)
foreach ($worker in $workers) { if (Get-LocalUser -Name $worker.name -ErrorAction SilentlyContinue) { throw 'A generated worker account already exists. Reconcile it before setup.' } }
foreach ($path in @($ownerRoot,$base,$runtimeRoot,$workers[0].root,$workers[1].root)) {
  [void](New-Item -ItemType Directory -Path $path -Force)
  if ((Get-Item -LiteralPath $path).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Setup folders must not be links.' }
}
. (Join-Path $PSScriptRoot 'worker-credential.ps1')
$created = New-Object 'Collections.Generic.List[string]'
try {
  $users = Get-LocalGroup -SID 'S-1-5-32-545'
  foreach ($worker in $workers) {
    $random = New-Object byte[] 48; $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
    try { $rng.GetBytes($random) } finally { $rng.Dispose() }
    $password = 'Aa1!' + [Convert]::ToBase64String($random); [Array]::Clear($random,0,$random.Length)
    $secure = ConvertTo-SecureString $password -AsPlainText -Force
    try {
      $account = New-LocalUser -Name $worker.name -Password $secure -AccountNeverExpires -PasswordNeverExpires -UserMayNotChangePassword -Description ('Tjommie isolated '+$worker.purpose+' worker')
      $created.Add($worker.name); Add-LocalGroupMember -Group $users -Member $account; $worker.sid = $account.SID
      [TjWorkerCredential]::Put($worker.target,(@{username=$worker.name;password=$password}|ConvertTo-Json -Compress))
    } finally { $password=$null; $secure.Dispose() }
  }
  foreach ($path in @($ownerRoot,$base,$runtimeRoot,$workers[0].root,$workers[1].root)) {
    $acl = New-Object Security.AccessControl.DirectorySecurity
    $acl.SetAccessRuleProtection($true,$false); $acl.SetOwner($identity.User)
    foreach ($sid in @($identity.User.Value,'S-1-5-18','S-1-5-32-544')) { $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule((New-Object Security.Principal.SecurityIdentifier($sid)),'FullControl','ContainerInherit,ObjectInherit','None','Allow'))) }
    foreach ($worker in $workers) {
      if ($path -eq $worker.root) { $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($worker.sid,'Modify','ContainerInherit,ObjectInherit','None','Allow'))) }
      if ($path -eq $runtimeRoot) { $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($worker.sid,'ReadAndExecute','ContainerInherit,ObjectInherit','None','Allow'))) }
    }
    Set-Acl -LiteralPath $path -AclObject $acl
  }
  $provisionedAt = [DateTime]::UtcNow.ToString('o')
  [IO.File]::WriteAllText((Join-Path $ownerRoot 'vault-protection-required'),'Separate provider and review identities remain required until all Logins secrets are removed.',(New-Object Text.UTF8Encoding($false)))
  for ($index=0;$index -lt $workers.Count;$index++) {
    $worker=$workers[$index]; $peer=$workers[1-$index]
    foreach ($child in @('tmp','accounts','AppData\Roaming','AppData\Local')) { [void](New-Item -ItemType Directory -Path (Join-Path $worker.root $child) -Force) }
    $config = [ordered]@{version=2;purpose=$worker.purpose;ownerSid=$identity.User.Value;workerSid=$worker.sid.Value;peerSid=$peer.sid.Value;workerName=$worker.name;workerRoot=$worker.root;peerRoot=$peer.root;runtimeRoot=$runtimeRoot;ownerDataRoot=$ownerRoot;provisionedAt=$provisionedAt;credentialTarget=$worker.target}
    [IO.File]::WriteAllText((Join-Path $ownerRoot $worker.file),($config|ConvertTo-Json -Compress),(New-Object Text.UTF8Encoding($false)))
  }
  Write-Output ('Created separate standard provider and review identities. Install runtimes in '+$runtimeRoot+', restart Windows and run acceptance. The desktop remains unelevated. No owner authentication was copied.')
} catch {
  foreach ($name in $created) { try { Remove-LocalUser -Name $name } catch {} }
  foreach ($worker in $workers) { try { [TjWorkerCredential]::Remove($worker.target) } catch {} }
  throw 'Worker setup failed. No Logins feature was enabled. Review the generated folders and protection marker before retrying.'
} finally { $password=$null }
