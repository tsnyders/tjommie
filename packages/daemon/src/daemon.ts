import type { ProviderSetup } from "@tjommie/protocol";
import { createBot, deleteBot, getStoredBot, listStoredBots, setPaused, updateBot } from "./bots.ts";
import { history } from "./chat.ts";
import { openDb } from "./db.ts";
import { DEFAULT_PORT, dataPaths, readOrCreateToken } from "./paths.ts";
import { Runner, type SessionFactory } from "./runner.ts";
import { startServer, type Broadcast, type Handlers } from "./server.ts";
import { checkSetup, ensureAccounts, listModels, signIn } from "./setup.ts";

// The desktop app loads the UI from app://tjommie (plan 1D); `npm run dev` serves it on localhost:3000.
const ALLOWED_ORIGINS = ["app://tjommie", "http://localhost:3000"];

export type Daemon = { port: number; token: string; runner: Runner; close: () => Promise<void> };

export async function startDaemon(
  options: { root?: string; port?: number; accountsDir?: string; createSession?: SessionFactory } = {},
): Promise<Daemon> {
  const paths = dataPaths(options.root);
  const db = openDb(paths.db);
  const token = readOrCreateToken(paths.token);
  ensureAccounts(db, options.accountsDir ?? paths.accounts);
  let setup: ProviderSetup[] = await checkSetup(db);
  let broadcast: Broadcast = () => {};
  const runner = new Runner({
    db,
    broadcast: (event, data) => broadcast(event, data),
    isSignedIn: (provider) => setup.some((status) => status.provider === provider && status.signedIn),
    createSession: options.createSession,
  });

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
    "bots.delete": ({ id }) => {
      runner.forget(id);
      deleteBot(db, id);
      broadcast("bot.deleted", { id });
      return {};
    },
    "bots.pause": ({ id, paused }) => {
      setPaused(db, id, paused);
      const bot = runner.toBot(getStoredBot(db, id));
      broadcast("bot.updated", bot);
      if (!paused) runner.pump(id);
      return bot;
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
    "approvals.respond": ({ approvalId, decision }) => runner.answerApproval(approvalId, decision),
  };

  const server = await startServer({
    port: options.port ?? DEFAULT_PORT,
    token,
    handlers,
    allowedOrigins: ALLOWED_ORIGINS,
  });
  broadcast = server.broadcast;
  runner.start();
  return {
    port: server.port,
    token,
    runner,
    close: async () => {
      runner.shutdown();
      await server.close();
      db.close();
    },
  };
}
