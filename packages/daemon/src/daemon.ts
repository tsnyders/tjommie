import { TEAM_ROOM_ID, type ProviderSetup } from "@tjommie/protocol";
import { getApproval } from "./approvals.ts";
import { createBot, deleteBot, getStoredBot, listStoredBots, setPaused, sweepDeletedBotFolders, updateBot } from "./bots.ts";
import { browserServer, showBrowser } from "./browser.ts";
import { history } from "./chat.ts";
import { openDb } from "./db.ts";
import { McpTokens, mcpHandler } from "./mcp.ts";
import { DEFAULT_PORT, dataPaths, readOrCreateToken } from "./paths.ts";
import { getWorkItem, type WorkItem } from "./queue.ts";
import { createRoutine, deleteRoutine, getRoutine, listRoutines, listRuns, updateRoutine } from "./routines.ts";
import { Runner, type SessionFactory, type TurnResult } from "./runner.ts";
import { Scheduler } from "./scheduler.ts";
import { RpcError, startServer, type Broadcast, type Handlers, type Server } from "./server.ts";
import { getSettings, setSettings } from "./settings.ts";
import { checkSetup, ensureAccounts, listModels, signIn } from "./setup.ts";
import { Team, getExchange } from "./team.ts";

// The desktop app loads the UI from app://tjommie (plan 1D); `npm run dev` serves it on localhost:3000.
const allowedOrigins = () => ["app://tjommie", ...(process.env.TJOMMIE_DEV === "1" ? ["http://localhost:3000"] : [])];

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

type DaemonOptions = { root?: string; port?: number; accountsDir?: string; createSession?: SessionFactory };

export async function startDaemon(options: DaemonOptions = {}): Promise<Daemon> {
  // The listening port is the lock. A duplicate launch must not even open the database.
  const server = await startServer({ port: options.port ?? DEFAULT_PORT, token: "", handlers: {}, allowedOrigins: allowedOrigins(), deferRequests: true });
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
  const db = openDb(paths.db);
  onCleanup(() => db.close());
  const token = readOrCreateToken(paths.token);
  ensureAccounts(db, options.accountsDir ?? paths.accounts, paths.root);
  let setup: ProviderSetup[] = [];
  let broadcast: Broadcast = () => {};
  const send: Broadcast = (event, data) => broadcast(event, data);
  let mcpUrl = ""; // Set before queued work can start a CLI session.
  const tokens = new McpTokens();
  const runner = new Runner({
    db,
    broadcast: send,
    isSignedIn: (provider) => setup.some((status) => status.provider === provider && status.signedIn),
    createSession: options.createSession,
    hooks: {
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
    stopItem: (itemId, notice) => runner.stopItem(itemId, notice),
  });
  const scheduler = new Scheduler({
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

  const refreshSetup = async (): Promise<ProviderSetup[]> => {
    setup = await checkSetup(db);
    broadcast("setup.changed", setup);
    for (const bot of listStoredBots(db)) broadcast("bot.updated", runner.toBot(bot));
    runner.pumpAll();
    return setup;
  };

  const refreshSetupInBackground = (): void => {
    void refreshSetup().catch((error: unknown) => console.error("Failed to refresh provider setup.", error));
  };

  const deletionRequests = new Set<string>();
  const handlers: Handlers = {
    "setup.status": () => refreshSetup(),
    "setup.signIn": ({ provider }) => signIn(db, provider, refreshSetupInBackground),
    "models.list": ({ provider }) => listModels(db, provider),
    "bots.list": () => listStoredBots(db).map((bot) => runner.toBot(bot)),
    "bots.create": (input) => {
      const bot = runner.toBot(createBot(db, paths.bots, input));
      broadcast("bot.updated", bot);
      return bot;
    },
    "bots.update": ({ id, patch }) => {
      const before = getStoredBot(db, id);
      const bot = runner.toBot(updateBot(db, id, patch));
      runner.botChanged(id, before.provider !== bot.provider);
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
      return history(db, botId, before, limit);
    },
    "chat.send": ({ botId, text, files }) => runner.submit(botId, text, files),
    "chat.stop": async ({ botId }) => {
      await runner.stop(botId);
      return {};
    },
    "chat.retry": ({ messageId }) => runner.retry(messageId),
    "approvals.respond": ({ approvalId, decision }) =>
      getApproval(db, approvalId).kind === "routine"
        ? team.answerRoutineProposal(approvalId, decision)
        : runner.answerApproval(approvalId, decision),
    "room.history": ({ before, limit }) => history(db, TEAM_ROOM_ID, before, limit),
    "room.post": ({ text }) => team.postToRoom(text),
    "exchanges.get": ({ id }) => ({ exchange: getExchange(db, id), messages: history(db, id, undefined, 200) }),
    "routines.list": ({ botId }) => {
      getStoredBot(db, botId);
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
