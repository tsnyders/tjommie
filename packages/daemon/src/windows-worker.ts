import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { AsyncLocalStorage } from "node:async_hooks";
import { createHash, randomUUID } from "node:crypto";
import { existsSync, readFileSync, realpathSync, rmSync, mkdirSync } from "node:fs";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { z } from "zod";
import { windowsCredentialTransport, type CredentialTransport } from "./account-credentials.ts";
import { WORKER_POLICY_OBSERVER } from "./worker-policy-observer.ts";

const Sid = z.string().regex(/^S-1-5-21-\d+-\d+-\d+-\d+$/);
export const WindowsWorkerConfiguration = z.object({
  version: z.literal(2), purpose: z.enum(["provider", "review"]), ownerSid: Sid, workerSid: Sid, peerSid: Sid, workerName: z.string().regex(/^Tj(?:Worker|Review)-[a-f0-9]{8}$/),
  workerRoot: z.string().min(1), peerRoot: z.string().min(1), runtimeRoot: z.string().min(1), ownerDataRoot: z.string().min(1),
  provisionedAt: z.string().datetime(),
  credentialTarget: z.string().regex(/^Tjommie\/[a-f0-9]{24}\/isolated-(?:worker|review)$/),
}).strict();
export type WindowsWorkerConfiguration = z.infer<typeof WindowsWorkerConfiguration>;
const Receipt = z.object({ version: z.literal(2), purpose: z.enum(["provider", "review"]), ownerSid: Sid, workerSid: Sid, wrapperPid: z.number().int().positive(), workerPid: z.number().int().positive(), elevated: z.literal(false), privileged: z.literal(false), jobBound: z.literal(true) }).strict();
const Acceptance = z.object({ version: z.literal(2), purpose: z.enum(["provider", "review"]), workerSid: Sid, peerSid: Sid, configurationHash: z.string().regex(/^[a-f0-9]{64}$/), peerConfigurationHash: z.string().regex(/^[a-f0-9]{64}$/), ownerFileDenied: z.literal(true), ownerCredentialDenied: z.literal(true), ownerProcessDenied: z.literal(true), ownerDesktopDenied: z.literal(true), workerWriteAllowed: z.literal(true), peerFileDenied: z.literal(true), peerWriteDenied: z.literal(true), peerRegistryDenied: z.literal(true), peerProcessDenied: z.literal(true), codexStarted: z.literal(true), claudeStarted: z.literal(true), reviewPolicyVerified: z.literal(true) }).strict();
const unavailable = "Logins needs the verified Windows worker account. Set up and verify provider isolation on the host before saving or filling logins.";
const failure = (): Error => new Error("The isolated Windows provider could not start. No owner credential was sent to it.");
export class WorkerLaunchTerminationError extends Error {
  constructor() { super("The Windows worker launch could not confirm process-tree termination. Keep its account and working folders reserved."); }
}
const exitProofs = new WeakMap<ChildProcessWithoutNullStreams, Promise<void>>();
export function workerProcessRetired(child: ChildProcessWithoutNullStreams): Promise<void> | undefined { return exitProofs.get(child); }
export type WorkerPolicyReceipt = Readonly<{ purpose: "review"; workerSid: string; providerSid: string; configurationSha256: string; observerSha256: string; policySha256: string }>;
export class WorkerPolicyBusyError extends Error { constructor() { super("Other worker jobs are running. Review can start after they finish."); } }
export function workerConfigurationHash(config: WindowsWorkerConfiguration): string { return createHash("sha256").update(JSON.stringify(WindowsWorkerConfiguration.parse(config))).digest("hex"); }
export function validateWorkerAcceptance(config: WindowsWorkerConfiguration, peer: WindowsWorkerConfiguration, raw: unknown): void {
  const accepted = Acceptance.parse(raw);
  if (accepted.purpose !== config.purpose || accepted.workerSid !== config.workerSid || accepted.peerSid !== config.peerSid || accepted.configurationHash !== workerConfigurationHash(config) || accepted.peerConfigurationHash !== workerConfigurationHash(peer)) throw failure();
}
function inside(root: string, path: string): boolean {
  const part = relative(realpathSync(root), realpathSync(path)); return part === "" || !part.startsWith("..") && !isAbsolute(part);
}

