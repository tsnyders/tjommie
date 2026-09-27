import { existsSync, lstatSync, rmSync } from "node:fs";
import { TEAM_ROOM_ID, type ProviderSetup } from "@tjommie/protocol";
import { getApproval } from "./approvals.ts";
import { createBot, deleteBot, getStoredBot, listStoredBots, setPaused, sweepDeletedBotFolders, updateBot, validateBotPatch } from "./bots.ts";
import { browserServer, showBrowser } from "./browser.ts";
import { historyPage } from "./chat.ts";
import { openDb } from "./db.ts";
import { McpTokens, mcpHandler } from "./mcp.ts";
import { DEFAULT_PORT, dataPaths, migrateLegacyDb, readOrCreateToken } from "./paths.ts";
import { getWorkItem, type WorkItem } from "./queue.ts";
import { createRoutine, deleteRoutine, getRoutine, listRoutines, listRuns, updateRoutine } from "./routines.ts";
import { Runner, type SessionFactory, type TurnResult } from "./runner.ts";
import { Scheduler } from "./scheduler.ts";
import { RpcError, startServer, type Broadcast, type Handlers, type Server } from "./server.ts";
import { getSettings, setSettings } from "./settings.ts";
import { checkSetup, ensureAccounts, listModels, prepareCodexSandbox, signIn } from "./setup.ts";
import { Team, getExchange } from "./team.ts";
import { protectCodexData, protectCodexDataAsync, type BotHome } from "./windows-acl.ts";

// The desktop app loads the UI from app://tjommie (plan 1D); `npm run dev` serves it on localhost:3000.
const allowedOrigins = (mode: DaemonAuth["mode"]) => ["app://tjommie", ...(mode === "development" ? ["http://localhost:3000"] : [])];

