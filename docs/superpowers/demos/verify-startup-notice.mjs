// Installed desktop integration: recovery happens before a client exists, then the desktop handles the notice.
// All database writes, process termination and cleanup belong to this disposable app root.
import assert from "node:assert/strict";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { connect } from "../../../packages/client/src/index.ts";
import { openDb } from "../../../packages/daemon/src/db.ts";
import { addRun } from "../../../packages/daemon/src/routines.ts";
import { launchRealApp, closeRealApp, daemonConnection, listeningPid, rememberDaemon, terminateOwnedProcess } from "../../../packages/desktop/src/real-app.ts";

const executable = process.argv[2];
assert(executable, "Supply the installed executable path.");
let app;
const result = { startedAt: new Date().toISOString(), checks: {}, error: null };
async function waitUntil(check) {
  const until = Date.now() + 90_000;
  for (;;) {
    const value = await check();
    if (value !== false && value !== null && value !== undefined) return value;
    if (Date.now() > until) throw new Error("Startup notice acceptance timed out.");
    await new Promise((resolve) => setTimeout(resolve, 250));
  }
}
try {
  app = await launchRealApp(executable, { isolatedAccounts: true });
  const bot = await app.client.call("bots.create", { name: "Startup notice fixture", color: "blue", shape: "circle", provider: "claude", model: "haiku", instructions: "Do not run tasks", autonomy: "ask" });
  const routine = await app.client.call("routines.create", { botId: bot.id, name: "Recovery fixture", prompt: "Synthetic test", days: [1], times: ["23:59"], timeZone: "UTC", enabled: false });
  const db = openDb(join(app.root, "data", "tjommie.db"));
  let run;
  try { run = addRun(db, routine.id, new Date().toISOString(), "queued"); }
  finally { db.close(); }
  const connection = await daemonConnection(app.page);
  // Keep the app focused so the documented foreground suppression counts as handled delivery;
  // the persisted failed label remains visible, and no test depends on Windows toast settings.
  assert(await app.electronApp.evaluate(({ BrowserWindow }) => {
    const window = BrowserWindow.getAllWindows()[0];
    window.show(); window.focus(); return window.isFocused();
  }));
  await terminateOwnedProcess(app, listeningPid(connection.port));
  const next = await waitUntil(async () => {
    const next = await daemonConnection(app.page);
    return next.token !== connection.token ? next : false;
  });
  await app.client.close();
  app.client = await connect(next.port, next.token);
  rememberDaemon(app, next.port);
  const recovered = await waitUntil(async () => (await app.client.call("routines.runs", { routineId: routine.id })).find((entry) => entry.id === run.id && entry.status === "failed"));
  assert.match(recovered.note, /restarted/);
  await waitUntil(async () => !(await app.client.call("notifications.pending", {})).some((notice) => notice.runId === run.id));
  // Neither this script nor the UI sends acknowledgements. Only the installed desktop consumer
  // can clear this notice after subscribing, showing it or applying foreground suppression.
  result.checks["orphan recovery persists failed history before clients reconnect"] = "Pass";
  result.checks["installed desktop handles and acknowledges startup notice"] = "Pass";
  result.notificationMode = "Foreground suppression; Windows toast rendering is not asserted";
} catch (error) { result.error = String(error); process.exitCode = 1; }
finally {
  await closeRealApp(app, false).catch((error) => { result.cleanupError = String(error); process.exitCode = 1; });
  result.finishedAt = new Date().toISOString();
  writeFileSync(fileURLToPath(new URL("./startup-notice-results.json", import.meta.url)), `${JSON.stringify(result, null, 2)}\n`);
  console.log(JSON.stringify(result));
}
