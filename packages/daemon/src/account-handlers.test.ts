import { join } from "node:path";
import { afterEach, expect, it, vi } from "vitest";
import type { AccountInfo } from "@tjommie/protocol";
import { accountHandlers, type AccountService } from "./account-handlers.ts";
import { createAccount, getAccount, listAccounts, updateAccount } from "./accounts-store.ts";
import { createBot } from "./bots.ts";
import { openDb } from "./db.ts";
import { tempRoot } from "./testing.ts";
import { accountBudget } from "./usage.ts";

const cleanup: (() => void)[] = [];
afterEach(() => { for (const close of cleanup.splice(0)) close(); });
function fixture() {
  const root = tempRoot(), db = openDb(join(root, "test.db"));
  cleanup.push(() => db.close());
  const keys = new Set<string>(), changing = new Set<string>();
  const info = (id: string): AccountInfo => {
    const account = getAccount(db, id);
    return { id, provider: account.provider, label: account.label, kind: account.kind, isDefault: account.isDefault, isFallback: account.isFallback,
      spendingCapUsd: account.spendingCapUsd, createdAt: account.createdAt, hasCredential: keys.has(id), setup: null, signIn: null, limits: [], spending: accountBudget(db, id) };
  };
  const runtime: AccountService = {
    info, list: () => listAccounts(db).map((account) => info(account.id)), check: async (id: string) => info(id), checkAll: async () => runtime.list(), retryLimit: (id: string) => info(id),
    signIn: async () => ({ url: null, attempt: "synthetic-attempt" }), cancelSignIn: async () => {}, prepareSandbox: async () => {},
    setApiKey: vi.fn(async (id: string, _key: string) => { keys.add(id); updateAccount(db, id, { credentialRef: `synthetic-reference/${id}` }); }),
    deleteCredential: vi.fn(async (id: string) => { keys.delete(id); }), changed: vi.fn(), metadataChanged: vi.fn(),
    hold: (id: string) => { getAccount(db, id); return () => {}; }, inUse: () => false,
  };
  const handlers = accountHandlers({ db, runtime, changing, accountsDir: join(root, "accounts"), protect: async () => {} });
  const account = createAccount(db, { id: "synthetic", provider: "claude", label: "Synthetic", kind: "api_key", spendingCapUsd: 5, folder: join(root, "account") });
  return { root, db, runtime, handlers, account, changing };
}

it("saves only a secret reference and returns safe metadata when creating an API account", async () => {
  const { db, handlers } = fixture();
  const created = await handlers["accounts.create"]!({ provider: "codex", label: "API", kind: "api_key", apiKey: "synthetic-test-key", spendingCapUsd: 10 });
  expect(created.hasCredential).toBe(true);
  const stored = getAccount(db, created.id);
  expect(stored.credentialRef).toMatch(/^synthetic-reference\//);
  expect(JSON.stringify(created)).not.toContain("synthetic-test-key");
  expect(created).not.toHaveProperty("folder");
  expect(db.prepare("SELECT sql FROM sqlite_master WHERE name='accounts'").get()?.sql).not.toContain("api_key_value");
});

it("checks account pins before deleting credentials", async () => {
  const { db, root, account, runtime, handlers } = fixture();
  createBot(db, join(root, "bots"), { name: "Pinned", color: "blue", shape: "circle", provider: "claude", model: "sonnet", instructions: "", accountId: account.id });
  await expect(handlers["accounts.remove"]!({ id: account.id })).rejects.toThrow(/another account/);
  expect(runtime.deleteCredential).not.toHaveBeenCalled();
  expect(getAccount(db, account.id).deletedAt).toBeNull();
});

it("holds the account across asynchronous credential deletion and rejects a competing mutation", async () => {
  const { db, account, runtime, handlers, changing } = fixture();
  let finish: (() => void) | undefined;
  runtime.deleteCredential = () => new Promise<void>((resolve) => { finish = resolve; });
  const removing = handlers["accounts.remove"]!({ id: account.id });
  expect(changing.has(account.id)).toBe(true);
  await expect(handlers["accounts.update"]!({ id: account.id, patch: { label: "Race" } })).rejects.toThrow(/already changing/);
  finish?.();
  await removing;
  expect(changing.size).toBe(0);
  expect(getAccount(db, account.id, { includeDeleted: true }).deletedAt).not.toBeNull();
});

it("keeps the account available for a retry when credential deletion fails", async () => {
  const { db, account, runtime, handlers, changing } = fixture();
  runtime.deleteCredential = async () => { throw new Error("Synthetic credential-store failure"); };
  await expect(handlers["accounts.remove"]!({ id: account.id })).rejects.toThrow(/credential-store failure/);
  expect(getAccount(db, account.id).deletedAt).toBeNull();
  expect(changing.size).toBe(0);
});

it("rejects combined credential/settings updates before changing either", async () => {
  const { db, account, runtime, handlers } = fixture();
  await expect(handlers["accounts.update"]!({ id: account.id, patch: { apiKey: "synthetic", label: "Changed" } })).rejects.toThrow(/separately/);
  expect(runtime.setApiKey).not.toHaveBeenCalled();
  expect(getAccount(db, account.id).label).toBe("Synthetic");
});

it("does not leave a mutation barrier behind when taking the hold fails", async () => {
  const { handlers, changing } = fixture();
  await expect(handlers["accounts.remove"]!({ id: "missing" })).rejects.toThrow(/not found/);
  expect(changing.size).toBe(0);
});

it("renames an in-use account without invalidating its running authentication context", async () => {
  const { handlers, runtime, account } = fixture();
  runtime.inUse = () => true;
  runtime.hold = vi.fn(() => { throw new Error("Metadata must not take an authentication hold"); });
  const renamed = await handlers["accounts.update"]!({ id: account.id, patch: { label: "Renamed during work" } });
  expect(renamed.label).toBe("Renamed during work");
  expect(runtime.hold).not.toHaveBeenCalled();
  expect(runtime.changed).not.toHaveBeenCalled();
  expect(runtime.metadataChanged).toHaveBeenCalledWith(account.id);
});

it("requires an idle account before authorizing one queued-work limit retry", async () => {
  const { handlers, runtime, account } = fixture();
  runtime.retryLimit = vi.fn((id: string) => runtime.info(id));
  runtime.inUse = () => true;
  await expect(handlers["accounts.retryLimit"]!({ id: account.id })).rejects.toThrow(/Stop the bots/);
  expect(runtime.retryLimit).not.toHaveBeenCalled();
  runtime.inUse = () => false;
  expect(await handlers["accounts.retryLimit"]!({ id: account.id })).toMatchObject({ id: account.id });
  expect(runtime.retryLimit).toHaveBeenCalledExactlyOnceWith(account.id);
});