const directoryGrant = String.raw`
$ErrorActionPreference='Stop'
try {
$p=[Console]::In.ReadToEnd()|ConvertFrom-Json
Add-Type -TypeDefinition @'
using System;using System.ComponentModel;using System.IO;using System.Runtime.InteropServices;using System.Security.AccessControl;using System.Security.Principal;using System.Text;using Microsoft.Win32;using Microsoft.Win32.SafeHandles;
public static class TjWorkerDirectoryGrant {
 [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern SafeFileHandle CreateFileW(string name,uint access,uint share,IntPtr security,uint creation,uint flags,IntPtr template);
 [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern uint GetFinalPathNameByHandleW(SafeFileHandle file,StringBuilder path,uint count,uint flags);
 [DllImport("advapi32.dll",SetLastError=true)] static extern uint GetSecurityInfo(SafeFileHandle file,int type,uint info,out IntPtr owner,out IntPtr group,out IntPtr dacl,out IntPtr sacl,out IntPtr descriptor);
 [DllImport("advapi32.dll",SetLastError=true)] static extern uint GetSecurityDescriptorLength(IntPtr descriptor);
 [DllImport("advapi32.dll",SetLastError=true)] static extern bool GetSecurityDescriptorDacl(IntPtr descriptor,out bool present,out IntPtr dacl,out bool defaulted);
 [DllImport("advapi32.dll",SetLastError=true)] static extern uint SetSecurityInfo(SafeFileHandle file,int type,uint info,IntPtr owner,IntPtr group,IntPtr dacl,IntPtr sacl);
 [DllImport("kernel32.dll")] static extern IntPtr LocalFree(IntPtr memory);
 static SafeFileHandle Open(string path,uint access) { var file=CreateFileW(path,access,3,IntPtr.Zero,3,0x02000000,IntPtr.Zero);if(file.IsInvalid){file.Dispose();throw new Win32Exception(Marshal.GetLastWin32Error());}return file; }
 static string Final(SafeFileHandle file) { var text=new StringBuilder(32768);var count=GetFinalPathNameByHandleW(file,text,(uint)text.Capacity,0);if(count==0||count>=text.Capacity)throw new Win32Exception(Marshal.GetLastWin32Error());var path=text.ToString();if(path.StartsWith(@"\\?\UNC\",StringComparison.OrdinalIgnoreCase))path=@"\\"+path.Substring(8);else if(path.StartsWith(@"\\?\",StringComparison.Ordinal))path=path.Substring(4);return Path.GetFullPath(path).TrimEnd('\\'); }
 public static string Canonical(string path) { using(var file=Open(path,0))return Final(file); }
 static bool Overlap(string a,string b) { return a.Equals(b,StringComparison.OrdinalIgnoreCase)||a.StartsWith(b+"\\",StringComparison.OrdinalIgnoreCase)||b.StartsWith(a+"\\",StringComparison.OrdinalIgnoreCase); }
 public static void Grant(string path,string sid,string peerSid) {
  using(var file=Open(path,0x00060000)) {
   var canonical=Final(file);if(!canonical.Equals(Path.GetFullPath(path).TrimEnd('\\'),StringComparison.OrdinalIgnoreCase))throw new IOException("Directory target changed.");
   using(var key=Registry.LocalMachine.OpenSubKey(@"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList\"+peerSid,false)) {
    var profile=key==null?null:key.GetValue("ProfileImagePath") as string;
    if(!String.IsNullOrEmpty(profile)){profile=Path.GetFullPath(Environment.ExpandEnvironmentVariables(profile));if(Directory.Exists(profile))profile=Canonical(profile);if(Overlap(canonical,profile.TrimEnd('\\')))throw new UnauthorizedAccessException("Protected peer profile.");}
   }
   IntPtr owner,group,dacl,sacl,descriptor;var error=GetSecurityInfo(file,1,4,out owner,out group,out dacl,out sacl,out descriptor);if(error!=0)throw new Win32Exception((int)error);
   byte[] bytes;try{bytes=new byte[GetSecurityDescriptorLength(descriptor)];Marshal.Copy(descriptor,bytes,0,bytes.Length);}finally{LocalFree(descriptor);}
   var security=new DirectorySecurity();security.SetSecurityDescriptorBinaryForm(bytes,AccessControlSections.Access);security.AddAccessRule(new FileSystemAccessRule(new SecurityIdentifier(sid),FileSystemRights.Modify,InheritanceFlags.ContainerInherit|InheritanceFlags.ObjectInherit,PropagationFlags.None,AccessControlType.Allow));
   var updated=security.GetSecurityDescriptorBinaryForm();var pinned=GCHandle.Alloc(updated,GCHandleType.Pinned);
   try{bool present,defaulted;if(!GetSecurityDescriptorDacl(pinned.AddrOfPinnedObject(),out present,out dacl,out defaulted)||!present)throw new IOException("Missing directory DACL.");error=SetSecurityInfo(file,1,4,IntPtr.Zero,IntPtr.Zero,dacl,IntPtr.Zero);if(error!=0)throw new Win32Exception((int)error);}finally{pinned.Free();}
  }
 }
}
'@
[TjWorkerDirectoryGrant]::Grant($p.path,$p.sid,$p.peerSid)
} catch { exit 1 }
`;

