// Runs real acceptance checks against an explicitly supplied installed executable.
// It uses disposable data/browser folders and the existing Tjommie provider accounts.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { DatabaseSync } from "node:sqlite";
import { claudeBot, closeRealApp, launchRealApp, openChat } from "../../../packages/desktop/src/real-app.ts";
import { packagedExecutable, provenanceFile, verifyPackagedProvenance } from "../../../packages/desktop/scripts/packaged-provenance.ts";

const executable = process.argv[2];
assert(executable && isAbsolute(executable) && existsSync(executable), "Supply the installed Tjommie.exe absolute path.");
const directory = dirname(fileURLToPath(import.meta.url));
const repo = resolve(directory, "../../..");
verifyPackagedProvenance(repo);
const hash = (path) => createHash("sha256").update(readFileSync(path)).digest("hex");
const executableSha256 = hash(executable);
assert.equal(executableSha256, hash(join(repo, packagedExecutable)), "Installed executable must match the verified package.");
const provenance = JSON.parse(readFileSync(join(repo, provenanceFile), "utf8"));
const unpackedPrefix = "dist-desktop/installer/win-unpacked/";
for (const [path, expected] of provenance.artifacts) {
  if (path.startsWith(unpackedPrefix)) assert.equal(hash(join(dirname(executable), path.slice(unpackedPrefix.length))), expected, `Installed payload differs: ${path}`);
}
const sourceCommit = execFileSync("git", ["rev-parse", "HEAD"], { cwd: repo, encoding: "utf8" }).trim();
const sourceWorktreeDirty = execFileSync("git", ["status", "--porcelain"], { cwd: repo, encoding: "utf8", windowsHide: true }).trim().length > 0;
const results = { startedAt: new Date().toISOString(), sourceCommit, sourceWorktreeDirty, inputsSha256: provenance.inputsSha256, executable, executableSha256, checks: {}, evidence: {}, finishedAt: null };
let app;
let observedDb;
let current;
async function waitUntil(check, timeoutMs = 60_000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check();
    if (value !== undefined && value !== null && value !== false) return value;
    if (Date.now() > deadline) throw new Error(`Timed out after ${timeoutMs} ms`);
    await new Promise((done) => setTimeout(done, 500));
  }
}
async function check(name, action) {
  current = name;
  console.log(`CHECK: ${name}`);
  await action();
  results.checks[name] = "Pass";
  console.log(`PASS: ${name}`);
}
try {
  app = await launchRealApp(executable);
  const { client, page, electronApp } = app;
  observedDb = new DatabaseSync(join(app.root, "data", "tjommie.db"), { readOnly: true });
  const workFor = (message) => observedDb.prepare("SELECT status, started_at, ended_at FROM work_items WHERE message_id = ?").get(message.id);
  const idle = (bot) => waitUntil(async () => (await client.call("bots.list", {})).find((entry) => entry.id === bot.id && entry.state === "idle"), 180_000);
  const said = (bot, marker) => waitUntil(async () => {
    const { messages } = await client.call("chat.history", { botId: bot.id });
    return messages.find((message) => message.author === bot.id && message.kind === "text" && message.status === "done" && message.body.includes(marker));
  }, 240_000);
  const send = (bot, text) => client.call("chat.send", { botId: bot.id, text });
  const approval = (bot, kind) => waitUntil(async () => client.events.find((event) => event.event === "approval.added" && event.data.botId === bot.id && event.data.kind === kind)?.data, 180_000);
  const stopMarker = `INSTALLED-STOP-${randomUUID()}`;
  const stopCommand = `node -e "console.log('${stopMarker}');setTimeout(() => console.log('done'),60000)"`;
  const findCommandPid = () => {
    // The fresh marker identifies only this exact synthetic command. Return numeric IDs only.
    const script = `Get-CimInstance Win32_Process -Filter "Name='node.exe'" | Where-Object { $_.CommandLine -like '*${stopMarker}*' } | Select-Object -ExpandProperty ProcessId | ConvertTo-Json -Compress`;
    const powershellDir = join(process.env.SystemRoot ?? "C:\\Windows", "System32", "WindowsPowerShell", "v1.0");
    const output = execFileSync(join(powershellDir, "powershell.exe"), ["-NoProfile", "-NonInteractive", "-Command", script], { encoding: "utf8", windowsHide: true, env: { ...process.env, PSModulePath: join(powershellDir, "Modules") } }).trim();
    if (!output) return null;
    const ids = JSON.parse(output);
    assert(!Array.isArray(ids), "Expected exactly one synthetic timer process.");
    assert(Number.isSafeInteger(ids) && ids > 0);
    return ids;
  };
  const allowedTimers = new Set([
    stopCommand,
    `node -e "setTimeout(() => console.log('first'), 8000)"`,
    `node -e "setTimeout(() => console.log('done'), 60000)"`,
  ]);
  const toolsStarted = (bot, from) => {
    const answered = new Set();
    return waitUntil(async () => {
      for (const event of client.events.slice(from)) {
        if (event.event !== "approval.added" || event.data.botId !== bot.id || answered.has(event.data.id)) continue;
        // Codex Always ask is intentional for accounts outside this disposable data root.
        // Only approve this harness's foreground node timer commands, never arbitrary commands.
        assert.equal(event.data.kind, "tool");
        assert.equal(event.data.tool, "Shell");
        assert(allowedTimers.has(event.data.detail), "Only the exact synthetic timer commands may be approved.");
        answered.add(event.data.id);
        await client.call("approvals.respond", { approvalId: event.data.id, decision: "once" });
      }
      if (answered.size === 0) return false;
      return client.events.slice(from).find((event) => (event.event === "message.added" || event.event === "message.updated") && event.data.conversationId === bot.id && event.data.kind === "activity" && event.data.data?.items?.some((item) => item.status === "running"));
    }, 180_000);
  };

  await check("installed providers ready", async () => {
    const setup = await client.call("setup.status", {});
    assert(setup.every((provider) => provider.installed && provider.signedIn));
    assert.equal(setup.find((provider) => provider.provider === "codex")?.sandbox, "ready");
  });
  const models = await client.call("models.list", { provider: "codex" });
  const model = models.find((entry) => entry.isDefault);
  assert(model, "Codex must report a default model.");
  // Like daemon.real.test.ts: Own folder correctly stays gated when the signed-in
  // account home is outside the disposable data root whose ACLs this daemon manages.
  const writer = await client.call("bots.create", { name: "Installed Writer", color: "blue", shape: "pill", provider: "codex", model: model.id, instructions: "Answer briefly and follow the requested test marker exactly.", autonomy: "ask", browser: false });
  assert.equal(writer.state, "idle");
  const researcher = await claudeBot(client, "Installed Researcher", { browser: true, canMessage: [writer.id] });

  await check("installed cross-provider exchange", async () => {
    await send(researcher, "Use message_bot to ask Installed Writer to reply with exactly INSTALLED-CODEX-OK. When its reply arrives, say INSTALLED-CROSS-OK.");
    await said(researcher, "INSTALLED-CROSS-OK");
    await waitUntil(async () => (await client.call("room.history", {})).messages.find((message) => message.author === writer.id && message.body.includes("INSTALLED-CODEX-OK")), 240_000);
    await idle(researcher);
    await idle(writer);
  });
  await check("installed Codex steering", async () => {
    const from = client.events.length;
    const original = await send(writer, "Run this shell command and wait for it: node -e \"setTimeout(() => console.log('first'), 8000)\". Then tell me what it printed.");
    await toolsStarted(writer, from);
    const steered = await send(writer, "Also end your answer with INSTALLED-STEER-OK.");
    await said(writer, "INSTALLED-STEER-OK");
    await idle(writer);
    const originalWork = workFor(original);
    const steeredWork = workFor(steered);
    assert.equal(originalWork.status, "done");
    assert.equal(steeredWork.status, "done");
    assert.equal(steeredWork.started_at, null, "Steering must not start a separate queued turn.");
    assert(steeredWork.ended_at <= originalWork.ended_at);
    results.evidence.steering = { separateTurnStarted: false, steeredAt: steeredWork.ended_at, originalEndedAt: originalWork.ended_at };
  });
  await check("installed Codex Stop", async () => {
    const from = client.events.length;
    const stopped = await send(writer, "Run this command in the foreground and wait: node -e \"setTimeout(() => console.log('done'), 60000)\". Do not put it in the background.");
    await toolsStarted(writer, from);
    const stopAt = Date.now();
    await client.call("chat.stop", { botId: writer.id });
    await idle(writer);
    const elapsedMs = Date.now() - stopAt;
    assert(elapsedMs < 20_000, "Stop must finish well before the 60-second timer could end naturally.");
    assert.equal(workFor(stopped).status, "interrupted");
    results.evidence[current] = { elapsedMs, outcome: "interrupted" };
    await send(writer, "Reply with exactly INSTALLED-AFTER-STOP-OK.");
    await said(writer, "INSTALLED-AFTER-STOP-OK");
    await idle(writer);
  });
  await check("installed running command Stop", async () => {
    const from = client.events.length;
    const stopped = await send(writer, `Run this exact command in the foreground and wait: ${stopCommand}. Do not put it in the background.`);
    await toolsStarted(writer, from);
    const pid = await waitUntil(findCommandPid, 180_000);
    const stopAt = Date.now();
    await client.call("chat.stop", { botId: writer.id });
    await idle(writer);
    const elapsedMs = Date.now() - stopAt;
    assert(elapsedMs < 20_000, "Stop must finish well before the 60-second timer could end naturally.");
    assert.equal(workFor(stopped).status, "interrupted");
    results.evidence[current] = { elapsedMs, outcome: "interrupted" };
    assert.throws(() => process.kill(pid, 0), { code: "ESRCH" });
    await send(writer, "Reply with exactly INSTALLED-AFTER-COMMAND-STOP-OK.");
    await said(writer, "INSTALLED-AFTER-COMMAND-STOP-OK");
    await idle(writer);
  });
  await check("installed persistent approval rule", async () => {
    const careful = await claudeBot(client, "Installed Approval", { autonomy: "ask", browser: false });
    await send(careful, "Use the Write tool to create first.txt containing exactly the two characters hi, with no trailing newline. Then reply INSTALLED-ALLOW-ONE.");
    const card = await approval(careful, "tool");
    assert.equal(card.tool, "Write");
    assert.equal(card.rule, "Write");
    const input = JSON.parse(card.detail);
    assert.equal(typeof input.file_path, "string");
    assert(isAbsolute(input.file_path));
    assert.equal(resolve(input.file_path).toLowerCase(), resolve(careful.homeFolder, "first.txt").toLowerCase());
    assert.equal(input.content, "hi");
    await client.call("approvals.respond", { approvalId: card.id, decision: "always" });
    await said(careful, "INSTALLED-ALLOW-ONE");
    await idle(careful);
    assert.equal(readFileSync(join(careful.homeFolder, "first.txt"), "utf8"), "hi");
    const from = client.events.length;
    await send(careful, "Use the Write tool to create second.txt containing exactly the two characters hi, with no trailing newline. Then reply INSTALLED-ALLOW-TWO.");
    await said(careful, "INSTALLED-ALLOW-TWO");
    await idle(careful);
    assert(!client.events.slice(from).some((event) => event.event === "approval.added" && event.data.botId === careful.id));
    assert.equal(readFileSync(join(careful.homeFolder, "second.txt"), "utf8"), "hi");
  });
  await check("installed browser tools and own profile", async () => {
    await send(researcher, "Use your browser tools to open https://example.com and reply with the page title only.");
    await said(researcher, "Example Domain");
    await idle(researcher);
    const profile = join(app.root, "data", "native-browser", createHash("sha256").update(researcher.id).digest("hex"));
    assert(existsSync(profile), "The bot's isolated native profile must exist.");
    const native = await electronApp.evaluate(({ BrowserWindow, webContents }) => ({
      windows: BrowserWindow.getAllWindows().length,
      pages: webContents.getAllWebContents().filter((contents) => contents.getURL().startsWith("https://example.com")).map((contents) => ({ title: contents.getTitle(), node: contents.getLastWebPreferences().nodeIntegration })),
    }));
    assert.equal(native.windows, 1);
    assert(native.pages.some((entry) => entry.title === "Example Domain" && entry.node === false), "Browser tools must use the native guest, with Node disabled.");
    results.evidence.nativeBrowser = { isolatedProfile: true, sameDesktopGuest: true, windows: native.windows };
  });
  await check("installed routine proposal and approval", async () => {
    await send(researcher, "Use add_routine to propose a routine named Installed proposal, weekdays at 08:48, prompt Reply PROPOSED. Then reply INSTALLED-PROPOSAL-OK.");
    const card = await approval(researcher, "routine");
    await client.call("approvals.respond", { approvalId: card.id, decision: "once" });
    await said(researcher, "INSTALLED-PROPOSAL-OK");
    const routines = await client.call("routines.list", { botId: researcher.id });
    assert(routines.some((routine) => /installed proposal/i.test(routine.name)));
    for (const routine of routines) await client.call("routines.update", { id: routine.id, patch: { enabled: false } });
  });
  await check("installed scheduled routine with window closed", async () => {
    const due = new Date(Math.ceil((Date.now() + 30_000) / 60_000) * 60_000);
    const routine = await client.call("routines.create", { botId: writer.id, name: "Installed clock", prompt: "Reply with exactly INSTALLED-ROUTINE-OK.", days: [due.getDay()], times: [`${String(due.getHours()).padStart(2, "0")}:${String(due.getMinutes()).padStart(2, "0")}`], timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone });
    await electronApp.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0].close());
    assert.equal(await electronApp.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0].isVisible()), false);
    await waitUntil(async () => (await client.call("routines.runs", { routineId: routine.id })).find((run) => run.status === "done"), 240_000);
    await said(writer, "INSTALLED-ROUTINE-OK");
    await electronApp.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0].show());
    await openChat(page, writer.name);
    await page.getByText("Routine: Installed clock", { exact: true }).waitFor();
  });
  const screenshot = resolve(directory, "../../screenshots/phase1-installed-main-chat.png");
  mkdirSync(dirname(screenshot), { recursive: true });
  await page.screenshot({ path: screenshot });
} catch (error) {
  results.checks[current ?? "startup"] = "Fail";
  results.failure = { message: error instanceof Error ? error.message : "Installed acceptance failed." };
  if (observedDb) {
    try { results.failure.work = observedDb.prepare("SELECT source, status FROM work_items ORDER BY created_at DESC LIMIT 8").all(); } catch { /* Preserve the original failure. */ }
  }
  console.error(error instanceof Error ? error.message : "Installed acceptance failed.");
  process.exitCode = 1;
} finally {
  try { observedDb?.close(); }
  catch { results.checks.databaseCleanup = "Fail"; process.exitCode = 1; }
  try { await closeRealApp(app, false); }
  catch { results.checks.cleanup = "Fail"; process.exitCode = 1; }
  results.finishedAt = new Date().toISOString();
  writeFileSync(join(directory, "installed-automated-results.json"), `${JSON.stringify(results, null, 2)}\n`);
}
