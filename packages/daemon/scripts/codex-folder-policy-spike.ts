// C1 candidate probe. Uses disposable files and an explicit policy override before changing production.
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { codexConfig } from "../src/providers/codex.ts";
import { startCodexServer } from "../src/providers/codex-rpc.ts";
import type { StartOptions } from "../src/providers/types.ts";
import { resolveCli } from "../src/cli.ts";
import { accountEnv } from "../src/setup.ts";
import { realAccount, tempRoot, waitFor } from "../src/testing.ts";

const root = tempRoot();
const project = join(root, "project");
mkdirSync(project);
const options: StartOptions = {
  cwd: project, extraDirs: [], model: "", instructions: "Follow only the exact test action requested. Never read sign-in files, tokens, environment secrets or unrelated files. Do not bypass a refusal or approval denial. Use apply_patch for edits.",
  autonomy: "folder", allowRules: [], resumeId: null, env: accountEnv(realAccount("codex")), mcpServers: [],
};
const cmd = resolveCli("codex");
if (!cmd) throw new Error("Codex is not installed.");
const server = await startCodexServer(cmd, options.env, project);
type Approval = { method: string; command: unknown; rule: unknown; grantRoot: unknown };
let approvals: Approval[] = [];
let completed = false;
let outputs: { type: unknown; status: unknown; exitCode: unknown; output: unknown }[] = [];
const evidence: Record<string, unknown> = { root };
const object = (value: unknown): Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : {};
server.rpc.onRequest = (id, method, params) => {
  const p = object(params);
  approvals.push({ method, command: p.command, rule: p.proposedExecpolicyAmendment, grantRoot: p.grantRoot });
  server.rpc.respond(id, { decision: "decline" });
};
server.rpc.onNotification = (method, params) => {
  const p = object(params);
  if (method === "turn/completed") completed = true;
  if (method === "item/completed") {
    const item = object(p.item);
    if (item.type === "commandExecution" || item.type === "fileChange") outputs.push({ type: item.type, status: item.status, exitCode: item.exitCode, output: item.aggregatedOutput });
  }
};
try {
  const thread = await server.rpc.request<{ thread: { id: string }; approvalPolicy: unknown; sandbox: unknown }>("thread/start", {
    cwd: project, model: null, developerInstructions: options.instructions,
    approvalPolicy: "untrusted", sandbox: "workspace-write", config: codexConfig(options),
  });
  evidence.policy = { approvalPolicy: thread.approvalPolicy, sandbox: thread.sandbox };
  const turn = async (prompt: string) => {
    approvals = []; outputs = []; completed = false;
    await server.rpc.request("turn/start", { threadId: thread.thread.id, input: [{ type: "text", text: prompt }] });
    await waitFor(() => completed);
    return { approvals: [...approvals], outputs: [...outputs] };
  };
  evidence.edit = { ...await turn("Use apply_patch to add ordinary.txt in the current project with exactly FOLDER_EDIT_OK. Use no shell command. Then stop."), written: existsSync(join(project, "ordinary.txt")) && readFileSync(join(project, "ordinary.txt"), "utf8").includes("FOLDER_EDIT_OK") };
  console.log(JSON.stringify({ probe: "C1-edit", result: evidence.edit }));
  if (approvals.length || !existsSync(join(project, "ordinary.txt"))) throw new Error("BLOCKED C1: ordinary in-folder edit did not proceed without a card.");
  console.log("The ordinary edit passed. This probe alone does not verify the remaining C1 requirements.");
} finally {
  const outDir = resolve("node_modules/.cache/fix-A/round1");
  mkdirSync(outDir, { recursive: true });
  writeFileSync(join(outDir, "codex-policy-candidate.json"), JSON.stringify(evidence, null, 2));
  const exited = new Promise<void>((done) => { if (server.child.exitCode !== null || server.child.signalCode !== null) done(); else server.child.once("exit", () => done()); });
  server.close();
  await exited;
}
