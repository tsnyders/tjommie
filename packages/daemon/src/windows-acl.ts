import { spawn, spawnSync } from "node:child_process";
import { lstatSync, readdirSync } from "node:fs";
import { isAbsolute, join, relative, resolve, sep } from "node:path";
import { SETUP_CODEX_SANDBOX_REASON, type Provider } from "@tjommie/protocol";
import type { Paths } from "./paths.ts";

export type ProtectedAccount = { id: string; provider: Provider; folder: string };
export type BotHome = { id: string; homeFolder: string; provider: Provider };
export type AclTarget = { path: string; folder: boolean; mode: "deny" | "allow" | "removeDeny" | "verifyBin" | "checkDeny" | "checkAllow" | "checkNoDeny"; codexDependent?: boolean };
export type AclPassResult = { sid: string | null; groupMissing: boolean; failures: string[]; applied: number };
export type AclDriver = {
  runSync: (targets: AclTarget[]) => AclPassResult;
  run: (targets: AclTarget[]) => Promise<AclPassResult>;
};
export type Protection =
  | { status: "ready"; ready: true; reason: null }
  | { status: "not_configured" | "check_failed"; ready: false; reason: string };

// Measured on Codex 0.155.1. Codex owns the protected .sandbox-bin ACL.
export const CODEX_MAIN_ACL_RECIPE = {
  home: { entry: ".", mode: "deny" },
  sandbox: { entry: ".sandbox", mode: "allow" },
  sandboxBin: { entry: ".sandbox-bin", mode: "verifyBin" },
} as const;

const systemRoot = process.env.SystemRoot ?? process.env.SYSTEMROOT ?? "C:\\Windows";
const powershell = join(systemRoot, "System32", "WindowsPowerShell", "v1.0", "powershell.exe");
const icacls = join(systemRoot, "System32", "icacls.exe");
const failureReason = "Tjommie couldn't check Codex's sandbox. If Codex was updated, run its sandbox setup again.";

// A daemon launched from PowerShell 7 can inherit incompatible Core modules. This pass needs only
// Windows PowerShell's built-in modules; keep the override local to each child and preserve other env.
function powershellEnvironment(): NodeJS.ProcessEnv {
  const inherited = Object.fromEntries(Object.entries(process.env).filter(([key]) => key.toUpperCase() !== "PSMODULEPATH"));
  return { ...inherited, PSModulePath: join(systemRoot, "System32", "WindowsPowerShell", "v1.0", "Modules") };
}

export type AclCommand = { file: string; args: string[] };
function buildAceCommand(path: string, sid: string, folder: boolean, ace: "deny" | "allow"): AclCommand {
  if (!/^S-1-\d+(?:-\d+)+$/.test(sid)) throw new Error("Invalid Codex sandbox group SID");
  return { file: icacls, args: [path, ace === "deny" ? "/deny" : "/grant", `*${sid}:${folder ? "(OI)(CI)" : ""}(RX,W)`] };
}
export const buildDenyCommand = (path: string, sid: string, folder: boolean): AclCommand => buildAceCommand(path, sid, folder, "deny");
export const buildAllowCommand = (path: string, sid: string, folder: boolean): AclCommand => buildAceCommand(path, sid, folder, "allow");

