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
  version: z.literal(1), ownerSid: Sid, workerSid: Sid, workerName: z.string().regex(/^TjWorker-[a-f0-9]{8}$/),
  workerRoot: z.string().min(1), runtimeRoot: z.string().min(1), ownerDataRoot: z.string().min(1),
  provisionedAt: z.string().datetime(),
  credentialTarget: z.string().regex(/^Tjommie\/[a-f0-9]{24}\/isolated-worker$/),
}).strict();
export type WindowsWorkerConfiguration = z.infer<typeof WindowsWorkerConfiguration>;
const Receipt = z.object({ version: z.literal(1), ownerSid: Sid, workerSid: Sid, wrapperPid: z.number().int().positive(), workerPid: z.number().int().positive(), elevated: z.literal(false), privileged: z.literal(false), jobBound: z.literal(true) }).strict();
const Acceptance = z.object({ version: z.literal(1), configurationHash: z.string().regex(/^[a-f0-9]{64}$/), ownerFileDenied: z.literal(true), ownerCredentialDenied: z.literal(true), ownerProcessDenied: z.literal(true), ownerDesktopDenied: z.literal(true), workerWriteAllowed: z.literal(true), codexStarted: z.literal(true), claudeStarted: z.literal(true), reviewPolicyVerified: z.literal(true) }).strict();
const unavailable = "Logins needs the verified Windows worker account. Set up and verify provider isolation on the host before saving or filling logins.";
const failure = (): Error => new Error("The isolated Windows provider could not start. No owner credential was sent to it.");
export class WorkerLaunchTerminationError extends Error {
  constructor() { super("The Windows worker launch could not confirm process-tree termination. Keep its account and working folders reserved."); }
}
const exitProofs = new WeakMap<ChildProcessWithoutNullStreams, Promise<void>>();
export function workerProcessRetired(child: ChildProcessWithoutNullStreams): Promise<void> | undefined { return exitProofs.get(child); }
export type WorkerPolicyReceipt = Readonly<{ workerSid: string; observerSha256: string; policySha256: string }>;
export class WorkerPolicyBusyError extends Error { constructor() { super("Other worker jobs are running. Review can start after they finish."); } }
export function workerConfigurationHash(config: WindowsWorkerConfiguration): string { return createHash("sha256").update(JSON.stringify(WindowsWorkerConfiguration.parse(config))).digest("hex"); }
function inside(root: string, path: string): boolean {
  const part = relative(realpathSync(root), realpathSync(path)); return part === "" || !part.startsWith("..") && !isAbsolute(part);
}
type Options = { configurationPath: string; helperPath?: string; transport?: CredentialTransport; exclusive: () => boolean };

