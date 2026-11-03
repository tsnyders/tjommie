import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { WindowsWorkerBoundary, WorkerLaunchTerminationError, workerConfigurationHash, workerProcessRetired } from "../../packages/daemon/src/windows-worker.ts";
import { installWindowsWorker, resolveCli, runCli, workerCliExclusive } from "../../packages/daemon/src/cli.ts";
import { windowsCredentialTransport } from "../../packages/daemon/src/account-credentials.ts";

const dataRoot = process.argv[2];
if (!dataRoot) throw new Error("Usage: accept-worker <Tjommie data root> [windows-worker.ps1 path]");
const configurationPath = join(resolve(dataRoot), "worker.json");
const packagedHelper = fileURLToPath(new URL("../../daemon/windows-worker.ps1", import.meta.url));
const sourceHelper = fileURLToPath(new URL("../../packages/daemon/src/windows-worker.ps1", import.meta.url));
const helperPath = process.argv[3] ?? (existsSync(packagedHelper) ? packagedHelper : sourceHelper);
let boundary: WindowsWorkerBoundary;
boundary = new WindowsWorkerBoundary({ configurationPath, helperPath, exclusive: () => workerCliExclusive(boundary) });
const uninstall = installWindowsWorker(boundary), config = boundary.configuration();
const id = randomUUID(), target = config.credentialTarget.replace(/isolated-worker$/, `acceptance/${id}`);
const marker = join(config.ownerDataRoot, `acceptance-${id}.txt`), scratch = join(config.workerRoot, `acceptance-${id}`);
const account = join(config.workerRoot, "accounts", `acceptance-${id}`), acceptancePath = join(config.ownerDataRoot, "worker-acceptance.json");
let terminationUnknown = false;
mkdirSync(scratch); const sentinel = `synthetic-vault-${randomUUID()}`;
const probe = String.raw`
$ErrorActionPreference='Stop'
$p=[Console]::In.ReadToEnd()|ConvertFrom-Json
Add-Type -TypeDefinition @'
using System;using System.Runtime.InteropServices;
public static class TjWorkerProbe {
 [DllImport("advapi32.dll",EntryPoint="CredReadW",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool Read(string target,uint type,uint flags,out IntPtr value);
 [DllImport("advapi32.dll")] static extern void CredFree(IntPtr value);
 [DllImport("kernel32.dll",SetLastError=true)] static extern IntPtr OpenProcess(uint access,bool inherit,uint pid);
 [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr value);
 [DllImport("user32.dll",SetLastError=true)] static extern IntPtr OpenInputDesktop(uint flags,bool inherit,uint access);
 [DllImport("user32.dll")] static extern bool CloseDesktop(IntPtr value);
 public static bool CredentialDenied(string target) { IntPtr value; if(!Read(target,1,0,out value)) return Marshal.GetLastWin32Error()==1168; CredFree(value); return false; }
 public static bool ProcessDenied(uint pid) { var value=OpenProcess(0x0038,false,pid); if(value==IntPtr.Zero) return true; CloseHandle(value); return false; }
 public static bool DesktopDenied() { var value=OpenInputDesktop(0,false,0x0108); if(value==IntPtr.Zero) return true; CloseDesktop(value); return false; }
}
'@
$fileDenied=$false
try { [void][IO.File]::ReadAllText($p.marker) } catch { $fileDenied=$true }
$writeAllowed=$false
try { [IO.File]::WriteAllText($p.write,'synthetic'); $writeAllowed=$true } catch {}
@{ownerFileDenied=$fileDenied;ownerCredentialDenied=[TjWorkerProbe]::CredentialDenied($p.target);ownerProcessDenied=[TjWorkerProbe]::ProcessDenied($p.ownerPid);ownerDesktopDenied=[TjWorkerProbe]::DesktopDenied();workerWriteAllowed=$writeAllowed}|ConvertTo-Json -Compress
`;
try {
  rmSync(acceptancePath, { force: true });
  writeFileSync(marker, sentinel); await windowsCredentialTransport({ operation: "write", target, key: sentinel });
  const probePath = join(scratch, "probe.ps1"); writeFileSync(probePath, probe);
  const node = join(config.runtimeRoot, "node.exe");
  const code = "const{spawnSync}=require('node:child_process');const{join}=require('node:path');const r=spawnSync(join(process.env.SystemRoot,'System32','WindowsPowerShell','v1.0','powershell.exe'),['-NoLogo','-NoProfile','-NonInteractive','-File',process.argv[1]],{input:process.argv[2],encoding:'utf8',windowsHide:true,timeout:15000,maxBuffer:4096});if(r.status!==0)process.exit(1);process.stdout.write(r.stdout);";
  const child = await boundary.spawn(node, ["-e", code, probePath, JSON.stringify({ marker, target, ownerPid: process.pid, write: join(scratch, "write.txt") })], { cwd: scratch, env: {} });
  child.stdin.end();
  const output = await new Promise<string>((done, reject) => {
    const timer = setTimeout(() => { child.kill(); reject(new WorkerLaunchTerminationError()); }, 30_000); timer.unref();
    let text = "", overflow = false; child.stdout.on("data", (bytes: Buffer) => { if (text.length + bytes.length > 4096) { overflow = true; child.kill(); } else text += bytes.toString("utf8"); });
    child.stderr.resume(); child.once("error", () => { clearTimeout(timer); reject(new WorkerLaunchTerminationError()); }); child.once("close", (status) => {
      clearTimeout(timer); const retired = workerProcessRetired(child);
      if (!retired) { reject(new WorkerLaunchTerminationError()); return; }
      void retired.then(() => status === 0 && !overflow ? done(text) : reject(new Error("The worker denial probe failed."))).catch(reject);
    });
  });
  const result: unknown = JSON.parse(output);
  if (!result || typeof result !== "object" || !["ownerFileDenied", "ownerCredentialDenied", "ownerProcessDenied", "ownerDesktopDenied", "workerWriteAllowed"].every((key) => Reflect.get(result, key) === true)) throw new Error("A native worker boundary check failed; Logins remains unavailable.");
  mkdirSync(account, { recursive: true });
  for (const provider of ["claude", "codex"] as const) {
    const command = resolveCli(provider); if (!command) throw new Error("Install both provider CLIs in the protected runtime folder before acceptance.");
    const checked = await runCli(command, ["--version"], { [provider === "codex" ? "CODEX_HOME" : "CLAUDE_CONFIG_DIR"]: account }, 30_000);
    if (checked.code !== 0 || !/\d+\.\d+\.\d+/.test(checked.stdout)) throw new Error("A provider could not start under the verified worker account.");
  }
  await boundary.withReviewPolicy(async () => {});
  const accepted = { version: 1, configurationHash: workerConfigurationHash(config), ...result, codexStarted: true, claudeStarted: true, reviewPolicyVerified: true };
  writeFileSync(acceptancePath + ".pending", JSON.stringify(accepted)); renameSync(acceptancePath + ".pending", acceptancePath);
  if (await boundary.unavailableReason()) throw new Error("The final protected worker receipt was not accepted.");
  console.log("Worker acceptance passed: owner file, credential, process and desktop access denied; worker writes and both provider version commands passed. No model turn or owner credential was used.");
} catch (error) {
  terminationUnknown = error instanceof WorkerLaunchTerminationError;
  rmSync(acceptancePath, { force: true }); rmSync(acceptancePath + ".pending", { force: true }); throw error;
} finally {
  await windowsCredentialTransport({ operation: "delete", target }).catch(() => {});
  rmSync(marker, { force: true });
  if (!terminationUnknown) for (const folder of [scratch, account]) {
    const canonical = resolve(folder); if (canonical.startsWith(resolve(config.workerRoot) + "\\") && canonical.endsWith(`acceptance-${id}`)) rmSync(canonical, { recursive: true, force: true });
  }
  boundary.close(); uninstall();
}
