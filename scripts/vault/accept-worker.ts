import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import type { ChildProcessWithoutNullStreams } from "node:child_process";
import { z } from "zod";
import { WindowsWorkerBoundary, WorkerLaunchTerminationError, workerConfigurationHash, workerProcessRetired } from "../../packages/daemon/src/windows-worker.ts";
import { installWindowsWorker, resolveCli, runCli, withWindowsWorker, workerCliExclusive } from "../../packages/daemon/src/cli.ts";
import { windowsCredentialTransport } from "../../packages/daemon/src/account-credentials.ts";

const dataRoot = process.argv[2];
if (!dataRoot) throw new Error("Usage: accept-worker <Tjommie data root> [windows-worker.ps1 path]");
const packagedHelper = fileURLToPath(new URL("../../daemon/windows-worker.ps1", import.meta.url));
const sourceHelper = fileURLToPath(new URL("../../packages/daemon/src/windows-worker.ps1", import.meta.url));
const helperPath = process.argv[3] ?? (existsSync(packagedHelper) ? packagedHelper : sourceHelper);
let provider: WindowsWorkerBoundary, review: WindowsWorkerBoundary;
provider = new WindowsWorkerBoundary({ configurationPath: join(resolve(dataRoot), "worker.json"), expectedPurpose: "provider", helperPath, exclusive: () => workerCliExclusive(provider) });
review = new WindowsWorkerBoundary({ configurationPath: join(resolve(dataRoot), "review-worker.json"), expectedPurpose: "review", helperPath, exclusive: () => workerCliExclusive(review) });
const uninstall = installWindowsWorker(provider, review), configuration = provider.configuration(), reviewing = review.configuration();
const id = randomUUID(), target = configuration.credentialTarget.replace(/isolated-worker$/, "acceptance/" + id);
const marker = join(configuration.ownerDataRoot, "acceptance-" + id + ".txt"), registry = "Software\\Tjommie\\Acceptance\\" + id;
const roots = new Map([[provider, join(configuration.workerRoot, "acceptance-" + id)], [review, join(reviewing.workerRoot, "acceptance-" + id)]]);
const paths = [join(configuration.ownerDataRoot, "worker-acceptance.json"), join(configuration.ownerDataRoot, "review-worker-acceptance.json")];
let terminationUnknown = false;
const Denials = z.object({ ownerFileDenied: z.literal(true), ownerCredentialDenied: z.literal(true), ownerProcessDenied: z.literal(true), ownerDesktopDenied: z.literal(true), workerWriteAllowed: z.literal(true), peerFileDenied: z.literal(true), peerWriteDenied: z.literal(true), peerRegistryDenied: z.literal(true), peerProcessDenied: z.literal(true) }).strict();
const powershellProbe = String.raw`
$ErrorActionPreference='Stop'
[Console]::InputEncoding=New-Object Text.UTF8Encoding($false)
[Console]::OutputEncoding=New-Object Text.UTF8Encoding($false)
$p=[Console]::In.ReadToEnd()|ConvertFrom-Json
Add-Type -TypeDefinition @'
using System;using System.IO;using System.Security;using System.Runtime.InteropServices;using Microsoft.Win32;
public static class TjWorkerProbe {
 [DllImport("advapi32.dll",EntryPoint="CredReadW",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool Read(string target,uint type,uint flags,out IntPtr value);
 [DllImport("advapi32.dll")] static extern void CredFree(IntPtr value);
 [DllImport("kernel32.dll",SetLastError=true)] static extern IntPtr OpenProcess(uint access,bool inherit,uint pid);
 [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr value);
 [DllImport("user32.dll",SetLastError=true)] static extern IntPtr OpenInputDesktop(uint flags,bool inherit,uint access);
 [DllImport("user32.dll")] static extern bool CloseDesktop(IntPtr value);
 public static bool CredentialDenied(string target) { IntPtr value; if(!Read(target,1,0,out value)) return Marshal.GetLastWin32Error()==1168; CredFree(value); return false; }
 public static bool ProcessDenied(uint pid) { var value=OpenProcess(0x0038,false,pid); if(value==IntPtr.Zero) return Marshal.GetLastWin32Error()==5; CloseHandle(value); return false; }
 public static bool DesktopDenied() { var value=OpenInputDesktop(0,false,0x0108); if(value==IntPtr.Zero) return Marshal.GetLastWin32Error()==5; CloseDesktop(value); return false; }
 public static bool FileDenied(string path,bool write) { try { using(var file=File.Open(path,FileMode.Open,write?FileAccess.ReadWrite:FileAccess.Read,FileShare.ReadWrite)){} return false; } catch(UnauthorizedAccessException){return true;} }
 public static bool RegistryDenied(string path) { try { using(var key=Registry.Users.OpenSubKey(path,true)){return false;} } catch(UnauthorizedAccessException){return true;} catch(SecurityException){return true;} }
}
'@
$writeAllowed=$false
try { [IO.File]::WriteAllText($p.write,'synthetic');$writeAllowed=$true } catch {}
@{ownerFileDenied=[TjWorkerProbe]::FileDenied($p.marker,$false);ownerCredentialDenied=[TjWorkerProbe]::CredentialDenied($p.target);ownerProcessDenied=[TjWorkerProbe]::ProcessDenied($p.ownerPid);ownerDesktopDenied=[TjWorkerProbe]::DesktopDenied();workerWriteAllowed=$writeAllowed;peerFileDenied=[TjWorkerProbe]::FileDenied($p.peerMarker,$false);peerWriteDenied=[TjWorkerProbe]::FileDenied($p.peerMarker,$true);peerRegistryDenied=[TjWorkerProbe]::RegistryDenied($p.peerRegistry);peerProcessDenied=[TjWorkerProbe]::ProcessDenied($p.peerPid)}|ConvertTo-Json -Compress
`;
const powershellHolder = String.raw`
$ErrorActionPreference='Stop'
[Console]::InputEncoding=New-Object Text.UTF8Encoding($false)
[Console]::OutputEncoding=New-Object Text.UTF8Encoding($false)
$p=[Console]::In.ReadLine()|ConvertFrom-Json
$key='HKCU:\'+$p.registry
try { [void](New-Item -Path $key -Force);[void](New-ItemProperty -LiteralPath $key -Name 'Synthetic' -Value 'present' -PropertyType String -Force);[IO.File]::WriteAllText($p.marker,'synthetic');[Console]::Out.WriteLine('ready');[void][Console]::In.ReadToEnd() }
finally { Remove-Item -LiteralPath $key -Recurse -Force -ErrorAction SilentlyContinue;Remove-Item -LiteralPath $p.marker -Force -ErrorAction SilentlyContinue }
`;
const nativeNode = (script: string, hold: boolean): string => "const{spawn,spawnSync}=require('node:child_process');const{join}=require('node:path');const file=join(process.env.SystemRoot,'System32','WindowsPowerShell','v1.0','powershell.exe');const args=['-NoLogo','-NoProfile','-NonInteractive','-Command'," + JSON.stringify(script) + "];" + (hold
  ? "const p=spawn(file,args,{windowsHide:true,stdio:['pipe','pipe','pipe']});p.stdin.write(process.argv[1]+'\\n');process.stdin.pipe(p.stdin);p.stdout.pipe(process.stdout);p.stderr.pipe(process.stderr);p.once('error',()=>process.exit(1));p.once('close',code=>process.exit(code??1));"
  : "const p=spawnSync(file,args,{windowsHide:true,input:process.argv[1],encoding:'utf8',timeout:15000,maxBuffer:4096});if(p.status!==0)process.exit(1);process.stdout.write(p.stdout);");
