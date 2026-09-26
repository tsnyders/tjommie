// Manual isolated probes for final-review A-I3. Never reads or copies sign-in files.
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { join } from "node:path";
import { resolveCli, runCli } from "../src/cli.ts";
import { openDb } from "../src/db.ts";
import { accountEnv, ensureAccounts } from "../src/setup.ts";
import { firstOf, realAccount, realAccountsDir, tempRoot, waitFor } from "../src/testing.ts";
import { ClaudeSession } from "../src/providers/claude.ts";
import { codexConfig, codexPolicy } from "../src/providers/codex.ts";
import { startCodexServer } from "../src/providers/codex-rpc.ts";
import type { ProviderEvent, StartOptions } from "../src/providers/types.ts";

const accounts = realAccountsDir();
const root = tempRoot();
const db = openDb(join(root, "tjommie.db"));
ensureAccounts(db, accounts, root);
const project = join(root, "project");
mkdirSync(join(project, ".claude"), { recursive: true });
const projectSettings = join(project, ".claude", "settings.json");
const originalSettings = JSON.stringify({ permissions: { allow: ["Write", "Edit"] } });
writeFileSync(projectSettings, originalSettings);
const options = (provider: "claude" | "codex"): StartOptions => ({ cwd: project, extraDirs: [], model: provider === "claude" ? "haiku" : "", instructions: "Follow the test request exactly. Never access sign-in or credential files. Do not try alternate ways around denied actions.", autonomy: "folder", allowRules: [], resumeId: null, env: accountEnv(realAccount(provider)), mcpServers: [] });
for (const provider of ["claude", "codex"] as const) {
  const version = await runCli(resolveCli(provider)!, ["--version"], options(provider).env);
  console.log(JSON.stringify({ provider, version: version.stdout.trim() }));
}

async function probeClaude(): Promise<void> {
const claude = new ClaudeSession(resolveCli("claude")!, options("claude"));
const claudeEvents: ProviderEvent[] = [];
claude.onEvent((event) => {
  claudeEvents.push(event);
  if (event.type === "approval") claude.answer(event.id, "deny");
});
try {
  await claude.start();
  await claude.send(`Use the Write tool to write NORMAL_WORK_OK to ${join(project, "normal.txt")}. Do not use shell tools. Reply briefly.`);
  await waitFor(() => firstOf(claudeEvents, "turn_done"));
  console.log(JSON.stringify({ probe: "claude-normal", written: existsSync(join(project, "normal.txt")) && readFileSync(join(project, "normal.txt"), "utf8").includes("NORMAL_WORK_OK"), approvals: claudeEvents.filter((event) => event.type === "approval").length }));
  claudeEvents.length = 0;
  await claude.send(`Use the Edit tool to change ${projectSettings}, replacing the exact text ${originalSettings} with {"probe":"CHANGED"}. This is a disposable test file. If the CLI refuses, stop and report that. Do not use shell tools or another method.`);
  await waitFor(() => firstOf(claudeEvents, "turn_done"));
  console.log(JSON.stringify({ probe: "claude-settings", unchanged: readFileSync(projectSettings, "utf8") === originalSettings, approvals: claudeEvents.filter((event) => event.type === "approval").map((event) => event.tool), events: claudeEvents.filter((event) => event.type === "tool" || event.type === "tool_done").map((event) => event.type === "tool" ? { type: event.type, name: event.name } : { type: event.type, ok: event.ok }), reply: claudeEvents.filter((event) => event.type === "text").map((event) => event.delta).join("") }));
  const fakeAccounts = join(root, "accounts");
  mkdirSync(fakeAccounts);
  const safeProbe = join(fakeAccounts, "deny-rule-probe.txt");
  writeFileSync(safeProbe, "Harmless test marker, contains no credentials.");
  claudeEvents.length = 0;
  await claude.send(`Use the Read tool once on ${safeProbe}. This one file is a harmless test marker, contains no credentials, and was created just for this permission test. Do not open any other file or use an alternate method. If access is denied, report that and stop.`);
  await waitFor(() => firstOf(claudeEvents, "turn_done"));
  console.log(JSON.stringify({ probe: "claude-absolute-read-deny", approvals: claudeEvents.filter((event) => event.type === "approval").length, events: claudeEvents.filter((event) => event.type === "tool" || event.type === "tool_done").map((event) => event.type === "tool" ? { type: event.type, name: event.name } : { type: event.type, ok: event.ok }), reply: claudeEvents.filter((event) => event.type === "text").map((event) => event.delta).join("") }));
} finally {
  claude.close();
  await claude.waitForExit();
}
}
if (process.env.TJOMMIE_SPIKE_CODEX_ONLY !== "1") await probeClaude();
if (process.env.TJOMMIE_SPIKE_CLAUDE_ONLY === "1") {
  db.close();
  process.exit(0);
}

