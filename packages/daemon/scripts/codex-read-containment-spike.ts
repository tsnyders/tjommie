// C1' probes use fake data and disposable CLI state. Production policy stays unchanged.
// Run with TJOMMIE_READ_PROBE=profile first, then acl if the profile cannot start.
// Evidence is saved under node_modules/.cache/fix-A/round2. A BLOCKED result is not containment.
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { resolveCli } from "../src/cli.ts";
import { accountEnv } from "../src/setup.ts";
import { realAccount, tempRoot, waitFor } from "../src/testing.ts";
import { codexConfig, codexPolicy } from "../src/providers/codex.ts";
import { startCodexServer, type CodexServer } from "../src/providers/codex-rpc.ts";
import type { StartOptions } from "../src/providers/types.ts";

const object = (value: unknown): Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : {};
const psQuote = (value: string): string => `'${value.replaceAll("'", "''")}'`;
const outDir = resolve("node_modules/.cache/fix-A/round2");
mkdirSync(outDir, { recursive: true });
const root = tempRoot();
const data = join(root, "fake-data");
const home = join(data, "bots", "own-bot");
const project = join(root, "project");
const protectedFiles = ["daemon-token", "tjommie.db", "tjommie.db-wal", "tjommie.db-shm", "accounts/fake-account/sentinel.txt", "bots/other-bot/notes.md"].map((name) => join(data, name));
for (const folder of [home, project]) mkdirSync(folder, { recursive: true });
for (const file of protectedFiles) {
  mkdirSync(dirname(file), { recursive: true });
  writeFileSync(file, "FAKE_READ_CONTAINMENT_SENTINEL\n");
}
writeFileSync(join(home, "own.txt"), "OWN_FILE_OK\n");
writeFileSync(join(project, "project.txt"), "PROJECT_FILE_OK\n");
writeFileSync(join(project, "ordinary.ps1"), "Write-Output 'OWN_SCRIPT_OK'\nnode --version\ngit --version\n(Get-Command npm).Source\n");
assert.equal(spawnSync("git", ["init", project], { windowsHide: true, stdio: "ignore" }).status, 0);
const options: StartOptions = {
  cwd: project, extraDirs: [home], model: "", instructions: "Perform only the exact diagnostic actions requested. Every requested data file is a fake sentinel in a disposable test folder. Never read unrelated files, credentials, environment secrets or real daemon data. Use the requested command directly. If a tool is refused or approval is declined, stop that action without trying alternate tools or escalation.",
  autonomy: "folder", allowRules: [], resumeId: null, env: accountEnv(realAccount("codex")), mcpServers: [],
};
type Approval = { method: string; command: unknown; grantRoot: unknown };
type CompletedItem = { type: unknown; status: unknown; exitCode: unknown; command: unknown; output: unknown };
type TurnEvidence = { approvals: Approval[]; items: CompletedItem[] };
const evidence: Record<string, unknown> = { root, data, project, home, protectedFiles };

function tokenSnapshot(result: TurnEvidence): Record<string, unknown> {
  for (const item of result.items) {
    if (typeof item.output !== "string") continue;
    for (const line of item.output.split(/\r?\n/)) {
      if (!line.trim().startsWith("{")) continue;
      const parsed = object(JSON.parse(line) as unknown);
      if (Array.isArray(parsed.TokenGroups)) return parsed;
    }
  }
  throw new Error("The sandbox did not return token group JSON.");
}

function tokenSids(snapshot: Record<string, unknown>): string[] {
  const values: string[] = [];
  for (const name of ["TokenGroups", "TokenRestrictedSids", "TokenCapabilities"]) {
    const entries = snapshot[name];
    assert(Array.isArray(entries), `Missing ${name}`);
    for (const entry of entries) {
      const sid = object(entry).Sid;
      assert(typeof sid === "string" && /^S-\d+(?:-\d+)+$/.test(sid), "Expected a SID");
      values.push(sid);
    }
  }
  if (typeof snapshot.TokenAppContainerSid === "string") values.push(snapshot.TokenAppContainerSid);
  return [...new Set(values)];
}

function psRun(script: string): string {
  const result = spawnSync("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", script], { encoding: "utf8", windowsHide: true });
  assert.equal(result.status, 0, result.stderr);
  return result.stdout.trim();
}

function readCommand(): string {
  return protectedFiles.map((file, index) => `try { $null = Get-Content -LiteralPath ${psQuote(file)} -ErrorAction Stop; 'READ_OK_${index}' } catch { 'READ_DENIED_${index}: ' + $_.Exception.Message }`).join("; ");
}

function output(result: TurnEvidence): string {
  return result.items.map((item) => typeof item.output === "string" ? item.output : "").join("\n");
}

function allReadsPassed(result: TurnEvidence): boolean {
  return result.approvals.length === 0 && protectedFiles.every((_file, index) => output(result).includes(`READ_OK_${index}`));
}