// One PowerShell process resolves the SID, checks every target, applies missing ACEs and verifies them.
// Paths arrive on stdin as JSON so a large bot list does not hit the Windows env limit.
const passScript = `
$ErrorActionPreference = 'Stop'
[Console]::InputEncoding = [System.Text.UTF8Encoding]::new($false)
[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false)
$payload = [Console]::In.ReadToEnd() | ConvertFrom-Json
try {
  $sid = ([System.Security.Principal.NTAccount]::new($env:COMPUTERNAME, 'CodexSandboxUsers')).Translate([System.Security.Principal.SecurityIdentifier]).Value
} catch [System.Security.Principal.IdentityNotMappedException] {
  @{ sid = $null; groupMissing = $true; failures = @(); applied = 0 } | ConvertTo-Json -Compress -Depth 4
  exit 0
}
if ($sid -notmatch '^S-1-\\d+(?:-\\d+)+$') { throw 'Invalid Codex sandbox group SID' }
$rx = [int][System.Security.AccessControl.FileSystemRights]::ReadAndExecute
$write = [int][System.Security.AccessControl.FileSystemRights]::Write
$oi = [System.Security.AccessControl.InheritanceFlags]::ObjectInherit
$ci = [System.Security.AccessControl.InheritanceFlags]::ContainerInherit
$failures = [System.Collections.Generic.List[string]]::new()
$applied = 0
$codexBinReady = $true
function Test-Target($target) {
  $acl = Get-Acl -LiteralPath $target.path
  if ($target.mode -eq 'verifyBin' -and -not $acl.AreAccessRulesProtected) { return $false }
  foreach ($rule in $acl.Access) {
    try { $identity = $rule.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value } catch { continue }
    if ($identity -ne $sid) { continue }
    $rights = [int]$rule.FileSystemRights
    if ($target.mode -eq 'removeDeny' -or $target.mode -eq 'checkNoDeny') {
      if ($rule.AccessControlType -eq 'Deny') { return $false }
      continue
    }
    if ($rule.IsInherited) { continue }
    if ($target.mode -eq 'verifyBin') {
      if ($rule.AccessControlType -eq 'Allow' -and ($rights -band $rx) -eq $rx) { return $true }
      continue
    }
    $kind = if ($target.mode -eq 'deny' -or $target.mode -eq 'checkDeny') { 'Deny' } else { 'Allow' }
    if ($rule.AccessControlType -ne $kind -or ($rights -band $rx) -ne $rx -or ($rights -band $write) -ne $write) { continue }
    if ($target.folder) {
      $inheritable = (($rule.InheritanceFlags -band $oi) -eq $oi) -and (($rule.InheritanceFlags -band $ci) -eq $ci)
      if (-not $inheritable) { continue }
    }
    return $true
  }
  return ($target.mode -eq 'removeDeny' -or $target.mode -eq 'checkNoDeny')
}
foreach ($target in $payload.targets) {
  if ($target.codexDependent -and -not $codexBinReady) { continue }
  try {
    $present = Test-Target $target
    if ($target.mode -eq 'verifyBin') {
      if (-not $present) { $codexBinReady = $false; $failures.Add("$($target.path): .sandbox-bin needs a protected CodexSandboxUsers ReadAndExecute allow") }
      continue
    }
    if ($target.mode -like 'check*') {
      if (-not $present) { $failures.Add("$($target.path): expected ACL state was not found") }
      continue
    }
    if (-not $present) {
      if ($target.mode -eq 'removeDeny') {
        & $payload.icacls $target.path '/remove:d' "*$sid" | Out-Null
      } else {
        $verb = if ($target.mode -eq 'deny') { '/deny' } else { '/grant' }
        $inheritance = if ($target.folder) { '(OI)(CI)' } else { '' }
        & $payload.icacls $target.path $verb "*\${sid}:\${inheritance}(RX,W)" | Out-Null
      }
      if ($LASTEXITCODE -ne 0) { throw "icacls exited with code $LASTEXITCODE" }
      $applied++
      if (-not (Test-Target $target)) { throw 'ACE was not verified after applying' }
    }
  } catch {
    if ($target.mode -eq 'verifyBin') { $codexBinReady = $false }
    $failures.Add("$($target.path): $($_.Exception.Message)")
  }
}
@{ sid = $sid; groupMissing = $false; failures = @($failures); applied = $applied } | ConvertTo-Json -Compress -Depth 4
`;

function passInput(targets: AclTarget[]): string {
  return JSON.stringify({ icacls, targets });
}

function parsePass(stdout: string): AclPassResult {
  const parsed: unknown = JSON.parse(stdout.trim());
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) throw new Error("PowerShell returned an invalid ACL result");
  const result = parsed as Record<string, unknown>;
  if ((result.sid !== null && (typeof result.sid !== "string" || !/^S-1-\d+(?:-\d+)+$/.test(result.sid))) ||
      typeof result.groupMissing !== "boolean" || !Array.isArray(result.failures) ||
      !result.failures.every((failure) => typeof failure === "string") || typeof result.applied !== "number") {
    throw new Error("PowerShell returned an invalid ACL result");
  }
  return { sid: result.sid, groupMissing: result.groupMissing, failures: result.failures, applied: result.applied };
}

function commandFailure(code: number | null, stderr: string): Error {
  return new Error(`PowerShell ACL pass exited with code ${code}: ${stderr.trim()}`);
}

