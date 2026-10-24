import { existsSync, lstatSync, readFileSync, rmSync } from "node:fs";
import { join, resolve } from "node:path";
import { SETUP_CODEX_SANDBOX_REASON, TEAM_ROOM_ID, type ProviderSetup } from "@tjommie/protocol";
import { AccountRuntime, type AccountRuntimeOptions } from "./account-runtime.ts";
import { accountHandlers } from "./account-handlers.ts";
import { bindLegacyBots, getAccount, getDefaultAccount, listAccounts } from "./accounts-store.ts";
import { getApproval } from "./approvals.ts";
import { createBot, deleteBot, getStoredBot, listStoredBots, setPaused, sweepDeletedBotFolders, updateBot, validateBotPatch } from "./bots.ts";
import { browsers, sharedBrowserServer, showBrowser } from "./browser.ts";
import { browserHandlers } from "./browser-handlers.ts";
import { BrowserMcp } from "./browser-mcp.ts";
import { NativeBrowserBroker } from "./native-browser-broker.ts";
import { NativeBrowserMcp } from "./native-browser-mcp.ts";
import { nativeBrowserHandlers } from "./native-browser-handlers.ts";
import type { NativeBrowserCommand } from "../../protocol/src/native-browser.ts";
import { runCleanup } from "./cleanup.ts";
import { historyPage } from "./chat.ts";
import { openDb } from "./db.ts";
import type { GoogleAuth } from "./google-auth.ts";
import { googleTools } from "./google-tools.ts";
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
import { RemoteHost } from "./remote-host.ts";
import { listDevices, renameDevice } from "./devices-store.ts";
import type { CredentialTransport } from "./account-credentials.ts";

const NOTES_PREVIEW_CHARS = 4_000;

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

export type DaemonAuth = { mode: "development" } | { mode: "production"; token: string; desktopHostToken?: string };
type DaemonOptions = { auth: DaemonAuth; root?: string; port?: number; accountsDir?: string; createSession?: SessionFactory; googleAuth?: GoogleAuth;
  remote?: { credentials?: CredentialTransport; port?: number; bind?: string; addresses?: () => string[] };
  createAccountsRuntime?: (options: AccountRuntimeOptions) => AccountRuntime };

export class PlainStartupError extends Error {}