async function retired(child: ChildProcessWithoutNullStreams): Promise<void> {
  const proof = workerProcessRetired(child); if (!proof) throw new WorkerLaunchTerminationError();
  let timer: NodeJS.Timeout | undefined;
  try { await Promise.race([proof, new Promise<never>((_done, reject) => { timer = setTimeout(() => { child.kill(); reject(new WorkerLaunchTerminationError()); }, 15_000); timer.unref(); })]); }
  finally { clearTimeout(timer); }
}
async function collect(child: ChildProcessWithoutNullStreams): Promise<string> {
  let text = "", overflow = false;
  const result = await new Promise<{ code: number | null; text: string }>((done, reject) => {
    const timer = setTimeout(() => { child.kill(); reject(new WorkerLaunchTerminationError()); }, 30_000); timer.unref();
    child.stdout.on("data", (bytes: Buffer) => { if (text.length + bytes.length > 4096) { overflow = true; child.kill(); } else text += bytes.toString("utf8"); });
    child.stderr.resume(); child.once("error", () => { clearTimeout(timer); reject(new WorkerLaunchTerminationError()); });
    child.once("close", (code) => { clearTimeout(timer); done({ code, text }); });
  });
  await retired(child); if (result.code !== 0 || overflow) throw new Error("The synthetic worker denial probe failed."); return result.text;
}
async function scope<T>(boundary: WindowsWorkerBoundary, action: () => Promise<T>): Promise<T> {
  return withWindowsWorker(boundary, () => boundary.purpose === "review" ? boundary.withReviewPolicy(action) : action());
}
async function holdPeer<T>(peer: WindowsWorkerBoundary, action: (peerPid: number, peerMarker: string) => Promise<T>): Promise<T> {
  return scope(peer, async () => {
    const config = peer.configuration(), scratch = roots.get(peer)!;
    const peerMarker = join(scratch, "peer-marker.txt");
    const child = await peer.spawn(join(config.runtimeRoot, "node.exe"), ["-e", nativeNode(powershellHolder, true), JSON.stringify({ marker: peerMarker, registry })], { cwd: scratch, env: {} });
    try {
      await new Promise<void>((done, reject) => {
        let output = ""; const timer = setTimeout(() => { child.kill(); reject(new WorkerLaunchTerminationError()); }, 15_000); timer.unref();
        const ready = (bytes: Buffer): void => { output += bytes.toString("utf8"); if (output.trim() === "ready") { clearTimeout(timer); child.stdout.removeListener("data", ready); done(); } else if (output.length > 100) { clearTimeout(timer); reject(new Error("The peer fixture did not initialize.")); } };
        child.stdout.on("data", ready); child.stderr.resume(); child.once("error", () => { clearTimeout(timer); reject(new WorkerLaunchTerminationError()); }); child.once("close", () => { clearTimeout(timer); reject(new Error("The peer fixture closed before ready.")); });
      });
      const receipt = peer.receipts().find((value) => value.wrapperPid === child.pid); if (!receipt) throw new WorkerLaunchTerminationError();
      return await action(receipt.workerPid, peerMarker);
    } finally { child.stdin.end(); await retired(child); }
  });
}
try {
  for (const path of paths) rmSync(path, { force: true });
  for (const path of roots.values()) mkdirSync(path);
  writeFileSync(marker, "synthetic"); await windowsCredentialTransport({ operation: "write", target, key: "synthetic-" + randomUUID() });
  const results = new Map<WindowsWorkerBoundary, z.infer<typeof Denials>>();
  for (const [active, peer] of [[provider, review], [review, provider]] as const) {
    await holdPeer(peer, async (peerPid, peerMarker) => scope(active, async () => {
      const config = active.configuration(), scratch = roots.get(active)!;
      const payload = { marker, target, ownerPid: process.pid, peerPid, peerMarker, peerRegistry: config.peerSid + "\\" + registry, write: join(scratch, "write.txt") };
      const child = await active.spawn(join(config.runtimeRoot, "node.exe"), ["-e", nativeNode(powershellProbe, false), JSON.stringify(payload)], { cwd: scratch, env: {} });
      child.stdin.end(); results.set(active, Denials.parse(JSON.parse(await collect(child))));
      const account = join(scratch, "accounts"); mkdirSync(account);
      for (const name of ["claude", "codex"] as const) {
        const command = resolveCli(name); if (!command) throw new Error("Install both provider CLI distributions in the protected runtime folder.");
        const checked = await runCli(command, ["--version"], { [name === "codex" ? "CODEX_HOME" : "CLAUDE_CONFIG_DIR"]: account }, 30_000);
        if (checked.code !== 0 || !/\d+\.\d+\.\d+/.test(checked.stdout)) throw new Error("A CLI could not start in its required identity.");
      }
    }));
  }
  for (const [index, boundary] of [provider, review].entries()) {
    const config = boundary.configuration(), peer = boundary === provider ? reviewing : configuration;
    const accepted = { version: 2, purpose: config.purpose, workerSid: config.workerSid, peerSid: config.peerSid, configurationHash: workerConfigurationHash(config), peerConfigurationHash: workerConfigurationHash(peer), ...results.get(boundary), codexStarted: true, claudeStarted: true, reviewPolicyVerified: true };
    writeFileSync(paths[index]! + ".pending", JSON.stringify(accepted)); renameSync(paths[index]! + ".pending", paths[index]!);
  }
  if (await provider.unavailableReason() || await review.unavailableReason()) throw new Error("The paired protected acceptance receipts were not accepted.");
  console.log("Acceptance passed for separate provider/review SIDs: owner and peer file, registry, process, credential and desktop denial checks; both CLI versions; pre/post review policy. No model turn or owner credential was used.");
} catch (error) {
  terminationUnknown = error instanceof WorkerLaunchTerminationError;
  for (const path of paths) { rmSync(path, { force: true }); rmSync(path + ".pending", { force: true }); } throw error;
} finally {
  await windowsCredentialTransport({ operation: "delete", target }).catch(() => {}); rmSync(marker, { force: true });
  if (!terminationUnknown) for (const [boundary, folder] of roots) { const canonical = resolve(folder), base = resolve(boundary.configuration().workerRoot); if (canonical.startsWith(base + "\\") && canonical.endsWith("acceptance-" + id)) rmSync(canonical, { recursive: true, force: true }); }
  provider.close(); review.close(); uninstall();
}
