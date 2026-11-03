import { randomUUID } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { WindowsWorkerBoundary, workerConfigurationHash } from "../../packages/daemon/src/windows-worker.ts";
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
  writeFileSync(marker, sentinel); await windowsCredentialTransport({ operation: "write", target, key: sentinel });
  const probePath = join(scratch, "probe.ps1"); writeFileSync(probePath, probe);
  const node = join(config.runtimeRoot, "node.exe");
  const code = "const{spawnSync}=require('node:child_process');const r=spawnSync(process.env.SystemRoot+'\\\\System32\\\\WindowsPowerShell\\\\v1.0\\\\powershell.exe',['-NoLogo','-NoProfile','-NonInteractive','-File',process.argv[1]],{input:process.argv[2],encoding:'utf8',windowsHide:true});if(r.status!==0)process.exit(1);process.stdout.write(r.stdout);";
  const child = await boundary.spawn(node, ["-e", code, probePath, JSON.stringify({ marker, target, ownerPid: process.pid, write: join(scratch, "write.txt") })], { cwd: scratch, env: {} });
  child.stdin.end();
  const output = await new Promise<string>((done, reject) => {
    let text = ""; child.stdout.on("data", (bytes: Buffer) => { text += bytes.toString("utf8"); if (text.length > 4096) { child.kill(); reject(new Error("The worker probe exceeded its output limit.")); } });
    child.stderr.resume(); child.once("error", reject); child.once("close", (status) => status === 0 ? done(text) : reject(new Error("The worker denial probe failed.")));
  });
  const result: unknown = JSON.parse(output);
  if (!result || typeof result !== "object" || !["ownerFileDenied", "ownerCredentialDenied", "ownerProcessDenied", "ownerDesktopDenied", "workerWriteAllowed"].every((key) => Reflect.get(result, key) === true)) throw new Error("A native worker boundary check failed; Logins remains unavailable.");
  const account = join(config.workerRoot, "accounts", `acceptance-${id}`); mkdirSync(account);
  for (const provider of ["claude", "codex"] as const) {
    const command = resolveCli(provider); if (!command) throw new Error("Install both provider CLIs in the protected runtime folder before acceptance.");
    const checked = await runCli(command, ["--version"], { [provider === "codex" ? "CODEX_HOME" : "CLAUDE_CONFIG_DIR"]: account }, 30_000);
    if (checked.code !== 0 || !/\d+\.\d+\.\d+/.test(checked.stdout)) throw new Error("A provider could not start under the verified worker account.");
  }
  const accepted = { version: 1, configurationHash: workerConfigurationHash(config), ...result, codexStarted: true, claudeStarted: true };
  writeFileSync(join(config.ownerDataRoot, "worker-acceptance.json"), JSON.stringify(accepted));
  if (await boundary.unavailableReason()) throw new Error("The final protected worker receipt was not accepted.");
  console.log("Worker acceptance passed: owner file, credential, process and desktop access denied; worker writes and both provider version commands passed. No model turn or owner credential was used.");
} finally {
  await windowsCredentialTransport({ operation: "delete", target }).catch(() => {});
  rmSync(marker, { force: true });
  const canonical = resolve(scratch); if (canonical.startsWith(resolve(config.workerRoot) + "\\") && canonical.includes(`acceptance-${id}`)) rmSync(canonical, { recursive: true, force: true });
  boundary.close(); uninstall();
}
