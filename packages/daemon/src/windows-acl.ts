import { lstatSync, readdirSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { isAbsolute, join, relative, resolve, sep } from "node:path";
import type { Paths } from "./paths.ts";

type Target = { path: string; folder: boolean };
export type AclCommand = { file: "icacls.exe"; args: string[] };
export type AclDriver = {
  lookupSid: () => string | null;
  hasDeny: (path: string, sid: string, folder: boolean) => boolean;
  hasAllow: (path: string, sid: string, folder: boolean) => boolean;
  sandboxBinReady: (path: string, sid: string) => boolean;
  apply: (command: AclCommand) => void;
};
export type Protection = { ready: boolean; reason: string | null };

// Measured on Codex 0.155.1. Only these account entries receive or require Tjommie's ACL checks.
// .sandbox-secrets keeps Codex's own deny and is never changed here.
export const CODEX_MAIN_ACL_RECIPE = {
  home: { entry: ".", ace: "deny" },
  sandbox: { entry: ".sandbox", ace: "allow" },
  sandboxBin: { entry: ".sandbox-bin", ace: "verifyProtectedAllow" },
} as const;

function buildAceCommand(path: string, sid: string, folder: boolean, ace: "deny" | "allow"): AclCommand {
  if (!/^S-1-\d+(?:-\d+)+$/.test(sid)) throw new Error("Invalid Codex sandbox group SID");
  return { file: "icacls.exe", args: [path, ace === "deny" ? "/deny" : "/grant", `*${sid}:${folder ? "(OI)(CI)" : ""}(RX,W)`] };
}

export const buildDenyCommand = (path: string, sid: string, folder: boolean): AclCommand => buildAceCommand(path, sid, folder, "deny");
export const buildAllowCommand = (path: string, sid: string, folder: boolean): AclCommand => buildAceCommand(path, sid, folder, "allow");

function run(file: string, args: string[], env: NodeJS.ProcessEnv = process.env): string {
  const result = spawnSync(file, args, { encoding: "utf8", windowsHide: true, env, timeout: 30_000 });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${file} exited with code ${result.status}: ${result.stderr.trim()}`);
  return result.stdout.trim();
}

const lookupScript = "$ErrorActionPreference='Stop'; try { ([System.Security.Principal.NTAccount]::new($env:COMPUTERNAME,'CodexSandboxUsers')).Translate([System.Security.Principal.SecurityIdentifier]).Value } catch { exit 2 }";

const inspectScript = `
$ErrorActionPreference='Stop'
$acl = Get-Acl -LiteralPath $env:TJOMMIE_ACL_PATH
$rx = [int][System.Security.AccessControl.FileSystemRights]::ReadAndExecute
$write = [int][System.Security.AccessControl.FileSystemRights]::Write
$oi = [System.Security.AccessControl.InheritanceFlags]::ObjectInherit
$ci = [System.Security.AccessControl.InheritanceFlags]::ContainerInherit
$found = $false
foreach ($rule in $acl.Access) {
  try { $identity = $rule.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value } catch { continue }
  $rights = [int]$rule.FileSystemRights
  $inheritable = (($rule.InheritanceFlags -band $oi) -eq $oi) -and (($rule.InheritanceFlags -band $ci) -eq $ci)
  if ($identity -eq $env:TJOMMIE_ACL_SID -and $rule.AccessControlType -eq $env:TJOMMIE_ACL_KIND -and
      ($rights -band $rx) -eq $rx -and ($rights -band $write) -eq $write -and
      (-not $env:TJOMMIE_ACL_FOLDER -or (-not $rule.IsInherited -and $inheritable))) { $found = $true; break }
}
Write-Output $found
`;

const inspectSandboxBinScript = `
$ErrorActionPreference='Stop'
$acl = Get-Acl -LiteralPath $env:TJOMMIE_ACL_PATH
$rx = [int][System.Security.AccessControl.FileSystemRights]::ReadAndExecute
$found = $false
if ($acl.AreAccessRulesProtected) {
  foreach ($rule in $acl.Access) {
    try { $identity = $rule.IdentityReference.Translate([System.Security.Principal.SecurityIdentifier]).Value } catch { continue }
    $rights = [int]$rule.FileSystemRights
    if ($identity -eq $env:TJOMMIE_ACL_SID -and -not $rule.IsInherited -and
        $rule.AccessControlType -eq 'Allow' -and ($rights -band $rx) -eq $rx) { $found = $true; break }
  }
}
Write-Output $found
`;

function hasAce(path: string, sid: string, folder: boolean, kind: "Deny" | "Allow"): boolean {
  return run("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", inspectScript], {
    ...process.env, TJOMMIE_ACL_PATH: path, TJOMMIE_ACL_SID: sid, TJOMMIE_ACL_FOLDER: folder ? "1" : "", TJOMMIE_ACL_KIND: kind,
  }).toLowerCase() === "true";
}

export const windowsAclDriver: AclDriver = {
  lookupSid: () => {
    try {
      const sid = run("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", lookupScript]);
      return /^S-1-\d+(?:-\d+)+$/.test(sid) ? sid : null;
    } catch { return null; }
  },
  hasDeny: (path, sid, folder) => hasAce(path, sid, folder, "Deny"),
  hasAllow: (path, sid, folder) => hasAce(path, sid, folder, "Allow"),
  sandboxBinReady: (path, sid) => run("powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", inspectSandboxBinScript], {
    ...process.env, TJOMMIE_ACL_PATH: path, TJOMMIE_ACL_SID: sid,
  }).toLowerCase() === "true",
  apply: (command) => { run(command.file, command.args); },
};

function targetIfPresent(root: string, path: string, folder: boolean): Target | null {
  try {
    const base = resolve(root);
    const target = resolve(path);
    const rel = relative(base, target);
    if (!rel || rel === ".." || rel.startsWith(`..${sep}`) || isAbsolute(rel)) throw new Error("protected path is outside the data root");
    let ancestor = base;
    if (lstatSync(ancestor).isSymbolicLink()) throw new Error("data root is a reparse point");
    for (const part of rel.split(sep).slice(0, -1)) {
      ancestor = join(ancestor, part);
      if (lstatSync(ancestor).isSymbolicLink()) throw new Error("protected path has a reparse-point parent");
    }
    const stat = lstatSync(path);
    if (stat.isSymbolicLink() || (folder ? !stat.isDirectory() : !stat.isFile())) throw new Error("unexpected path type");
    return { path, folder };
  } catch (error) {
    if (error && typeof error === "object" && "code" in error && error.code === "ENOENT") return null;
    throw error;
  }
}

function codexEntry(home: string, entry: string, homeEntry = false): string {
  const path = resolve(home, entry);
  const rel = relative(home, path);
  if ((homeEntry && rel) || (!homeEntry && (!rel || rel === ".." || rel.startsWith(`..${sep}`) || isAbsolute(rel)))) {
    throw new Error("codex-main protection entry is outside its expected folder");
  }
  return path;
}

export function protectCodexData(paths: Paths, driver: AclDriver = windowsAclDriver, platform: NodeJS.Platform = process.platform): Protection {
  if (platform !== "win32") {
    console.info("Codex data ACLs: Windows only; no changes made.");
    return { ready: true, reason: null };
  }
  const sid = driver.lookupSid();
  if (!sid) {
    console.error("Codex data ACLs: CodexSandboxUsers group was not found. Own folder is unavailable.");
    return { ready: false, reason: "Set up Codex's sandbox" };
  }
  let ready = true;
  const protect = (path: string, folder: boolean, required: boolean, ace: "deny" | "allow" = "deny"): void => {
    try {
      const target = targetIfPresent(paths.root, path, folder);
      if (!target) {
        if (required) throw new Error("required folder is missing");
        return;
      }
      const hasAce = ace === "deny" ? driver.hasDeny : driver.hasAllow;
      if (!hasAce(target.path, sid, target.folder)) driver.apply(buildAceCommand(target.path, sid, target.folder, ace));
      if (!hasAce(target.path, sid, target.folder)) throw new Error(`${ace} ACE not found after applying`);
    } catch (error) {
      ready = false;
      console.error(`Codex data ACLs: could not protect ${path}: ${String(error)}`);
    }
  };

  for (const target of [
    { path: join(paths.accounts, "claude-main"), folder: true, required: true },
    { path: paths.logs, folder: true, required: true },
    { path: paths.data, folder: true, required: true },
    { path: paths.token, folder: false, required: false },
  ]) protect(target.path, target.folder, target.required);

  try {
    for (const name of readdirSync(paths.data).filter((entry) => /^tjommie\.db(?:$|[-.])/.test(entry))) {
      protect(join(paths.data, name), false, false);
    }
  } catch (error) {
    ready = false;
    console.error(`Codex data ACLs: could not list database files: ${String(error)}`);
  }

  const codexHome = join(paths.accounts, "codex-main");
  try {
    const home = targetIfPresent(paths.root, codexEntry(codexHome, CODEX_MAIN_ACL_RECIPE.home.entry, true), true);
    const sandboxPath = codexEntry(codexHome, CODEX_MAIN_ACL_RECIPE.sandbox.entry);
    const sandbox = targetIfPresent(paths.root, sandboxPath, true);
    const sandboxBinPath = codexEntry(codexHome, CODEX_MAIN_ACL_RECIPE.sandboxBin.entry);
    const sandboxBin = targetIfPresent(paths.root, sandboxBinPath, true);
    if (!home) throw new Error("codex-main folder is missing");
    if (!sandbox) throw new Error(".sandbox folder is missing");
    if (!sandboxBin || !driver.sandboxBinReady(sandboxBin.path, sid)) {
      throw new Error(".sandbox-bin must have a protected ACL with a CodexSandboxUsers ReadAndExecute allow");
    }
    protect(home.path, true, true, CODEX_MAIN_ACL_RECIPE.home.ace);
    protect(sandbox.path, true, true, CODEX_MAIN_ACL_RECIPE.sandbox.ace);
  } catch (error) {
    ready = false;
    console.error(`Codex data ACLs: could not verify ${codexHome}: ${String(error)}`);
  }
  return { ready, reason: ready ? null : "Tjommie could not protect its data" };
}
