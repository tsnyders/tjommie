import { spawnSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, it } from "vitest";
import { tempRoot } from "./testing.ts";

it.skipIf(process.platform !== "win32")("compiles the native job code and validates only in-memory descriptors and synthetic receipts", () => {
  const helper = readFileSync(fileURLToPath(new URL("./windows-worker.ps1", import.meta.url)), "utf8").replace(/\r\n/g, "\n");
  const start = helper.indexOf("  Add-Type -TypeDefinition @'\nusing System;"), end = helper.indexOf("\n'@", start) + 4;
  const functions = helper.slice(helper.indexOf("function Add-TrustedWorkerJobs("), helper.indexOf("\ntry {", helper.indexOf("function Add-TrustedWorkerJobs(")));
  expect(start).toBeGreaterThan(0); expect(end).toBeGreaterThan(start); expect(functions).toContain("Owned job identity mismatch");
  const script = "$ErrorActionPreference='Stop'\n" + helper.slice(start, end) + "\n" + functions + String.raw`
$owner='S-1-5-21-1-2-3-1000';$provider='S-1-5-21-1-2-3-1001';$review='S-1-5-21-1-2-3-1002';$passed=0
function Descriptor([string]$sddl){$raw=New-Object Security.AccessControl.RawSecurityDescriptor($sddl);$bytes=New-Object byte[] $raw.BinaryLength;$raw.GetBinaryForm($bytes,0);return ,$bytes}
$safe='O:'+$owner+'D:P(A;;GA;;;'+$owner+')(A;;GA;;;SY)(A;;GA;;;BA)'
[TjWorkerSecurity]::AssertJobDescriptor((Descriptor $safe),$owner);$passed++
foreach($unsafe in @(($safe+'(A;;GR;;;'+$provider+')'),($safe+'(A;;GA;;;'+$review+')'),('O:'+$provider+'D:P(A;;GA;;;'+$owner+')'),('O:'+$owner+'D:NO_ACCESS_CONTROL'))){
 $refused=$false;try{[TjWorkerSecurity]::AssertJobDescriptor((Descriptor $unsafe),$owner)}catch{$refused=$true}
 if(-not $refused){throw 'Unsafe in-memory job descriptor accepted'};$passed++
}
$config=[pscustomobject]@{purpose='provider';ownerSid=$owner;workerSid=$provider;peerSid=$review}
$good=[pscustomobject]@{version=3;purpose='provider';ownerSid=$owner;workerSid=$provider;wrapperPid=10;workerPid=42;jobName='Global\TjommieWorkerJob-provider-12345678-1234-1234-1234-123456789abc';jobBound=$true;elevated=$false;privileged=$false}
function FakeJobs {
 $jobs=[pscustomobject]@{Calls=0;Fail=$false}
 $jobs|Add-Member ScriptMethod Add {param($name,$root) $this.Calls++;if($this.Fail){throw 'Synthetic missing/stale/unknown job'};if($root -le 0){throw 'Invalid root'}}
 return $jobs
}
$jobs=FakeJobs;Add-TrustedWorkerJobs $jobs @($good) $config;if($jobs.Calls -ne 1){throw 'Valid receipt not queried'};$passed++
$peer=$good.psobject.Copy();$peer.purpose='review';$peer.workerSid=$review;$peer.jobName='Global\TjommieWorkerJob-review-12345678-1234-1234-1234-123456789abc'
$jobs=FakeJobs;Add-TrustedWorkerJobs $jobs @($peer) $config;if($jobs.Calls -ne 1){throw 'Peer receipt not queried'};$passed++
foreach($change in @(@{version=2},@{purpose='other'},@{ownerSid=$provider},@{workerSid=$review},@{wrapperPid=0},@{workerPid=0},@{jobBound=$false},@{elevated=$true},@{privileged=$true},@{jobName=$peer.jobName},@{jobName='unknown'})){
 $bad=$good.psobject.Copy();foreach($key in $change.Keys){$bad.$key=$change[$key]};$jobs=FakeJobs
 $refused=$false;try{Add-TrustedWorkerJobs $jobs @($bad) $config}catch{$refused=$true}
 if(-not $refused -or $jobs.Calls -ne 0){throw 'Mismatched receipt reached native job query'};$passed++
}
$jobs=FakeJobs;$refused=$false;try{Add-TrustedWorkerJobs $jobs @($good,$good) $config}catch{$refused=$true}
if(-not $refused -or $jobs.Calls -ne 1){throw 'Duplicate job identity accepted'};$passed++
$jobs=FakeJobs;$jobs.Fail=$true;$refused=$false;try{Add-TrustedWorkerJobs $jobs @($good) $config}catch{$refused=$true}
if(-not $refused){throw 'Unknown native job accepted'};$passed++
[Console]::Out.Write($passed)
`;
  const path = join(tempRoot(), "compile-and-synthetic-job-check.ps1"); writeFileSync(path, script);
  // This compiles P/Invoke declarations; it never calls job/mutex creation,
  // process/logon/native ACL APIs or a real credential store.
  const result = spawnSync(join(process.env.SystemRoot ?? "C:\\Windows", "System32", "WindowsPowerShell", "v1.0", "powershell.exe"), ["-NoLogo", "-NoProfile", "-NonInteractive", "-File", path], { encoding: "utf8", windowsHide: true, timeout: 25_000, maxBuffer: 4096 });
  expect(result.error).toBeUndefined(); expect(result.stderr).toBe(""); expect(result.status).toBe(0); expect(result.stdout).toBe("20");
}, 30_000);