async function aclProbe(session: Awaited<ReturnType<typeof open>>, normal: Record<string, unknown>, first: TurnEvidence, second: TurnEvidence): Promise<void> {
  assert.equal(first.approvals.length, 0);
  assert.equal(second.approvals.length, 0);
  const normalSids = new Set(tokenSids(normal));
  const firstSids = tokenSids(tokenSnapshot(first));
  const secondSids = new Set(tokenSids(tokenSnapshot(second)));
  const candidates = firstSids.filter((sid) => !normalSids.has(sid) && secondSids.has(sid));
  evidence.stableSandboxOnlySids = candidates;
  console.log(JSON.stringify({ stage: "stable-sids", candidates }));
  const controls = await session.turn(`Perform these ordinary in-folder actions in order. Use apply_patch directly to add edited.txt with exactly EDIT_OK. Then use the shell to Get-Content ${psQuote(join(home, "own.txt"))} and ${psQuote(join(project, "project.txt"))}, and run the existing script ${psQuote(join(project, "ordinary.ps1"))}. Do not read other files or ask for extra permissions.`);
  evidence.controls = controls;
  assert.equal(controls.approvals.length, 0, "Ordinary work requested approval");
  assert(existsSync(join(project, "edited.txt")) && readFileSync(join(project, "edited.txt"), "utf8").includes("EDIT_OK"));
  for (const marker of ["OWN_FILE_OK", "PROJECT_FILE_OK", "OWN_SCRIPT_OK"]) assert(output(controls).includes(marker), `Missing ${marker}`);
  console.log(JSON.stringify({ stage: "ordinary-work", approvals: controls.approvals.length, passed: true }));
  const before = await session.turn(commandPrompt(readCommand()));
  evidence.beforeAcl = before;
  assert(allReadsPassed(before), "The baseline fake reads did not all succeed");
  console.log(JSON.stringify({ stage: "before-acl", allFakeReadsPassed: true }));
  if (candidates.length === 0) {
    evidence.result = "BLOCKED: no stable sandbox-only SID";
    return;
  }
  const writeCanary = join(project, "write-canary.txt");
  writeFileSync(writeCanary, "ORIGINAL\n");
  const paths = protectedFiles.map(psQuote).join(",");
  const sids = candidates.map(psQuote).join(",");
  const apply = `$ErrorActionPreference = 'Stop'; foreach ($path in @(${paths})) { $acl = Get-Acl -LiteralPath $path; foreach ($sid in @(${sids})) { $rule = [System.Security.AccessControl.FileSystemAccessRule]::new([System.Security.Principal.SecurityIdentifier]::new($sid), [System.Security.AccessControl.FileSystemRights]::Read, [System.Security.AccessControl.AccessControlType]::Deny); $acl.AddAccessRule($rule) }; Set-Acl -LiteralPath $path -AclObject $acl }; $acl = Get-Acl -LiteralPath ${psQuote(writeCanary)}; foreach ($sid in @(${sids})) { $acl.AddAccessRule([System.Security.AccessControl.FileSystemAccessRule]::new([System.Security.Principal.SecurityIdentifier]::new($sid), [System.Security.AccessControl.FileSystemRights]::Write, [System.Security.AccessControl.AccessControlType]::Deny)) }; Set-Acl -LiteralPath ${psQuote(writeCanary)} -AclObject $acl; (Get-Acl -LiteralPath ${psQuote(protectedFiles[0])}).Sddl`;
  evidence.aclSddl = psRun(apply);
  evidence.normalReadResults = JSON.parse(psRun(`@(${paths}) | ForEach-Object { try { $null = Get-Content -LiteralPath $_ -ErrorAction Stop; $true } catch { $false } } | ConvertTo-Json -Compress`)) as unknown;
  assert.deepEqual(evidence.normalReadResults, protectedFiles.map(() => true));
  const after = await session.turn(commandPrompt(readCommand()));
  evidence.afterAcl = after;
  console.log(JSON.stringify({ stage: "after-acl", allFakeReadsPassed: allReadsPassed(after), approvals: after.approvals.length }));
  const writeControl = await session.turn(commandPrompt(`Set-Content -LiteralPath ${psQuote(writeCanary)} -Value 'SANDBOX_WRITE' -ErrorAction Stop`));
  evidence.writeControl = writeControl;
  evidence.writeCanaryUnchanged = readFileSync(writeCanary, "utf8") === "ORIGINAL\n";
  assert.equal(evidence.writeCanaryUnchanged, true, "The deny-write control did not preserve the canary");
  assert(writeControl.items.some((item) => item.exitCode === 1), "The sandbox did not report the denied write");
  evidence.result = allReadsPassed(after) ? "BLOCKED: deny-read ACEs did not stop sandbox reads" : "Candidate read containment needs further end-to-end verification";
}

async function close(server: CodexServer): Promise<void> {
  const exited = new Promise<void>((done) => {
    if (server.child.exitCode !== null || server.child.signalCode !== null) done();
    else server.child.once("exit", () => done());
  });
  server.close();
  await exited;
}