/** Owner process only. All Tjommie CLI launches must use this boundary while Logins is enabled. */
export class WindowsWorkerBoundary {
  readonly #options: Options;
  #generation = 0;
  #leases = 0;
  #closed = false;
  readonly #children = new Map<number, z.infer<typeof Receipt>>();
  readonly #reviewScope = new AsyncLocalStorage<string>();
  #reviewLease: string | null = null;
  #launches = 0;
  #terminationUnknown = false;
  constructor(options: Options) { this.#options = options; }
  get configured(): boolean { return existsSync(this.#options.configurationPath); }
  get required(): boolean { return this.configured || existsSync(join(dirname(this.#options.configurationPath), "vault-protection-required")); }
  configuration(): WindowsWorkerConfiguration {
    if (process.platform !== "win32" || this.#closed) throw failure();
    const config = WindowsWorkerConfiguration.parse(JSON.parse(readFileSync(this.#options.configurationPath, "utf8")));
    if (config.ownerSid === config.workerSid || ![config.workerRoot, config.runtimeRoot, config.ownerDataRoot].every(isAbsolute)
      || resolve(dirname(this.#options.configurationPath)).toLowerCase() !== resolve(config.ownerDataRoot).toLowerCase()
      || inside(config.workerRoot, config.ownerDataRoot) || inside(config.runtimeRoot, config.ownerDataRoot)) throw failure();
    return config;
  }
  async unavailableReason(): Promise<string | null> {
    try {
      if (this.#terminationUnknown) throw failure();
      const config = this.configuration();
      const accepted = Acceptance.parse(JSON.parse(readFileSync(join(config.ownerDataRoot, "worker-acceptance.json"), "utf8")));
      if (accepted.configurationHash !== workerConfigurationHash(config) || !this.#options.exclusive()) throw failure();
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
    const config = this.configuration(), canonical = realpathSync(path);
    const protectedPaths = [config.ownerDataRoot, config.runtimeRoot, dirname(this.#options.helperPath ?? fileURLToPath(new URL("./windows-worker.ps1", import.meta.url)))];
    if (protectedPaths.some((protectedPath) => inside(canonical, protectedPath))) throw failure();
    const systemRoot = process.env.SystemRoot ?? "C:\\Windows";
    const script = "$ErrorActionPreference='Stop';try{$p=[Console]::In.ReadToEnd()|ConvertFrom-Json;$a=Get-Acl -LiteralPath $p.path;$r=New-Object Security.AccessControl.FileSystemAccessRule($p.sid,'Modify','ContainerInherit,ObjectInherit','None','Allow');$a.AddAccessRule($r);Set-Acl -LiteralPath $p.path -AclObject $a}catch{exit 1}";
    await new Promise<void>((done, reject) => {
      const child = spawn(join(systemRoot, "System32", "WindowsPowerShell", "v1.0", "powershell.exe"), ["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script], { stdio: ["pipe", "ignore", "ignore"], windowsHide: true });
      const timer = setTimeout(() => { child.kill(); reject(failure()); }, 10_000); timer.unref();
      child.once("error", () => { clearTimeout(timer); reject(failure()); }); child.once("close", (code) => { clearTimeout(timer); code === 0 ? done() : reject(failure()); });
      child.stdin.on("error", () => reject(failure())); child.stdin.end(JSON.stringify({ path: canonical, sid: config.workerSid }));
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
      const timer = setTimeout(() => { child.kill(); reject(failure()); }, 10_000); timer.unref();
      child.once("error", () => { clearTimeout(timer); reject(failure()); });
      child.stdout.on("data", (chunk: Buffer) => { if (output.length + chunk.length > 100) { child.kill(); reject(failure()); } else output += chunk.toString("utf8"); });
      child.once("close", (code) => { clearTimeout(timer); if (code === 0 && output === "verified") done(); else reject(failure()); });
      child.stdin.on("error", () => reject(failure())); child.stdin.end(JSON.stringify({ mode: "verify", config, quiescent: this.#reviewLease !== null, allowedWorkerPids: [...this.#children.values()].map((receipt) => receipt.workerPid) }) + "\n");
    });
  }
  async spawn(file: string, args: string[], options: { cwd: string; env: Record<string, string> }): Promise<ChildProcessWithoutNullStreams> {
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
      child.stdin.write(JSON.stringify({ config, password, file, args, cwd: options.cwd, env: options.env, receiptPath, quiescent: this.#reviewLease !== null, allowedWorkerPids: [...this.#children.values()].map((receipt) => receipt.workerPid) }) + "\n");
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
        const deadline = Date.now() + 15_000;
        while (!existsSync(receiptPath)) {
          if (failed || child.exitCode !== null || Date.now() >= deadline) throw failure();
          await new Promise<void>((done) => setTimeout(done, 20));
        }
        const receipt = Receipt.parse(JSON.parse(readFileSync(receiptPath, "utf8")));
        if (receipt.ownerSid !== config.ownerSid || receipt.workerSid !== config.workerSid || receipt.wrapperPid !== child.pid) throw failure();
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
    return Object.freeze({ workerSid: config.workerSid, observerSha256: createHash("sha256").update(WORKER_POLICY_OBSERVER).digest("hex"), policySha256: createHash("sha256").update(JSON.stringify(value)).digest("hex") });
  }
  async withReviewPolicy<T>(action: (receipt: WorkerPolicyReceipt) => Promise<T>): Promise<T> {
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