function grantTarget(config: WindowsWorkerConfiguration, helper: string, path: string): string {
  const canonical = realpathSync(path);
  // The host's runtime/helper can never receive a worker write grant, including a child or ancestor.
  for (const trusted of [config.runtimeRoot, dirname(helper)]) if (inside(trusted, canonical) || inside(canonical, trusted)) throw failure();
  if (inside(config.peerRoot, canonical) || inside(canonical, config.peerRoot)) throw failure();
  if (config.purpose === "review" && !inside(config.workerRoot, canonical)) throw failure();
  if (inside(canonical, config.ownerDataRoot)) throw failure();
  if (inside(config.ownerDataRoot, canonical)) {
    // These are the only host-managed paths assigned to provider turns. Metadata, account
    // credentials, receipts, logs and shared attachments stay behind the owner ACL.
    const parts = relative(realpathSync(config.ownerDataRoot), canonical).split(/[\\/]/);
    const assigned = ["bots", "workspaces"].includes(parts[0]!.toLowerCase()) && parts.length >= 2 && /^[A-Za-z0-9_-]{1,100}$/.test(parts[1]!);
    if (!assigned) throw failure();
  }
  return canonical;
}

type Options = { configurationPath: string; expectedPurpose?: "provider" | "review"; helperPath?: string; transport?: CredentialTransport; exclusive: () => boolean };
const configName = (purpose: "provider" | "review"): string => purpose === "provider" ? "worker.json" : "review-worker.json";
const acceptanceName = (purpose: "provider" | "review"): string => purpose === "provider" ? "worker-acceptance.json" : "review-worker-acceptance.json";
export function validateWorkerPair(config: WindowsWorkerConfiguration, peer: WindowsWorkerConfiguration): void {
  const samePath = (a: string, b: string): boolean => resolve(a).toLowerCase() === resolve(b).toLowerCase();
  const overlap = (a: string, b: string): boolean => { const part = relative(resolve(a), resolve(b)); return !part || !part.startsWith("..") && !isAbsolute(part); };
  if (config.purpose === peer.purpose || new Set([config.ownerSid, config.workerSid, peer.workerSid]).size !== 3 || config.ownerSid !== peer.ownerSid
    || config.peerSid !== peer.workerSid || peer.peerSid !== config.workerSid || config.provisionedAt !== peer.provisionedAt
    || ![config.workerRoot, config.peerRoot, config.runtimeRoot, config.ownerDataRoot, peer.workerRoot].every(isAbsolute)
    || !samePath(config.peerRoot, peer.workerRoot) || !samePath(peer.peerRoot, config.workerRoot) || !samePath(config.runtimeRoot, peer.runtimeRoot) || !samePath(config.ownerDataRoot, peer.ownerDataRoot)
    || !config.workerName.startsWith(config.purpose === "review" ? "TjReview-" : "TjWorker-") || !config.credentialTarget.endsWith(config.purpose === "review" ? "/isolated-review" : "/isolated-worker")
    || [config.workerRoot, peer.workerRoot, config.runtimeRoot, config.ownerDataRoot].some((a, index, paths) => paths.some((b, other) => index !== other && (overlap(a, b) || overlap(b, a))))) throw failure();
}

