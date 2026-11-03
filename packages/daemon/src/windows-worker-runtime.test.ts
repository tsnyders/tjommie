import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, it } from "vitest";

it.skipIf(process.platform !== "win32")("rejects unsafe nested runtime ACLs using the actual native-helper validator without changing OS ACLs", () => {
  const helper = readFileSync(fileURLToPath(new URL("./windows-worker.ps1", import.meta.url)), "utf8");
  const start = helper.indexOf("function Assert-TrustedRuntimeDescriptor("), end = helper.indexOf("\ntry {", start);
  expect(start).toBeGreaterThan(0); expect(end).toBeGreaterThan(start);
  const script = helper.slice(start, end) + String.raw`
$ErrorActionPreference='Stop'
$owner='S-1-5-21-1-2-3-1000';$provider='S-1-5-21-1-2-3-1001';$review='S-1-5-21-1-2-3-1002'
function Descriptor([bool]$file,[string]$who,[string]$rights,[string]$owned) {
 $acl=if($file){New-Object Security.AccessControl.FileSecurity}else{New-Object Security.AccessControl.DirectorySecurity}
 $acl.SetOwner((New-Object Security.Principal.SecurityIdentifier($owned)))
 $acl.SetAccessRuleProtection($true,$false)
 $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule((New-Object Security.Principal.SecurityIdentifier($owner)),'FullControl','Allow')))
 $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule((New-Object Security.Principal.SecurityIdentifier($who)),$rights,'Allow')))
 return $acl
}
$passed=0
foreach($file in @($false,$true)) {
 foreach($sid in @($provider,$review,'S-1-5-32-545')) {
  Assert-TrustedRuntimeDescriptor (Descriptor $file $sid 'ReadAndExecute,Synchronize' $owner) $owner;$passed++
  foreach($rights in @('Modify','Delete','DeleteSubdirectoriesAndFiles','ChangePermissions','TakeOwnership','WriteData','AppendData','WriteAttributes','WriteExtendedAttributes')) {
   $refused=$false;try{Assert-TrustedRuntimeDescriptor (Descriptor $file $sid $rights $owner) $owner}catch{$refused=$true}
   if(-not $refused){throw 'unsafe child ACL accepted'};$passed++
  }
 }
 $refused=$false;try{Assert-TrustedRuntimeDescriptor (Descriptor $file $provider 'ReadAndExecute' $provider) $owner}catch{$refused=$true}
 if(-not $refused){throw 'worker ownership accepted'};$passed++
}
$nullDacl=New-Object Security.AccessControl.RawSecurityDescriptor(('O:'+$owner+'D:NO_ACCESS_CONTROL'))
$bytes=New-Object byte[] $nullDacl.BinaryLength;$nullDacl.GetBinaryForm($bytes,0)
$acl=New-Object Security.AccessControl.FileSecurity;$acl.SetSecurityDescriptorBinaryForm($bytes)
$refused=$false;try{Assert-TrustedRuntimeDescriptor $acl $owner}catch{$refused=$true}
if(-not $refused){throw 'null DACL accepted'};$passed++
$items=@{
 runtime=[pscustomobject]@{FullName='runtime';PSIsContainer=$true;Attributes=[IO.FileAttributes]::Directory};
 nested=[pscustomobject]@{FullName='nested';PSIsContainer=$true;Attributes=[IO.FileAttributes]::Directory};
 executable=[pscustomobject]@{FullName='executable';PSIsContainer=$false;Attributes=[IO.FileAttributes]::Normal}
}
$descriptors=@{runtime=(Descriptor $false $provider 'ReadAndExecute' $owner);nested=(Descriptor $false $review 'ReadAndExecute' $owner);executable=(Descriptor $true $provider 'ReadAndExecute' $owner)}
function Get-Item([string]$LiteralPath,[switch]$Force){return $items[$LiteralPath]}
function Get-Acl([string]$LiteralPath){return $descriptors[$LiteralPath]}
function Get-ChildItem([string]$LiteralPath,[switch]$Force){if($LiteralPath -eq 'runtime'){return $items.nested}else{return $items.executable}}
Assert-TrustedRuntimeTree 'runtime' $owner;$passed++
foreach($name in @('nested','executable')){
 $before=$descriptors[$name];$descriptors[$name]=Descriptor ($name -eq 'executable') $provider 'Modify' $owner
 $refused=$false;try{Assert-TrustedRuntimeTree 'runtime' $owner}catch{$refused=$true}
 if(-not $refused){throw 'unsafe nested tree accepted'};$passed++;$descriptors[$name]=$before
}
$items.nested.Attributes=[IO.FileAttributes]::Directory -bor [IO.FileAttributes]::ReparsePoint
$refused=$false;try{Assert-TrustedRuntimeTree 'runtime' $owner}catch{$refused=$true}
if(-not $refused){throw 'runtime reparse target accepted'};$passed++
$candidate=[pscustomobject]@{Name='node.exe';ProcessId=42;CommandLine=$null}
if(Test-UnreceiptedProviderCandidate $candidate @(42)){throw 'native receipt was not honored before argv'};$passed++
$refused=$false;try{[void](Test-UnreceiptedProviderCandidate $candidate @())}catch{$refused=$true}
if(-not $refused){throw 'unknown redacted Node was accepted'};$passed++
$candidate.CommandLine='node C:\runtime\node_modules\@anthropic-ai\claude-code\cli.js'
if(-not (Test-UnreceiptedProviderCandidate $candidate @())){throw 'untracked provider skipped'};$passed++
$candidate.CommandLine='node fixed-synthetic-fixture.js'
if(Test-UnreceiptedProviderCandidate $candidate @()){throw 'unrelated identified Node misclassified'};$passed++
$candidate.Name='claude.exe';$candidate.CommandLine=$null
if(-not (Test-UnreceiptedProviderCandidate $candidate @())){throw 'untracked native provider skipped'};$passed++
if(Test-UnreceiptedProviderCandidate $candidate @(42)){throw 'native provider receipt was ignored'};$passed++
[Console]::Out.Write($passed)
`;
  const result = spawnSync(join(process.env.SystemRoot ?? "C:\\Windows", "System32", "WindowsPowerShell", "v1.0", "powershell.exe"), ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script], { encoding: "utf8", windowsHide: true, timeout: 10_000, maxBuffer: 4096 });
  expect(result.error).toBeUndefined(); expect(result.stderr).toBe(""); expect(result.status).toBe(0); expect(result.stdout).toBe("73");
}, 15_000);
it.skipIf(process.platform !== "win32")("fences cross-host review ownership, abandoned locks and unsafe mutex ACLs without creating OS objects", () => {
  const helper = readFileSync(fileURLToPath(new URL("./windows-worker.ps1", import.meta.url)), "utf8");
  const start = helper.indexOf("function Assert-TrustedReviewMutexDescriptor("), end = helper.indexOf("\ntry {", start);
  expect(start).toBeGreaterThan(0); expect(end).toBeGreaterThan(start);
  const script = helper.slice(start, end) + String.raw`
$ErrorActionPreference='Stop'
$owner='S-1-5-21-1-2-3-1000';$provider='S-1-5-21-1-2-3-1001'
if(-not ([Threading.Mutex].GetConstructors()|Where-Object{($_.GetParameters().Count -eq 4) -and ($_.GetParameters()[3].ParameterType -eq [Security.AccessControl.MutexSecurity])})){throw 'secure mutex constructor unavailable'}
function MutexDescriptor([string]$owned,[string]$extra) {
 $acl=New-Object Security.AccessControl.MutexSecurity
 $acl.SetOwner((New-Object Security.Principal.SecurityIdentifier($owned)));$acl.SetAccessRuleProtection($true,$false)
 $acl.AddAccessRule((New-Object Security.AccessControl.MutexAccessRule((New-Object Security.Principal.SecurityIdentifier($owner)),'FullControl','Allow')))
 if($extra){$acl.AddAccessRule((New-Object Security.AccessControl.MutexAccessRule((New-Object Security.Principal.SecurityIdentifier($extra)),'FullControl','Allow')))}
 return $acl
}
$passed=0
Assert-TrustedReviewMutexDescriptor (MutexDescriptor $owner '') $owner;$passed++
foreach($acl in @((MutexDescriptor $owner $provider),(MutexDescriptor $provider ''))) {
 $refused=$false;try{Assert-TrustedReviewMutexDescriptor $acl $owner}catch{$refused=$true}
 if(-not $refused){throw 'untrusted mutex accepted'};$passed++
}
function FakeMutex([bool]$available,[bool]$abandoned) {
 $mutex=[pscustomobject]@{Available=$available;Abandoned=$abandoned;Released=0}
 $mutex|Add-Member ScriptMethod WaitOne {param($timeout) if($this.Abandoned){throw(New-Object Threading.AbandonedMutexException)}return $this.Available}
 $mutex|Add-Member ScriptMethod ReleaseMutex {$this.Released++}
 return $mutex
}
$script:marked=$false;$script:writes=0
function Test-Path([string]$LiteralPath){return $script:marked}
function New-ReviewMarker([string]$path,$config){if($script:marked){throw 'existing marker'};$script:marked=$true;$script:writes++}
$config=[pscustomobject]@{workerSid=$provider}
$mutex=FakeMutex $true $false
Enter-TrustedReviewMutex $mutex 'synthetic-marker' $config
if($mutex.Released -ne 0 -or $script:marked){throw 'successful launch did not retain ownership'};$passed++
$mutex=FakeMutex $false $false
$refused=$false;try{Enter-TrustedReviewMutex $mutex 'synthetic-marker' $config}catch{$refused=$true}
if(-not $refused -or $mutex.Released -ne 0 -or $script:marked){throw 'busy mutex not fenced'};$passed++
$script:marked=$true;$mutex=FakeMutex $true $false
$refused=$false;try{Enter-TrustedReviewMutex $mutex 'synthetic-marker' $config}catch{$refused=$true}
if(-not $refused -or $mutex.Released -ne 1 -or -not $script:marked){throw 'old marker not retained'};$passed++
$script:marked=$false;$mutex=FakeMutex $true $true
$refused=$false;try{Enter-TrustedReviewMutex $mutex 'synthetic-marker' $config}catch{$refused=$true}
if(-not $refused -or $mutex.Released -ne 1 -or -not $script:marked -or $script:writes -ne 1){throw 'abandoned job not durably fenced'};$passed++
$mutex=FakeMutex $true $false
$refused=$false;try{Enter-TrustedReviewMutex $mutex 'synthetic-marker' $config}catch{$refused=$true}
if(-not $refused -or $mutex.Released -ne 1 -or -not $script:marked){throw 'later host reused unknown job'};$passed++
[Console]::Out.Write($passed)
`;
  const result = spawnSync(join(process.env.SystemRoot ?? "C:\\Windows", "System32", "WindowsPowerShell", "v1.0", "powershell.exe"), ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script], { encoding: "utf8", windowsHide: true, timeout: 10_000, maxBuffer: 4096 });
  expect(result.error).toBeUndefined(); expect(result.stderr).toBe(""); expect(result.status).toBe(0); expect(result.stdout).toBe("8");
}, 15_000);
