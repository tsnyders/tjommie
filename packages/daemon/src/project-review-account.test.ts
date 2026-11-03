import { join } from "node:path";
import { afterEach, expect, it, vi } from "vitest";
import type { ProviderSetup } from "@tjommie/protocol";
import { AccountRuntime } from "./account-runtime.ts";
import { createAccount } from "./accounts-store.ts";
import { createBot, updateBot } from "./bots.ts";
import { openDb } from "./db.ts";
import { dataPaths } from "./paths.ts";
import { reviewAccountResolver } from "./project-review-account.ts";
import { ensureAccounts } from "./setup.ts";
import { tempRoot } from "./testing.ts";

const databases: ReturnType<typeof openDb>[] = [];
afterEach(() => { for (const db of databases.splice(0)) db.close(); });

async function fixture(api = true) {
  const paths = dataPaths(tempRoot()), db = openDb(paths.db); databases.push(db);
  ensureAccounts(db, paths.accounts);
  const keys = new Map<string, string>();
  const read = vi.fn(async (id: string) => keys.get(id) ?? null);
  const accounts = new AccountRuntime({ db, paths,
    credentials: { reference: (id) => `fixture/${id}`, read,
      write: async (id, key) => { keys.set(id, key); }, delete: async (id) => { keys.delete(id); } },
    probe: async (account): Promise<ProviderSetup> => account.provider === "claude"
      ? { provider: "claude", installed: true, version: "2.1.314", signedIn: true, account: null, plan: null, error: null }
      : { provider: "codex", installed: true, version: "0.160.0", signedIn: true, account: null, plan: null, error: null, sandbox: "ready" },
    probeLimit: async () => null,
  });
  if (api) {
    createAccount(db, { id: "review-api", label: "Review fixture", provider: "claude", kind: "api_key", folder: join(paths.accounts, "review-api"), spendingCapUsd: 1 });
    await accounts.setApiKey("review-api", "synthetic-fixture");
  }
  await accounts.checkAll(); read.mockClear();
  const bot = createBot(db, paths.bots, { name: "Reviewer", provider: "claude", model: "claude-sonnet-4-6", instructions: "Review", color: "blue", shape: "circle" });
  if (api) updateBot(db, bot.id, { accountId: "review-api" });
  const broadcast = vi.fn();
  const context = { botId: bot.id, projectId: "project", taskId: "task", attemptId: "attempt", evaluationId: "evaluation", revision: 1 };
  return { db, accounts, read, broadcast, context, resolve: reviewAccountResolver(db, accounts, broadcast) };
}

it("keeps subscription reviews unavailable before reading credentials", async () => {
  const h = await fixture(false);
  expect(await h.resolve(h.context, new AbortController().signal)).toHaveProperty("unavailable");
  expect(h.read).not.toHaveBeenCalled();
});

it("charges review events once, refreshes the remaining cap, and releases the account lease", async () => {
  const h = await fixture();
  const account = await h.resolve(h.context, new AbortController().signal);
  if ("unavailable" in account) throw new Error(account.unavailable);
  expect(account.getMaxBudgetUsd()).toBe(1);
  expect(h.accounts.inUse("review-api")).toBe(true);
  const event = { type: "usage", model: "claude-sonnet-4-6", input: 2, cachedInput: 0, output: 1, reasoning: 0, costUsd: 0.2, eventId: "acceptance:1" } as const;
  account.onEvent(event); account.onEvent(event);
  expect(account.getMaxBudgetUsd()).toBeCloseTo(0.8);
  expect(h.db.prepare("SELECT COUNT(*) AS count FROM usage").get()).toEqual({ count: 1 });
  expect(h.broadcast).toHaveBeenCalledOnce();
  await account.release(); await account.release();
  expect(h.accounts.inUse("review-api")).toBe(false);
});

it("does not acquire a lease after cancellation", async () => {
  const h = await fixture(), controller = new AbortController(); controller.abort();
  await expect(h.resolve(h.context, controller.signal)).rejects.toThrow();
  expect(h.read).not.toHaveBeenCalled();
  expect(h.accounts.inUse("review-api")).toBe(false);
});