/** Owner process only. All Tjommie CLI launches must use this boundary while Logins is enabled. */
export class WindowsWorkerBoundary {
  readonly #options: Options;
  #generation = 0;
  #leases = 0;
  #closed = false;
  readonly #children = new Map<number, z.infer<typeof Receipt>>();
  #peer: Pick<WindowsWorkerBoundary, "configuration" | "receipts"> | null = null;
  readonly #reviewScope = new AsyncLocalStorage<string>();
  #reviewLease: string | null = null;
  #launches = 0;
  #terminationUnknown = false;
  constructor(options: Options) { this.#options = options; }
  get purpose(): "provider" | "review" { return this.#options.expectedPurpose ?? "provider"; }
  pairWith(peer: Pick<WindowsWorkerBoundary, "configuration" | "receipts">): void {
    if (this.#peer && this.#peer !== peer || this.#children.size || this.#launches) throw failure();
    validateWorkerPair(this.configuration(), peer.configuration()); this.#peer = peer;
  }
  #knownWorkerPids(): number[] { return [...this.#children.values(), ...(this.#peer?.receipts() ?? [])].map((receipt) => receipt.workerPid); }
  get configured(): boolean { return existsSync(this.#options.configurationPath); }
  get required(): boolean { return this.configured || existsSync(join(dirname(this.#options.configurationPath), "vault-protection-required")); }
  configuration(): WindowsWorkerConfiguration {
    if (process.platform !== "win32" || this.#closed) throw failure();
    const config = WindowsWorkerConfiguration.parse(JSON.parse(readFileSync(this.#options.configurationPath, "utf8")));
    if (config.purpose !== this.purpose || resolve(this.#options.configurationPath).toLowerCase() !== resolve(config.ownerDataRoot, configName(this.purpose)).toLowerCase()) throw failure();
    const peer = WindowsWorkerConfiguration.parse(JSON.parse(readFileSync(join(config.ownerDataRoot, configName(this.purpose === "provider" ? "review" : "provider")), "utf8")));
    validateWorkerPair(config, peer); validateWorkerPair(peer, config);
    const canonical = (value: WindowsWorkerConfiguration): WindowsWorkerConfiguration => ({ ...value, workerRoot: realpathSync(value.workerRoot), peerRoot: realpathSync(value.peerRoot), runtimeRoot: realpathSync(value.runtimeRoot), ownerDataRoot: realpathSync(value.ownerDataRoot) });
    validateWorkerPair(canonical(config), canonical(peer));
    return config;
  }
  async unavailableReason(): Promise<string | null> {
    try {
      if (this.#terminationUnknown) throw failure();
      const config = this.configuration();
      const peer = WindowsWorkerConfiguration.parse(JSON.parse(readFileSync(join(config.ownerDataRoot, configName(this.purpose === "provider" ? "review" : "provider")), "utf8")));
      for (const current of [config, peer]) {
        validateWorkerAcceptance(current, current === config ? peer : config, JSON.parse(readFileSync(join(config.ownerDataRoot, acceptanceName(current.purpose)), "utf8")));
      }
      if (!this.#options.exclusive()) throw failure();
      await this.#verify(config);
      return null;
    } catch { return unavailable; }
  }
  async acquire(): Promise<{ assert(): void; release(): void }> {
    if (await this.unavailableReason()) throw failure();
    const generation = this.#generation; this.#leases++;
    let released = false;
    return {
      assert: () => { if (released || this.#closed || this.#terminationUnknown || generation !== this.#generation || !this.#options.exclusive()) throw failure(); },
      release: () => { if (!released) { released = true; this.#leases--; } },
    };
  }
  async grantDirectory(path: string): Promise<void> {
    const config = this.configuration();
    const helper = this.#options.helperPath ?? fileURLToPath(new URL("./windows-worker.ps1", import.meta.url));
    const canonical = grantTarget(config, helper, path);
    const systemRoot = process.env.SystemRoot ?? "C:\\Windows";
    await new Promise<void>((done, reject) => {
      const child = spawn(join(systemRoot, "System32", "WindowsPowerShell", "v1.0", "powershell.exe"), ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", directoryGrant], { stdio: ["pipe", "ignore", "ignore"], windowsHide: true });
      const timer = setTimeout(() => { child.kill(); reject(failure()); }, 10_000); timer.unref();
      child.once("error", () => { clearTimeout(timer); reject(failure()); }); child.once("close", (code) => { clearTimeout(timer); code === 0 ? done() : reject(failure()); });
      child.stdin.on("error", () => reject(failure())); child.stdin.end(JSON.stringify({ path: canonical, sid: config.workerSid, peerSid: config.peerSid }));
    });
  }
  /** Call before any legacy launch or boundary change; active private operations fail closed. */
  invalidate(): void { this.#generation++; }
  async #verify(config: WindowsWorkerConfiguration): Promise<void> {
    const systemRoot = process.env.SystemRoot ?? "C:\\Windows";
    const helper = this.#options.helperPath ?? fileURLToPath(new URL("./windows-worker.ps1", import.meta.url));
    await new Promise<void>((done, reject) => {
      const child = spawn(join(systemRoot, "System32", "WindowsPowerShell", "v1.0", "powershell.exe"), ["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", helper], { stdio: ["pipe", "pipe", "ignore"], windowsHide: true });
      let output = "";
      const timer = setTimeout(() => { child.kill(); reject(failure()); }, 30_000); timer.unref();
      child.once("error", () => { clearTimeout(timer); reject(failure()); });
      child.stdout.on("data", (chunk: Buffer) => { if (output.length + chunk.length > 100) { child.kill(); reject(failure()); } else output += chunk.toString("utf8"); });
      child.once("close", (code) => { clearTimeout(timer); if (code === 0 && output === "verified") done(); else reject(failure()); });
      child.stdin.on("error", () => reject(failure())); child.stdin.end(JSON.stringify({ mode: "verify", config, allowedWorkerPids: this.#knownWorkerPids() }) + "\n");
    });
  }
  async spawn(file: string, args: string[], options: { cwd: string; env: Record<string, string> }): Promise<ChildProcessWithoutNullStreams> {
    if (this.purpose === "review" && !this.#reviewLease) throw new Error("review_isolated_identity_unavailable");
    if (this.#reviewLease !== null && this.#reviewScope.getStore() !== this.#reviewLease) throw new WorkerPolicyBusyError();
    if (this.#reviewLease !== null && (this.#children.size || this.#launches)) throw new WorkerPolicyBusyError();
    if (this.#terminationUnknown) throw new WorkerLaunchTerminationError();
    this.#launches++;
    try {
    const config = this.configuration();
    if (!this.#options.exclusive() || !inside(config.runtimeRoot, file) || !/\.exe$/i.test(file)) throw failure();
    for (const name of ["CODEX_HOME", "CLAUDE_CONFIG_DIR"]) if (options.env[name] && !inside(config.workerRoot, options.env[name])) throw failure();
    await this.#verify(config);
    await this.grantDirectory(options.cwd);
    for (const name of ["CODEX_HOME", "CLAUDE_CONFIG_DIR"]) if (options.env[name]) await this.grantDirectory(options.env[name]);
    if (!inside(config.workerRoot, options.cwd)) {
      // External worktrees are provisioned explicitly; the helper verifies their worker ACL.
      if (inside(config.ownerDataRoot, options.cwd) && resolve(options.cwd).toLowerCase() === resolve(config.ownerDataRoot).toLowerCase()) throw failure();
    }
    const transport = this.#options.transport ?? windowsCredentialTransport;
    let password = "";
    try {
      const saved = await transport({ operation: "read", target: config.credentialTarget });
      if (!saved) throw failure();
      const value: unknown = JSON.parse(saved);
      if (!value || typeof value !== "object" || !("username" in value) || value.username !== config.workerName || !("password" in value) || typeof value.password !== "string" || !value.password) throw failure();
      password = value.password;
      const receiptRoot = join(config.ownerDataRoot, "worker-receipts"); mkdirSync(receiptRoot, { recursive: true });
      const receiptPath = join(receiptRoot, `${randomUUID()}.json`);
      const systemRoot = process.env.SystemRoot ?? "C:\\Windows";
      const helper = this.#options.helperPath ?? fileURLToPath(new URL("./windows-worker.ps1", import.meta.url));
      const child = spawn(join(systemRoot, "System32", "WindowsPowerShell", "v1.0", "powershell.exe"), ["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", helper], {
        env: { SystemRoot: systemRoot, WINDIR: systemRoot, TEMP: process.env.TEMP ?? join(systemRoot, "Temp"), TMP: process.env.TMP ?? join(systemRoot, "Temp") },
        stdio: ["pipe", "pipe", "pipe"], windowsHide: true,
      });
      child.stdin.write(JSON.stringify({ config, password, file, args, cwd: options.cwd, env: options.env, receiptPath, allowedWorkerPids: this.#knownWorkerPids() }) + "\n");
      password = "";
      const retired = new Promise<void>((done, reject) => {
        const closed = (): void => {
          try {
            const proof: unknown = JSON.parse(readFileSync(receiptPath + ".closed", "utf8"));
            if (!proof || typeof proof !== "object" || !("wrapperPid" in proof) || proof.wrapperPid !== child.pid || !("stopped" in proof) || proof.stopped !== true) throw failure();
            done();
          } catch { this.#terminationUnknown = true; this.invalidate(); reject(new WorkerLaunchTerminationError()); }
          finally { rmSync(receiptPath + ".closed", { force: true }); }
        };
        child.once("close", closed); child.once("error", () => { this.#terminationUnknown = true; this.invalidate(); reject(new WorkerLaunchTerminationError()); });
      });
      void retired.catch(() => {}); exitProofs.set(child, retired);
      try {
        let failed = false; child.once("error", () => { failed = true; });
        const deadline = Date.now() + 40_000;
        while (!existsSync(receiptPath)) {
          if (failed || child.exitCode !== null || Date.now() >= deadline) throw failure();
          await new Promise<void>((done) => setTimeout(done, 20));
        }
        const receipt = Receipt.parse(JSON.parse(readFileSync(receiptPath, "utf8")));
        if (receipt.purpose !== config.purpose || receipt.ownerSid !== config.ownerSid || receipt.workerSid !== config.workerSid || receipt.wrapperPid !== child.pid) throw failure();
        this.#children.set(child.pid!, receipt); child.once("exit", () => this.#children.delete(child.pid!));
        child.stdin.write("continue\n");
        return child;
      } catch {
        child.stdin.end();
        let timer: NodeJS.Timeout | undefined;
        try { await Promise.race([retired, new Promise<never>((_done, reject) => { timer = setTimeout(() => { child.kill(); reject(new WorkerLaunchTerminationError()); }, 7000); timer.unref(); })]); }
        finally { clearTimeout(timer); }
        throw failure();
      }
      finally { rmSync(receiptPath, { force: true }); }
    } catch (error) { if (error instanceof WorkerLaunchTerminationError) { this.#terminationUnknown = true; throw error; } throw failure(); }
    finally { password = ""; }
    } catch (error) {
      if (error instanceof WorkerLaunchTerminationError) { this.#terminationUnknown = true; this.invalidate(); throw error; }
      throw failure();
    } finally { this.#launches--; }
  }
  async observeReviewPolicy(): Promise<WorkerPolicyReceipt> {
    if (!this.#reviewLease || this.#reviewScope.getStore() !== this.#reviewLease || this.#children.size || this.#launches) throw new WorkerPolicyBusyError();
    const config = this.configuration(), scratch = join(config.workerRoot, "policy-observer"); mkdirSync(scratch, { recursive: true });
    const child = await this.spawn(join(config.runtimeRoot, "node.exe"), ["-e", WORKER_POLICY_OBSERVER], { cwd: scratch, env: {} });
    let output = ""; child.stdin.end();
    const code = await new Promise<number | null>((done, reject) => {
      const timer = setTimeout(() => { child.kill(); reject(new WorkerLaunchTerminationError()); }, 20_000); timer.unref();
      child.stdout.on("data", (bytes: Buffer) => { if (output.length + bytes.length > 4096) { child.kill(); reject(failure()); } else output += bytes.toString("utf8"); });
      child.stderr.resume(); child.once("error", () => { clearTimeout(timer); reject(failure()); }); child.once("close", (status) => { clearTimeout(timer); done(status); });
    });
    await workerProcessRetired(child);
    const value = z.object({ version: z.literal(1), registryAbsent: z.literal(true), files: z.array(z.tuple([z.enum(["managed-settings.json", "managed-mcp.json"]), z.string().regex(/^[a-f0-9]{64}$/).nullable()])).length(2) }).strict().parse(JSON.parse(output));
    if (code !== 0) throw failure();
    return Object.freeze({ purpose: "review", workerSid: config.workerSid, providerSid: config.peerSid, configurationSha256: workerConfigurationHash(config), observerSha256: createHash("sha256").update(WORKER_POLICY_OBSERVER).digest("hex"), policySha256: createHash("sha256").update(JSON.stringify(value)).digest("hex") });
  }
  async withReviewPolicy<T>(action: (receipt: WorkerPolicyReceipt) => Promise<T>): Promise<T> {
    if (this.purpose !== "review") throw new Error("review_isolated_identity_unavailable");
    if (this.#reviewLease || this.#children.size || this.#launches) throw new WorkerPolicyBusyError();
    const lease = randomUUID(); this.#reviewLease = lease;
    return this.#reviewScope.run(lease, async () => {
      let caught: unknown;
      try {
        const before = await this.observeReviewPolicy();
        const result = await action(before);
        const after = await this.observeReviewPolicy();
        if (JSON.stringify(before) !== JSON.stringify(after)) throw new Error("review_worker_policy_changed");
        return result;
      } catch (error) {
        caught = error;
        if (error instanceof WorkerLaunchTerminationError) { this.#terminationUnknown = true; this.invalidate(); }
        throw error;
      } finally {
        if (this.#children.size || this.#launches) { this.#terminationUnknown = true; this.invalidate(); }
        if (this.#terminationUnknown) {
          if (!(caught instanceof WorkerLaunchTerminationError)) throw new WorkerLaunchTerminationError();
        } else if (this.#reviewLease === lease) this.#reviewLease = null;
      }
    });
  }
  receipts(): readonly z.infer<typeof Receipt>[] { return [...this.#children.values()]; }
  close(): void { this.#closed = true; this.invalidate(); }
}