export const windowsAclDriver: AclDriver = {
  runSync: (targets) => {
    const result = spawnSync(powershell, ["-NoProfile", "-NonInteractive", "-Command", passScript], {
      input: passInput(targets), encoding: "utf8", windowsHide: true, timeout: 60_000, maxBuffer: 1024 * 1024, env: powershellEnvironment(),
    });
    if (result.error) {
      if ("code" in result.error && result.error.code === "ETIMEDOUT") throw new Error("PowerShell ACL pass timed out.");
      throw new Error(`PowerShell ACL pass could not start: ${result.error.message}`);
    }
    if (result.status !== 0) throw commandFailure(result.status, result.stderr);
    return parsePass(result.stdout);
  },
  run: (targets) => new Promise((resolvePass, rejectPass) => {
    const child = spawn(powershell, ["-NoProfile", "-NonInteractive", "-Command", passScript], { windowsHide: true, stdio: ["pipe", "pipe", "pipe"], env: powershellEnvironment() });
    let stdout = "";
    let stderr = "";
    const timer = setTimeout(() => {
      rejectPass(new Error("PowerShell ACL pass timed out."));
      child.kill();
    }, 60_000);
    child.stdout.setEncoding("utf8").on("data", (chunk: string) => { stdout += chunk; });
    child.stderr.setEncoding("utf8").on("data", (chunk: string) => { stderr += chunk; });
    child.on("error", (error) => { clearTimeout(timer); rejectPass(new Error(`PowerShell ACL pass could not start: ${error.message}`)); });
    child.on("close", (code) => {
      clearTimeout(timer);
      if (code !== 0) rejectPass(commandFailure(code, stderr));
      else {
        try { resolvePass(parsePass(stdout)); } catch (error) { rejectPass(error); }
      }
    });
    child.stdin.end(passInput(targets));
  }),
};

function targetIfPresent(root: string, path: string, folder: boolean): AclTarget | null {
  try {
    const base = resolve(root);
    const target = resolve(path);
    const rel = relative(base, target);
    if (!rel || rel === ".." || rel.startsWith(`..${sep}`) || isAbsolute(rel)) throw new Error("protected path is outside its protection root");
    let ancestor = base;
    if (lstatSync(ancestor).isSymbolicLink()) throw new Error("protection root is a reparse point");
    for (const part of rel.split(sep).slice(0, -1)) {
      ancestor = join(ancestor, part);
      if (lstatSync(ancestor).isSymbolicLink()) throw new Error("protected path has a reparse-point parent");
    }
    const stat = lstatSync(path);
    if (stat.isSymbolicLink() || (folder ? !stat.isDirectory() : !stat.isFile())) throw new Error("unexpected path type");
    return { path, folder, mode: "deny" };
  } catch (error) {
    if (error && typeof error === "object" && "code" in error && error.code === "ENOENT") return null;
    throw error;
  }
}