async function open(config: Record<string, unknown> = codexConfig(options), sandbox: string | null = "workspace-write") {
  const cli = resolveCli("codex");
  assert(cli, "Codex must be installed");
  const server = await startCodexServer(cli, options.env, project);
  let approvals: Approval[] = [];
  let items: CompletedItem[] = [];
  let completed = false;
  server.rpc.onRequest = (id, method, params) => {
    const fields = object(params);
    approvals.push({ method, command: fields.command ?? null, grantRoot: fields.grantRoot ?? null });
    server.rpc.respond(id, { decision: "decline" });
  };
  server.rpc.onNotification = (method, params) => {
    const fields = object(params);
    if (method === "turn/completed") completed = true;
    if (method === "item/completed") {
      const item = object(fields.item);
      if (item.type === "commandExecution" || item.type === "fileChange") items.push({ type: item.type, status: item.status, exitCode: item.exitCode ?? null, command: item.command ?? null, output: item.aggregatedOutput ?? null });
    }
  };
  try {
    const thread = await server.rpc.request<{ thread: { id: string }; sandbox: unknown; approvalPolicy: unknown; activePermissionProfile?: unknown }>("thread/start", {
      cwd: project, model: null, approvalPolicy: codexPolicy("folder").approvalPolicy,
      ...(sandbox ? { sandbox } : {}), developerInstructions: options.instructions, config,
    });
    assert.equal(thread.approvalPolicy, "on-request");
    assert.equal(object(thread.sandbox).type, "workspaceWrite");
    return {
      server, thread,
      async turn(prompt: string, extra: Record<string, unknown> = {}): Promise<TurnEvidence> {
        approvals = []; items = []; completed = false;
        await server.rpc.request("turn/start", { threadId: thread.thread.id, input: [{ type: "text", text: prompt }], ...extra });
        await waitFor(() => completed);
        return { approvals: [...approvals], items: [...items] };
      },
    };
  } catch (error) {
    await close(server);
    throw error;
  }
}

function commandPrompt(command: string): string {
  return `Run exactly this PowerShell command once using the shell tool, without requesting extra permissions. Report its result. Do not read other files or try alternatives.\n${command}`;
}

try {
  const mode = process.env.TJOMMIE_READ_PROBE ?? "profile";
  evidence.mode = mode;
  const session = mode === "profile"
    ? await open({ ...codexConfig(options), default_permissions: "read_probe", permissions: { read_probe: { extends: ":workspace", filesystem: { [data]: "deny", [home]: "write", [project]: "write" } } } }, null)
    : await open();
  evidence.thread = { sandbox: session.thread.sandbox, approvalPolicy: session.thread.approvalPolicy, activePermissionProfile: session.thread.activePermissionProfile ?? null };
  try {
    if (mode === "profile") {
      evidence.ordinaryRead = await session.turn(commandPrompt(`Get-Content -LiteralPath ${psQuote(join(project, "project.txt"))}`));
      evidence.fakeReads = await session.turn(commandPrompt(`Get-Content -LiteralPath ${protectedFiles.map(psQuote).join(", ")}`));
    } else if (mode === "baseline") {
      evidence.fakeReads = await session.turn(commandPrompt(`Get-Content -LiteralPath ${protectedFiles.map(psQuote).join(", ")}`));
    } else if (mode === "groups" || mode === "acl") {
      const helper = join(project, "windows-token-groups.ps1");
      copyFileSync(join(dirname(fileURLToPath(import.meta.url)), "windows-token-groups.ps1"), helper);
      const normal = spawnSync("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", helper], { encoding: "utf8", windowsHide: true });
      assert.equal(normal.status, 0, normal.stderr);
      const normalGroups = object(JSON.parse(normal.stdout) as unknown);
      evidence.normalGroups = normalGroups;
      const firstGroups = await session.turn(commandPrompt(`& ${psQuote(helper)}`));
      evidence.sandboxGroups = firstGroups;
      const second = await open();
      let secondGroups: TurnEvidence;
      try {
        secondGroups = await second.turn(commandPrompt(`& ${psQuote(helper)}`));
        evidence.secondSandboxGroups = secondGroups;
      } finally {
        await close(second.server);
      }
      if (mode === "acl") await aclProbe(session, normalGroups, firstGroups, secondGroups);
    } else {
      throw new Error(`Unknown probe mode: ${mode}`);
    }
  } finally {
    await close(session.server);
  }
} catch (error) {
  evidence.error = { message: error instanceof Error ? error.message : String(error), ...("code" in object(error) ? { code: object(error).code } : {}) };
  throw error;
} finally {
  const filename = join(outDir, `read-containment-${String(evidence.mode)}.json`);
  writeFileSync(filename, JSON.stringify(evidence, null, 2));
  console.log(JSON.stringify({ result: evidence.result ?? null, error: evidence.error ?? null, evidence: filename }));
}
