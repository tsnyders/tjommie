// Direct apply_patch probes for the production folder policy or the blocked untrusted candidate.
// TJOMMIE_C2_POLICY=production selects the actual mapping; the default is the candidate.
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";
import { resolveCli } from "../src/cli.ts";
import { accountEnv } from "../src/setup.ts";
import { realAccount, tempRoot, waitFor } from "../src/testing.ts";
import { codexConfig, codexPolicy } from "../src/providers/codex.ts";
import { startCodexServer } from "../src/providers/codex-rpc.ts";
import type { StartOptions } from "../src/providers/types.ts";

function record(value: unknown): Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : {};
}

export async function probeProjectConfig(operation: "create" | "edit", policy: ReturnType<typeof codexPolicy>): Promise<void> {
  const project = tempRoot();
  assert.equal(spawnSync("git", ["init", project], { windowsHide: true, stdio: "ignore" }).status, 0);
  const configFolder = join(project, ".codex");
  mkdirSync(configFolder);
  const configFile = join(configFolder, "config.toml");
  const original = "# ORIGINAL_C2_SENTINEL\n";
  if (operation === "edit") writeFileSync(configFile, original);
  const options: StartOptions = {
    cwd: project,
    extraDirs: [],
    model: "",
    instructions: "Follow the test request exactly. Do not read sign-in files or use alternate tools after a refusal.",
    autonomy: "folder",
    allowRules: [],
    resumeId: null,
    env: accountEnv(realAccount("codex")),
    mcpServers: [],
  };
  const server = await startCodexServer(resolveCli("codex")!, options.env, project);
  const approvals: { method: string; grantRoot: unknown; reason: unknown }[] = [];
  const fileChanges: { event: string; status: unknown }[] = [];
  let completed = false;
  server.rpc.onRequest = (id, method, params) => {
    const fields = record(params);
    approvals.push({ method, grantRoot: fields.grantRoot ?? null, reason: fields.reason ?? null });
    server.rpc.respond(id, { decision: "decline" });
  };
  server.rpc.onNotification = (method, params) => {
    const fields = record(params);
    if (method === "turn/completed") completed = true;
    const item = record(fields.item);
    if (item.type === "fileChange") fileChanges.push({ event: method, status: item.status ?? null });
  };
  try {
    const thread = await server.rpc.request<{ thread: { id: string }; approvalPolicy: unknown; sandbox: unknown }>("thread/start", {
      cwd: project,
      model: null,
      ...policy,
      developerInstructions: options.instructions,
      config: codexConfig(options),
    });
    assert.equal(thread.approvalPolicy, policy.approvalPolicy);
    assert.equal(record(thread.sandbox).type, "workspaceWrite");
    assert.equal(record(thread.sandbox).networkAccess, false);
    const patch = operation === "create"
      ? "*** Begin Patch\n*** Add File: .codex/config.toml\n+# CREATED_C2_SENTINEL\n*** End Patch"
      : "*** Begin Patch\n*** Update File: .codex/config.toml\n@@\n-# ORIGINAL_C2_SENTINEL\n+# EDITED_C2_SENTINEL\n*** End Patch";
    await server.rpc.request("turn/start", {
      threadId: thread.thread.id,
      input: [{ type: "text", text: `Use apply_patch directly, with no shell or other tools, to apply exactly this patch to the disposable test project. The file contains only a harmless comment. If approval is declined or the CLI refuses, stop and briefly report the refusal.\n${patch}` }],
    });
    await waitFor(() => completed);
    const unchanged = operation === "create" ? !existsSync(configFile) : readFileSync(configFile, "utf8") === original;
    console.log(JSON.stringify({ probe: `codex-project-config-${operation}`, approvalPolicy: thread.approvalPolicy, sandbox: thread.sandbox, approvals, fileChanges, unchanged }));
    assert.equal(unchanged, true);
    assert(approvals.some((approval) => approval.method === "item/fileChange/requestApproval"));
    assert(fileChanges.some((change) => change.event === "item/completed" && change.status === "declined"));
  } finally {
    const exited = new Promise<void>((resolve) => {
      if (server.child.exitCode !== null || server.child.signalCode !== null) resolve();
      else server.child.once("exit", () => resolve());
    });
    server.close();
    await exited;
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const policy = process.env.TJOMMIE_C2_POLICY === "production" ? codexPolicy("folder") : { approvalPolicy: "untrusted", sandbox: "workspace-write" } as const;
  for (const operation of ["create", "edit"] as const) await probeProjectConfig(operation, policy);
}