type AclPlan = { targets: AclTarget[]; failures: string[]; expected: string[]; skipped: string[]; setupMissing: boolean };
function planTargets(paths: Paths, botHomes: BotHome[], accounts?: ProtectedAccount[]): AclPlan {
  const plan: AclPlan = { targets: [], failures: [], expected: [], skipped: [], setupMissing: false };
  const add = (path: string, folder: boolean, mode: AclTarget["mode"], required: boolean, codexDependent = false, missingMessage?: string, anchor = paths.root): boolean => {
    try {
      const target = targetIfPresent(anchor, path, folder);
      if (!target) {
        if (required) plan.expected.push(`${path}: required folder is missing`);
        else if (missingMessage) plan.skipped.push(`${path}: ${missingMessage}`);
        return false;
      }
      plan.targets.push({ ...target, mode, codexDependent });
      return true;
    } catch (error) {
      plan.failures.push(`${path}: ${String(error)}`);
      return false;
    }
  };
  // The owner may explicitly place account profiles outside the application data root.
  // Account identity still fixes the exact child path; every target must pass the same
  // containment and reparse-point checks beneath that configured account root.
  const addAccount = (path: string, mode: AclTarget["mode"], required: boolean, codexDependent = false): boolean =>
    add(path, true, mode, required, codexDependent, undefined, paths.accounts);
  const owned = accounts ?? [{ id: "claude-main", provider: "claude" as const, folder: join(paths.accounts, "claude-main") }, { id: "codex-main", provider: "codex" as const, folder: join(paths.accounts, "codex-main") }];
  const valid = owned.filter((account) => {
    if (resolve(account.folder).toLowerCase() === resolve(paths.accounts, account.id).toLowerCase() && !account.id.includes("/") && !account.id.includes("\\") && account.id !== "." && account.id !== "..") return true;
    plan.failures.push(`${account.id}: account folder does not match its id`); return false;
  });
  for (const account of valid.filter((account) => account.provider === "claude")) addAccount(account.folder, "deny", true);
  for (const folder of [paths.logs, paths.data, paths.sharedFiles]) add(folder, true, "deny", true);
  add(paths.token, false, "deny", false);
  try {
    for (const name of readdirSync(paths.data).filter((entry) => /^tjommie\.db(?:$|[-.])/.test(entry))) {
      add(join(paths.data, name), false, "deny", false);
    }
  } catch (error) { plan.failures.push(`database files: ${String(error)}`); }
  for (const bot of botHomes) {
    const expected = resolve(paths.bots, bot.id);
    if (resolve(bot.homeFolder).toLowerCase() !== expected.toLowerCase()) {
      plan.failures.push(`${bot.homeFolder}: bot folder does not match its id`);
      continue;
    }
    const homePresent = add(bot.homeFolder, true, bot.provider === "claude" ? "deny" : "removeDeny", false, false,
      `${bot.provider === "claude" ? "Claude" : "Codex"} bot folder is missing; nothing to protect.`);
    if (bot.provider === "codex" && homePresent) {
      add(join(bot.homeFolder, "notes.md"), false, "checkNoDeny", false);
      add(join(bot.homeFolder, "inbox"), true, "checkNoDeny", false);
    }
  }
  for (const account of valid.filter((account) => account.provider === "codex")) {
  const home = account.folder;
  const sandbox = join(home, ".sandbox");
  const sandboxBin = join(home, ".sandbox-bin");
  const homePresent = addAccount(home, "verifyBin", false);
  if (homePresent) plan.targets.pop();
  const sandboxPresent = homePresent && addAccount(sandbox, "verifyBin", false);
  if (sandboxPresent) plan.targets.pop();
  if (!homePresent || !sandboxPresent || !addAccount(sandboxBin, "verifyBin", false)) {
    // A newly added, unconfigured account must be protected without blocking a different,
    // configured account. Its own runtime readiness gate prevents Own folder execution.
    if (homePresent) addAccount(home, "deny", true);
    if (accounts === undefined) plan.setupMissing = true;
  } else {
    addAccount(home, "deny", true, true);
    addAccount(sandbox, "allow", true, true);
  }
  }
  return plan;
}

function finish(plan: AclPlan, result: AclPassResult): Protection {
  if (result.groupMissing) console.info(`Codex data ACLs: CodexSandboxUsers group does not exist yet. ${SETUP_CODEX_SANDBOX_REASON}.`);
  for (const message of plan.expected) console.info(`Codex data ACLs: ${message}`);
  for (const message of plan.skipped) console.info(`Codex data ACLs: ${message}`);
  if (plan.setupMissing) console.info("Codex data ACLs: Codex setup marker is not present yet.");
  for (const message of [...plan.failures, ...result.failures]) console.error(`Codex data ACLs: ${message}`);
  if (plan.failures.length || result.failures.length || plan.expected.length) return { status: "check_failed", ready: false, reason: failureReason };
  if (result.groupMissing || plan.setupMissing) return { status: "not_configured", ready: false, reason: SETUP_CODEX_SANDBOX_REASON };
  return { status: "ready", ready: true, reason: null };
}

function failedPass(error: unknown): Protection {
  console.error(`Codex data ACLs: ${String(error)}`);
  return { status: "check_failed", ready: false, reason: failureReason };
}

export function protectCodexData(paths: Paths, botHomes: BotHome[] = [], driver: AclDriver = windowsAclDriver, platform: NodeJS.Platform = process.platform, accounts?: ProtectedAccount[]): Protection {
  if (platform !== "win32") { console.info("Codex data ACLs: Windows only; no changes made."); return { status: "ready", ready: true, reason: null }; }
  const plan = planTargets(paths, botHomes, accounts);
  try { return finish(plan, driver.runSync(plan.targets)); } catch (error) { return failedPass(error); }
}

export async function protectCodexDataAsync(paths: Paths, botHomes: BotHome[] = [], driver: AclDriver = windowsAclDriver, platform: NodeJS.Platform = process.platform, accounts?: ProtectedAccount[]): Promise<Protection> {
  if (platform !== "win32") { console.info("Codex data ACLs: Windows only; no changes made."); return { status: "ready", ready: true, reason: null }; }
  const plan = planTargets(paths, botHomes, accounts);
  try { return finish(plan, await driver.run(plan.targets)); } catch (error) { return failedPass(error); }
}