async function waitForBotClose(closed: Promise<void>): Promise<void> {
  let timer: NodeJS.Timeout | undefined;
  try {
    await Promise.race([
      closed,
      new Promise<never>((_resolve, reject) => {
        timer = setTimeout(() => reject(new RpcError("busy", "The bot is taking too long to close. Try deleting it again shortly.")), 15_000);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

export type Daemon = { port: number; token: string; runner: Runner; scheduler: Scheduler; close: () => Promise<void> };

export type DaemonAuth = { mode: "development" } | { mode: "production"; token: string };
type DaemonOptions = { auth: DaemonAuth; root?: string; port?: number; accountsDir?: string; createSession?: SessionFactory };

export class PlainStartupError extends Error {}

export async function startDaemon(options: DaemonOptions): Promise<Daemon> {
  if (options.auth.mode === "production" && !/^[A-Za-z0-9_-]{43,}$/.test(options.auth.token)) {
    throw new PlainStartupError("Daemon token must be at least 43 base64url characters.");
  }
  // The listening port is the lock. A duplicate launch must not even open the database.
  const server = await startServer({ port: options.port ?? DEFAULT_PORT, token: "", handlers: {}, allowedOrigins: allowedOrigins(options.auth.mode), deferRequests: true });
  let cleanup = () => {};
  try {
    return await initializeDaemon(options, server, (next) => { cleanup = next; });
  } catch (error) {
    try { cleanup(); } finally { await server.close(); }
    throw error;
  }
}

async function initializeDaemon(options: DaemonOptions, server: Server, onCleanup: (cleanup: () => void) => void): Promise<Daemon> {
  const paths = dataPaths(options.root);
  migrateLegacyDb(paths);
  if (options.auth.mode === "production") {
    const leftover = existsSync(paths.token);
    try {
      if (leftover && !lstatSync(paths.token).isFile()) throw new Error("Not a regular file");
      rmSync(paths.token, { force: true, recursive: true, maxRetries: 3 });
    } catch {
      throw new PlainStartupError("Could not remove the development daemon-token file. Close programs using it and start Tjommie again.");
    }
    if (leftover) console.log("Removed a leftover development daemon-token file.");
  }
  // This is the only synchronous ACL pass. It finishes before SQLite opens.
  let protection = protectCodexData(paths);
  const db = openDb(paths.db);
  onCleanup(() => db.close());
  const token = options.auth.mode === "development" ? readOrCreateToken(paths.token) : options.auth.token;
  ensureAccounts(db, options.accountsDir ?? paths.accounts, paths.root);
  // Account and token creation, plus existing bot homes, need a fresh pass.
  protection = await protectCodexDataAsync(paths, listStoredBots(db));
  let lastProtectionAt = Date.now();
  let aclQueue: Promise<void> = Promise.resolve();
  const refreshProtection = (force = false, override?: BotHome): Promise<void> => {
    const task = aclQueue.then(async () => {
      if (!force && Date.now() - lastProtectionAt < 60_000) return;
      const homes = listStoredBots(db).map((bot) => bot.id === override?.id ? override : bot);
      protection = await protectCodexDataAsync(paths, homes);
      lastProtectionAt = Date.now();
    });
    aclQueue = task.catch(() => {});
    return task;
  };
  let loggedCodexVersion: string | null = null;
  const logVersionDrift = (statuses: ProviderSetup[]): void => {
    const version = statuses.find((status) => status.provider === "codex")?.version;
    if (version && version !== "0.155.1" && version !== loggedCodexVersion) {
      console.info(`Codex CLI ${version} differs from the measured ACL recipe version 0.155.1.`);
      loggedCodexVersion = version;
    }
  };
  let setup: ProviderSetup[] = [];
  let broadcast: Broadcast = () => {};
  const send: Broadcast = (event, data) => broadcast(event, data);
  let mcpUrl = ""; // Set before queued work can start a CLI session.
  const tokens = new McpTokens();
  const switchingBots = new Set<string>();
  const runner: Runner = new Runner({
    db,
    broadcast: send,
    isSignedIn: (provider) => setup.some((status) => status.provider === provider && status.signedIn),
    codexSandboxReady: () => setup.some((status) => status.provider === "codex" && status.sandbox === "ready"),
    setupIssue: (bot) => {
      if (switchingBots.has(bot.id)) return "This bot is changing provider. Try again shortly.";
      if (bot.provider !== "codex" || bot.autonomy !== "folder") return null;
      if (!setup.some((status) => status.provider === "codex" && status.sandbox === "ready")) return "Set up Codex's sandbox";
      return protection.reason;
    },
    isHeld: (botId) => switchingBots.has(botId),
    createSession: options.createSession,
    hooks: {
      retryRoutine: (message) => scheduler.retry(message),
      mcpServers: (bot) => [
        { name: "tjommie", url: mcpUrl, headers: { Authorization: `Bearer ${tokens.issue(bot.id)}` }, autoAllow: true },
        ...(bot.browser ? [browserServer(bot)] : []),
      ],
      sessionClosed: (botId) => {
        tokens.revoke(botId);
        team.sessionClosed(botId);
      },
      turnStarted: (item) => {
        if (item.routineRunId) scheduler.runStarted(item);
      },
      turnEnded,
      turnRequeued: (item) => {
        if (item.routineRunId) scheduler.runRequeued(item);
      },
    },
  });
  const team = new Team({
    db, broadcast: send,
    deliver: (work) => runner.deliver(work),
    currentWork: (botId) => runner.currentWork(botId),
    stateOf: (bot) => runner.toBot(bot).state,
    botView: (bot) => runner.toBot(bot),
    stopItem: (itemId, notice) => runner.stopItem(itemId, notice),
  });
  const scheduler: Scheduler = new Scheduler({
    db, broadcast: send,
    deliver: (work) => runner.deliver(work),
    stopItem: (itemId, notice) => runner.stopItem(itemId, notice),
  });
  onCleanup(() => { scheduler.stop(); runner.shutdown(); db.close(); });

  function turnEnded(item: WorkItem, result: TurnResult): void {
    try {
      if (item.routineRunId) scheduler.runEnded(item, result);
    } finally {
      team.turnEnded(item, result);
    }
  }

  const refreshSetup = async (forceAcl = false): Promise<ProviderSetup[]> => {
    await refreshProtection(forceAcl);
    setup = await checkSetup(db);
    logVersionDrift(setup);
    broadcast("setup.changed", setup);
    for (const bot of listStoredBots(db)) broadcast("bot.updated", runner.toBot(bot));
    runner.setupChanged();
    return setup;
  };

  const refreshSetupInBackground = (): void => {
    void refreshSetup(true).catch((error: unknown) => console.error("Failed to refresh provider setup.", error));
  };

  const deletionRequests = new Set<string>();
  const handlers: Handlers = {
    "setup.status": () => refreshSetup(),
    "setup.signIn": ({ provider }) => signIn(db, provider, refreshSetupInBackground),
    "setup.prepareCodexSandbox": async () => {
      try {
        await prepareCodexSandbox(db);
      } finally {
        await refreshSetup(true);
      }
      const status = setup.find((entry) => entry.provider === "codex");
      if (!status || status.provider !== "codex") throw new RpcError("internal", "Codex setup status is missing.");
      return status;
    },
    "models.list": ({ provider }) => listModels(db, provider),
    "bots.list": () => listStoredBots(db).map((bot) => runner.toBot(bot)),
    "bots.create": async (input) => {
      let created;
      try { created = createBot(db, paths.bots, input); }
      catch (error) { await refreshProtection(true); runner.setupChanged(); throw error; }
      await refreshProtection(true);
      runner.setupChanged();
      const bot = runner.toBot(created);
      broadcast("bot.updated", bot);
      return bot;
    },
    "bots.update": async ({ id, patch }) => {
      const before = getStoredBot(db, id);
      const providerChanged = patch.provider !== undefined && patch.provider !== before.provider;
      if (providerChanged) {
        if (switchingBots.has(id)) throw new RpcError("busy", "This bot is already changing provider. Try again shortly.");
        validateBotPatch(db, before, patch);
        switchingBots.add(id);
      }
      let passAttempted = false;
      let changed: ReturnType<typeof updateBot>;
      try {
        if (providerChanged) {
          await runner.stop(id);
          runner.botChanged(id, true);
          passAttempted = true;
          await refreshProtection(true, { id, homeFolder: before.homeFolder, provider: patch.provider! });
          if (protection.status === "check_failed") throw new RpcError("bad_state", "Could not update the bot folder protection. Try again after checking Codex's sandbox setup.");
        }
        changed = updateBot(db, id, patch);
      } catch (error) {
        if (providerChanged && passAttempted) await refreshProtection(true);
        throw error;
      } finally {
        if (providerChanged) {
          switchingBots.delete(id);
          runner.setupChanged();
        }
      }
      if (!providerChanged) runner.botChanged(id, false);
      const bot = runner.toBot(changed);
      broadcast("bot.updated", bot);
      return bot;
    },
    "bots.delete": async ({ id }) => {
      getStoredBot(db, id);
      if (deletionRequests.has(id)) throw new RpcError("busy", "The bot is already being deleted. Try again shortly.");
      deletionRequests.add(id);
      try {
        let handle: ReturnType<Team["botDeleting"]>;
        try {
          await waitForBotClose(runner.closeForDeletion(id));
          handle = team.botDeleting(id);
          deleteBot(db, id);
        } catch (error) {
          runner.cancelDeletion(id);
          throw error;
        }
        try {
          team.botDeleted(handle);
        } finally {
          runner.forget(id);
          broadcast("bot.deleted", { id });
        }
        return {};
      } finally {
        deletionRequests.delete(id);
      }
    },
    "bots.pause": ({ id, paused }) => {
      setPaused(db, id, paused);
      const bot = runner.toBot(getStoredBot(db, id));
      broadcast("bot.updated", bot);
      if (!paused) runner.pump(id);
      return bot;
    },
    "bots.showBrowser": async ({ id }) => {
      const bot = getStoredBot(db, id);
      if (!bot.browser) throw new RpcError("bad_params", "This bot's browser is off");
      await showBrowser(bot);
      return {};
    },
    "chat.history": ({ botId, before, limit }) => {
      getStoredBot(db, botId);
      return historyPage(db, botId, before, limit);
    },
    "chat.send": ({ botId, text, files }) => runner.submit(botId, text, files),
    "chat.stop": async ({ botId }) => {
      await runner.stop(botId);
      return {};
    },
    "chat.retry": ({ messageId, botId }) => runner.retry(messageId, botId),
    "approvals.respond": ({ approvalId, decision }) => {
      const approval = getApproval(db, approvalId).kind === "routine"
        ? team.answerRoutineProposal(approvalId, decision)
        : runner.answerApproval(approvalId, decision);
      runner.pump(approval.botId);
      return approval;
    },
    "room.history": ({ before, limit }) => historyPage(db, TEAM_ROOM_ID, before, limit),
    "room.post": ({ text }) => team.postToRoom(text),
    "exchanges.get": ({ id }) => ({ exchange: getExchange(db, id), ...historyPage(db, id, undefined, 200) }),
    "routines.list": ({ botId }) => {
      if (botId !== undefined) getStoredBot(db, botId);
      return listRoutines(db, botId);
    },
    "routines.create": (input) => {
      const routine = createRoutine(db, input);
      broadcast("routine.updated", routine);
      return routine;
    },
    "routines.update": ({ id, patch }) => {
      const routine = updateRoutine(db, id, patch);
      broadcast("routine.updated", routine);
      return routine;
    },
    "routines.delete": ({ id }) => {
      const queued = db.prepare("SELECT id FROM work_items WHERE status = 'queued' AND routine_run_id IN (SELECT id FROM routine_runs WHERE routine_id = ?)").all(id) as { id: string }[];
      const dropped = queued.map((row) => getWorkItem(db, row.id));
      deleteRoutine(db, id);
      for (const item of dropped) turnEnded(item, { outcome: "interrupted", text: null, textMessageId: null, error: "The routine was deleted." });
      broadcast("routine.deleted", { id });
      return {};
    },
    "routines.runs": ({ routineId }) => {
      getRoutine(db, routineId);
      return listRuns(db, routineId);
    },
    "settings.get": () => getSettings(db),
    "settings.set": (patch) => {
      const settings = setSettings(db, patch);
      broadcast("settings.changed", settings);
      return settings;
    },
  };

  sweepDeletedBotFolders(db, paths.bots);
  team.start();
  runner.recover(); // Synchronous after binding, before setup yields or requests can be served.
  setup = await checkSetup(db);
  logVersionDrift(setup);
  mcpUrl = `http://127.0.0.1:${server.port}/mcp`;
  broadcast = server.broadcast;
  runner.start();
  scheduler.start();
  server.activate({ token, handlers, routes: { "/mcp": mcpHandler({ tokens, tools: team.tools() }) } });
  return {
    port: server.port,
    token,
    runner,
    scheduler,
    close: async () => {
      scheduler.stop();
      runner.shutdown();
      await server.close();
      db.close();
    },
  };
}