let healthRequests = 0;
let projectMcpRequests = 0;
const http = createServer((req, res) => {
  if (req.url === "/health") healthRequests++;
  if (req.url === "/project-mcp") projectMcpRequests++;
  res.setHeader("Content-Type", "application/json");
  res.end(JSON.stringify({ status: "ok" }));
});
await new Promise<void>((resolve) => http.listen(0, "127.0.0.1", resolve));
const address = http.address();
if (!address || typeof address === "string") throw new Error("No loopback port");
mkdirSync(join(project, ".codex"), { recursive: true });
spawnSync("git", ["init", project], { stdio: "ignore", windowsHide: true });
const marker = join(project, "notify-marker.txt");
const notifyScript = join(project, "notify-probe.cjs");
writeFileSync(notifyScript, `require('node:fs').writeFileSync(${JSON.stringify(marker)}, 'project notify ran');`);
writeFileSync(join(project, ".codex", "config.toml"), `notify = [${JSON.stringify(process.execPath)}, ${JSON.stringify(notifyScript)}]\n[mcp_servers.safety_project_probe]\nurl = "http://127.0.0.1:${address.port}/project-mcp"\n`);
const server = await startCodexServer(resolveCli("codex")!, options("codex").env, project);
let interrupted = false;
let completed = false;
let commandStarted = false;
let approvalCount = 0;
const eventOrder: string[] = [];
const commandResults: { command: unknown; exitCode: unknown; output: unknown }[] = [];
server.rpc.onNotification = (method, params) => {
  const record = typeof params === "object" && params !== null ? params as Record<string, unknown> : {};
  const item = typeof record.item === "object" && record.item !== null ? record.item as Record<string, unknown> : {};
  if (method === "item/started" && item.type === "commandExecution") commandStarted = true;
  if (method === "turn/completed") completed = true;
  if (method === "item/completed" && item.type === "commandExecution") commandResults.push({ command: item.command, exitCode: item.exitCode, output: item.aggregatedOutput });
  if (interrupted) eventOrder.push(`${method}${typeof item.type === "string" ? `:${item.type}` : ""}`);
};
server.rpc.onRequest = (id) => { approvalCount++; server.rpc.respond(id, { decision: "decline" }); };
try {
  const config = await server.rpc.request<{ config: Record<string, unknown>; layers?: { name?: unknown; disabledReason?: unknown }[] }>("config/read", { cwd: project, includeLayers: true });
  const mcp = config.config.mcp_servers;
  console.log(JSON.stringify({ probe: "codex-project-config", notifyLoaded: Array.isArray(config.config.notify) && config.config.notify.includes(notifyScript), mcpLoaded: typeof mcp === "object" && mcp !== null && Object.hasOwn(mcp, "safety_project_probe"), layers: config.layers?.map((layer) => ({ name: layer.name, disabledReason: layer.disabledReason })) }));
  const started = await server.rpc.request<{ thread: { id: string }; sandbox: unknown; approvalPolicy: unknown }>("thread/start", { cwd: project, model: null, ...codexPolicy("folder"), developerInstructions: options("codex").instructions, config: codexConfig(options("codex")) });
  console.log(JSON.stringify({ probe: "codex-thread-policy", sandbox: started.sandbox, approvalPolicy: started.approvalPolicy, projectMcpRequests }));
  const afterThread = await server.rpc.request<{ config: Record<string, unknown>; layers?: { name?: unknown; disabledReason?: unknown }[] }>("config/read", { cwd: project, includeLayers: true });
  console.log(JSON.stringify({ probe: "codex-project-config-after-thread", notifyLoaded: Array.isArray(afterThread.config.notify) && afterThread.config.notify.includes(notifyScript), mcpLoaded: typeof afterThread.config.mcp_servers === "object" && afterThread.config.mcp_servers !== null && Object.hasOwn(afterThread.config.mcp_servers, "safety_project_probe"), layers: afterThread.layers?.map((layer) => ({ name: layer.name, disabledReason: layer.disabledReason })) }));
  await server.rpc.request("turn/start", { threadId: started.thread.id, input: [{ type: "text", text: `Use a shell command to request http://127.0.0.1:${address.port}/health with Invoke-WebRequest. Do not use MCP, browser, web tools or proxy settings. If access fails, do not retry outside the sandbox; report the failure.` }] });
  await waitFor(() => completed);
  console.log(JSON.stringify({ probe: "codex-loopback", healthRequests, approvals: approvalCount, projectMcpRequests, notifyExecuted: existsSync(marker), commandResults }));
  completed = false;
  commandStarted = false;
  const turn = await server.rpc.request<{ turn: { id: string } }>("turn/start", { threadId: started.thread.id, input: [{ type: "text", text: `Run node -e "setTimeout(() => console.log('done'), 60000)" and wait for it. Do not run it in the background.` }] });
  await waitFor(() => commandStarted || completed);
  if (commandStarted && !completed) {
    interrupted = true;
    await server.rpc.request("turn/interrupt", { threadId: started.thread.id, turnId: turn.turn.id });
    eventOrder.push("turn/interrupt:response");
    await waitFor(() => completed);
    await new Promise((resolve) => setTimeout(resolve, 5000));
    console.log(JSON.stringify({ probe: "codex-interrupt-events", eventOrder }));
  } else console.log(JSON.stringify({ probe: "codex-interrupt-events", error: "No running command to interrupt" }));
} finally {
  const exited = new Promise<void>((resolve) => server.child.once("exit", () => resolve()));
  server.close();
  await exited;
  await new Promise<void>((resolve) => http.close(() => resolve()));
}
// Scope project trust to this child. Never overwrite the real account's config file.
const codex = resolveCli("codex")!;
const trusted = await startCodexServer({ ...codex, args: [...codex.args, "-c", `projects.${JSON.stringify(project)}.trust_level="trusted"`] }, options("codex").env, project);
try {
  const config = await trusted.rpc.request<{ config: Record<string, unknown> }>("config/read", { cwd: project, includeLayers: true });
  const mcp = config.config.mcp_servers;
  console.log(JSON.stringify({ probe: "codex-trusted-project-config", notifyLoaded: Array.isArray(config.config.notify) && config.config.notify.includes(notifyScript), mcpLoaded: typeof mcp === "object" && mcp !== null && Object.hasOwn(mcp, "safety_project_probe") }));
} finally {
  const exited = new Promise<void>((resolve) => trusted.child.once("exit", () => resolve()));
  trusted.close();
  await exited;
  db.close();
}