export async function startDaemon(options: DaemonOptions): Promise<Daemon> {
  if (options.auth.mode === "production" && !/^[A-Za-z0-9_-]{43,}$/.test(options.auth.token)) {
    throw new PlainStartupError("Daemon token must be at least 43 base64url characters.");
  }
  if (options.auth.mode === "production" && options.auth.desktopHostToken !== undefined &&
    (!/^[A-Za-z0-9_-]{43,}$/.test(options.auth.desktopHostToken) || options.auth.desktopHostToken === options.auth.token)) {
    throw new PlainStartupError("The native browser host requires an independent private capability.");
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
  if (options.accountsDir) paths.accounts = resolve(options.accountsDir);
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
  bindLegacyBots(db);
  // Account and token creation, plus existing bot homes, need a fresh pass.
  protection = await protectCodexDataAsync(paths, listStoredBots(db), undefined, undefined, listAccounts(db));
  let lastProtectionAt = Date.now();
  let aclQueue: Promise<void> = Promise.resolve();
  const refreshProtection = (force = false, override?: BotHome): Promise<void> => {
    const task = aclQueue.then(async () => {
      if (!force && Date.now() - lastProtectionAt < 60_000) return;
      const homes = listStoredBots(db).map((bot) => bot.id === override?.id ? override : bot);
      protection = await protectCodexDataAsync(paths, homes, undefined, undefined, listAccounts(db));
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
  // The separate API feature is on hold. Only isolated dependency-injected tests enable its wiring.
  let googleOpening: Promise<GoogleAuth> | undefined = options.googleAuth ? Promise.resolve(options.googleAuth) : undefined;
  const google = (): Promise<GoogleAuth> => googleOpening ?? Promise.reject(new RpcError("bad_state", "The separate Google API connection is not enabled in this version. Use the native browser."));
  let mcpUrl = ""; // Set before queued work can start a CLI session.
  const tokens = new McpTokens();
  const nativeBroker = options.auth.mode === "production" && options.auth.desktopHostToken
    ? new NativeBrowserBroker<NativeBrowserCommand>(options.auth.desktopHostToken) : null;
  const nativeRequest = async (id: string, command: NativeBrowserCommand, authorized?: () => boolean): Promise<unknown> => {
    const bot = getStoredBot(db, id);
    if (command.kind !== "close" && command.kind !== "cancel_tools" && command.kind !== "delete_profile" && !bot.browser) throw new RpcError("bad_state", "This bot's browser is off.");
    if (!nativeBroker) throw new RpcError("bad_state", "The native browser needs the Tjommie desktop app.");
    return nativeBroker.request(bot, command, authorized);
  };
  const browserMcp = nativeBroker
    ? new NativeBrowserMcp({ tokens, getBot: (id: string) => getStoredBot(db, id), request: nativeRequest, cancelTools: (id: string) => {
      nativeBroker.cancelTools(id);
      void nativeRequest(id, { kind: "cancel_tools" }).catch(() => {});
    } })
    : new BrowserMcp({ tokens, getBot: (id) => getStoredBot(db, id) });
  const switchingBots = new Set<string>();
  const changingAccounts = new Set<string>();
  const useAccountRuntime = options.createSession === undefined || options.createAccountsRuntime !== undefined;
  let accountsReady = false;
  let runner: Runner;
  const runtimeOptions: AccountRuntimeOptions = {
    db, paths,
    onChanged: () => {
      if (!accountsReady) return;
      const current = accounts.list();
      setup = current.filter((account) => account.isDefault).flatMap((account) => account.setup ? [account.setup] : []);
      send("accounts.changed", current);
      send("setup.changed", setup);
      runner.setupChanged();
      for (const bot of listStoredBots(db)) send("bot.updated", runner.toBot(bot));
    },
    onSignInEnded: (accountId, attempt) => {
      send("accounts.signInEnded", { accountId, attempt });
      const account = getAccount(db, accountId);
      if (getDefaultAccount(db, account.provider).id === accountId) signInEnded(account.provider, attempt);
    },
    onCap: (accountId, threshold) => {
      const account = getAccount(db, accountId);
      const unpriced = accounts.info(accountId).spending.unknownRows > 0;
      for (const bot of listStoredBots(db)) {
        if (bot.accountId !== accountId && runner.toBot(bot).effectiveAccountId !== accountId) continue;
        if (threshold === 100) void runner.stop(bot.id).catch(() => {
          send("notify", { kind: "limit", botId: bot.id, title: "Check this bot after its spending cap", body: "The provider did not confirm Stop. Check the chat before starting more work." });
        });
        send("notify", { kind: "limit", botId: bot.id, title: unpriced ? `${account.label} has unpriced usage` : `${account.label} reached ${threshold}% of its local cap`,
          body: unpriced ? "This key is blocked because a turn's cost is unknown. Review its usage in Accounts." : threshold === 100 ? "This key reached its local cap. Review its recorded spend in Accounts." : "Review this key's recorded spend in Accounts. Provider charges may differ from local estimates." });
      }
    },
  };
  const accounts = options.createAccountsRuntime?.(runtimeOptions) ?? new AccountRuntime(runtimeOptions);
  runner = new Runner({
    db,
    broadcast: send,
    isSignedIn: (provider) => setup.some((status) => status.provider === provider && status.signedIn),
    codexSandboxReady: () => setup.some((status) => status.provider === "codex" && status.sandbox === "ready"),
    setupIssue: (bot) => {
      if (switchingBots.has(bot.id)) return "This bot is changing provider. Try again shortly.";
      if (bot.provider !== "codex" || bot.autonomy !== "folder") return null;
      if (!useAccountRuntime && !setup.some((status) => status.provider === "codex" && status.sandbox === "ready")) return SETUP_CODEX_SANDBOX_REASON;
      return protection.reason;
    },
    isHeld: (botId) => switchingBots.has(botId),
    createSession: options.createSession,
    ...(useAccountRuntime ? { accounts } : {}),
    hooks: {
      retryRoutine: (message) => scheduler.retry(message),
      mcpServers: (bot) => {
        const sessionToken = tokens.issue(bot.id);
        return [
          { name: "tjommie", url: mcpUrl, headers: { Authorization: `Bearer ${sessionToken}` }, autoAllow: true },
          ...(bot.browser ? [sharedBrowserServer(bot, `${mcpUrl}/browser`, sessionToken)] : []),
        ];
      },
      sessionClosed: (botId) => {
        tokens.revoke(botId);
        void browserMcp.close(botId).catch(() => {});
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
    if (useAccountRuntime) {
      const defaults = listAccounts(db).filter((account) => account.isDefault);
      await Promise.all(defaults.map((account) => accounts.check(account.id)));
      setup = accounts.list().filter((account) => account.isDefault)
        .flatMap((account) => account.setup ? [account.setup] : []);
    } else setup = await checkSetup(db);
    logVersionDrift(setup);
    broadcast("setup.changed", setup);
    for (const bot of listStoredBots(db)) broadcast("bot.updated", runner.toBot(bot));
    runner.setupChanged();
    return setup;
  };

  // A sign-in ended. The fresh setup goes out first, so one that worked already reads as signed in; then
  // setup.signInEnded, because one that failed or was abandoned changes nothing a client could notice.
  const signInEnded = (provider: ProviderSetup["provider"], attempt: string): void => {
    void refreshSetup(true)
      .catch((error: unknown) => console.error("Failed to refresh provider setup.", error))
      .finally(() => broadcast("setup.signInEnded", { provider, attempt }));
  };

  const deletionRequests = new Set<string>();
  const accountsApi = accountHandlers({ db, runtime: accounts, changing: changingAccounts,
    accountsDir: options.accountsDir ?? paths.accounts, protect: () => refreshProtection(true) });
  function checkAccountAvailable(provider: ProviderSetup["provider"], requested?: string | null): void {
    const id = requested ?? getDefaultAccount(db, provider).id;
    if (changingAccounts.has(id)) throw new RpcError("busy", "This account is changing. Try again shortly.");
  }
  const remote = new RemoteHost({ db, root: paths.root, dispatch: server.dispatch, ...options.remote, onChanged: () => send("devices.changed", {}) });
  const handlers: Handlers = {
    "remote.status": () => remote.status(),
    "devices.list": () => listDevices(db),
    "devices.rename": ({ id, name }) => { const device = renameDevice(db, id, name); send("devices.changed", {}); return device; },
    "devices.revoke": ({ id }) => remote.revoke(id),
    "pairing.create": () => remote.createPairing(),
    "pairing.cancel": ({ id }) => { remote.cancelPairing(id); return {}; },
    ...accountsApi,
    ...browserHandlers((id) => {
      if (deletionRequests.has(id)) throw new RpcError("busy", "This bot is being deleted.");
      return getStoredBot(db, id);
    }),
    ...(nativeBroker ? nativeBrowserHandlers(nativeRequest) : {}),
    "google.status": async () => (await google()).status(),
    "google.configure": async (config) => (await google()).configure(config),
    "google.begin": async ({ services }) => (await google()).begin(services),
    "google.cancel": async ({ attempt }) => (await google()).cancel(attempt),
    "google.disconnect": async () => (await google()).disconnect(),
    "google.grant": async ({ botId, enabled }) => {
      const auth = await google();
      if (deletionRequests.has(botId)) throw new RpcError("busy", "This bot is being deleted.");
      getStoredBot(db, botId);
      return auth.setGrant(botId, enabled);
    },
    "setup.status": () => refreshSetup(),
    "setup.signIn": ({ provider }) => useAccountRuntime
      ? accountsApi["accounts.signIn"]!({ id: getDefaultAccount(db, provider).id })
      : signIn(db, provider, (_ok, attempt) => signInEnded(provider, attempt)),
    "setup.prepareCodexSandbox": async () => {
      try {
        if (useAccountRuntime) await accountsApi["accounts.prepareCodexSandbox"]!({ id: getDefaultAccount(db, "codex").id });
        else await prepareCodexSandbox(db);
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
      checkAccountAvailable(input.provider, input.accountId);
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
      const accountChanged = patch.accountId !== undefined && patch.accountId !== before.accountId;
      const identityChanged = providerChanged || accountChanged;
      if (identityChanged) {
        checkAccountAvailable(patch.provider ?? before.provider, patch.accountId ?? (providerChanged ? undefined : before.accountId));
        if (switchingBots.has(id)) throw new RpcError("busy", "This bot is already changing provider. Try again shortly.");
        validateBotPatch(db, before, patch);
        switchingBots.add(id);
      }
      let passAttempted = false;
      let changed: ReturnType<typeof updateBot>;
      try {
        if (identityChanged) {
          await runner.stop(id);
          runner.botChanged(id, true);
        }
        if (providerChanged) {
          passAttempted = true;
          await refreshProtection(true, { id, homeFolder: before.homeFolder, provider: patch.provider! });
          if (protection.status === "check_failed") throw new RpcError("bad_state", "Could not update the bot folder protection. Try again after checking Codex's sandbox setup.");
        }
        changed = updateBot(db, id, patch);
      } catch (error) {
        if (providerChanged && passAttempted) await refreshProtection(true);
        throw error;
      } finally {
        if (identityChanged) {
          switchingBots.delete(id);
          runner.setupChanged();
        }
      }
      if (!identityChanged) runner.botChanged(id, false);
      const bot = runner.toBot(changed);
      try {
        if (before.browser && !changed.browser) {
          await runCleanup([() => browserMcp.close(id), () => nativeBroker ? nativeRequest(id, { kind: "close" }).then(() => { nativeBroker.closeBot(id); }) : browsers.close(id)], "The browser was disabled, but some browser resources could not close.");
        }
      } finally { broadcast("bot.updated", bot); }
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
          if (nativeBroker) {
            await nativeRequest(id, { kind: "delete_profile" });
            nativeBroker.closeBot(id);
          }
          if (googleOpening) {
            const auth = await google();
            if (auth.status().grantedBotIds.includes(id)) await auth.unregisterBot(id);
          }
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
      if (nativeBroker) { await nativeRequest(id, { kind: "open" }); return {}; }
      await showBrowser(bot);
      return {};
    },
    // Bot settings shows the start of the bot's own notes.
    "bots.notes": ({ id }) => {
      const file = join(getStoredBot(db, id).homeFolder, "notes.md");
      return { text: existsSync(file) ? readFileSync(file, "utf8").slice(0, NOTES_PREVIEW_CHARS) : "" };
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
    "notifications.pending": () => scheduler.pendingFailures(),
    "notifications.ack": ({ runId }) => {
      scheduler.acknowledgeFailure(runId);
      return {};
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
  setup = useAccountRuntime ? [] : await checkSetup(db);
  logVersionDrift(setup);
  mcpUrl = `http://127.0.0.1:${server.port}/mcp`;
  broadcast = (event, data) => { server.broadcast(event, data); remote.broadcast(event, data); };
  accountsReady = true;
  runner.start();
  scheduler.start();
  server.activate({ token, handlers, routes: { "/mcp": mcpHandler({ tokens, tools: [...team.tools(), ...(options.googleAuth ? googleTools({ request: async (botId, service, path) => {
    if (deletionRequests.has(botId)) throw new RpcError("busy", "This bot is being deleted.");
    getStoredBot(db, botId);
    return (await google()).request(botId, service, path);
  } }) : [])] }), "/mcp/browser": browserMcp.handle, ...(nativeBroker ? { "/browser-host": nativeBroker.handle } : {}) } });
  if (useAccountRuntime) void accounts.checkAll().catch(() => console.error("Some accounts could not be checked. Retry from Accounts."));
  if (remote.hasDevices()) void remote.start().catch(() => console.error("The phone connection could not start. Retry from Devices."));
  return {
    port: server.port,
    token,
    runner,
    scheduler,
    close: async () => {
      await runCleanup([
        () => remote.close(),
        () => scheduler.stop(),
        () => runner.shutdown(),
        () => accounts.close(),
        async () => { if (googleOpening) await (await googleOpening).close(); },
        () => browserMcp.closeAll(),
        () => nativeBroker?.close(),
        () => browsers.closeAll(),
        () => server.close(),
        () => db.close(),
      ], "Tjommie stopped with cleanup errors.");
    },
  };
}
