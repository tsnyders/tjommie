import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { afterEach, expect, it, vi } from "vitest";
import { NativeLoginVault, type VaultBrowser, type VaultEntryInput } from "./login-vault.ts";
import { VaultRecords, type VaultSecret, WindowsLoginCredentials } from "./vault-storage.ts";
import type { LoginEntry, VaultHostAction } from "../../protocol/src/vault.ts";

const roots: string[] = [], records: VaultRecords[] = [];
afterEach(() => { for (const record of records.splice(0)) record.close(); for (const root of roots.splice(0)) { if (resolve(root).startsWith(join(tmpdir(), "tjommie-vault-test-"))) rmSync(root, { recursive: true, force: true }); } });
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>((done) => { resolve = done; }); return { promise, resolve }; }
const unique = "Synthetic-Login-Secret-ABSENT-FROM-OUTPUT-7d14";
function fixture() {
  const root = mkdtempSync(join(tmpdir(), "tjommie-vault-test-")); roots.push(root);
  const path = join(root, "logins.sqlite"), record = new VaultRecords(path); records.push(record);
  const store = new Map<string, VaultSecret>(), sinks: unknown[] = [];
  let available = true, held = 0, privateActive = false;
  const credentials = {
    read: vi.fn(async (reference: string) => { const value = store.get(reference); return value ? { ...value } : null; }),
    write: vi.fn(async (reference: string, secret: VaultSecret) => { store.set(reference, { ...secret }); }),
    delete: vi.fn(async (reference: string) => { store.delete(reference); }),
  };
  const entry = {
    entry: vi.fn(async (): Promise<VaultEntryInput | null> => ({ name: "Synthetic account", origin: "https://login.invalid", secret: { username: "synthetic@example.invalid", password: unique } })),
    confirm: vi.fn(async () => true),
  };
  const browser: VaultBrowser = {
    destination: vi.fn(() => "https://login.invalid"), begin: vi.fn(async () => { privateActive = true; return 4; }),
    fill: vi.fn(async (_id, _epoch, _secret, check) => { check(); }), finish: vi.fn(async (_id, _generation, _epoch, _mode, check) => { check(); privateActive = false; }),
  };
  const isolation = {
    unavailableReason: vi.fn(async () => available ? null : "Verified worker unavailable."),
    acquire: vi.fn(async () => { if (!available) throw new Error(unique); held++; let released = false; return { assert: () => { if (!available || released) throw new Error(unique); }, release: () => { if (!released) { held--; released = true; } } }; }),
  };
  const create = () => new NativeLoginVault({ records: record, credentials, entry, browser, isolation });
  const vault = create();
  const call = async (action: VaultHostAction, owner = "local", instance = vault): Promise<unknown> => {
    try { const result = await instance.command({ controllerId: owner, action }); sinks.push(result); return result; }
    catch (error) { sinks.push(error instanceof Error ? error.message : "failure"); throw error; }
  };
  const saved = async (): Promise<LoginEntry> => {
    await call({ kind: "unlock" }); const result = await call({ kind: "entry", botId: "bot" }) as { entry: LoginEntry }; return result.entry;
  };
  const target = (entry: LoginEntry) => ({ kind: "fill" as const, id: entry.id, revision: entry.revision, botId: entry.botId, generation: "g", pageId: "p", controlEpoch: 1 });
  return { root, path, record, store, sinks, credentials, entry, browser, isolation, vault, create, call, saved, target,
    unavailable: () => { available = false; }, private: () => privateActive, held: () => held };
}

it("does not offer secret entry or reads without a real isolation lease", async () => {
  const h = fixture(); h.unavailable();
  expect(await h.call({ kind: "status" })).toMatchObject({ state: "unavailable" });
  await expect(h.call({ kind: "unlock" })).rejects.toThrow("could not finish");
  expect(h.entry.confirm).not.toHaveBeenCalled(); expect(h.entry.entry).not.toHaveBeenCalled(); expect(h.credentials.read).not.toHaveBeenCalled();
  expect(JSON.stringify(h.sinks)).not.toContain(unique);
});

it("stores metadata and opaque references, starts locked after restart, and cleans replaced OS secrets", async () => {
  const h = fixture(), entry = await h.saved();
  expect(h.store.size).toBe(1); expect(h.held()).toBe(0);
  expect(readFileSync(h.path).includes(Buffer.from(unique))).toBe(false);
  expect(readFileSync(h.path + "-wal").includes(Buffer.from(unique))).toBe(false);
  const edited = await h.call({ kind: "entry", botId: "bot", id: entry.id, revision: entry.revision }) as { entry: LoginEntry };
  expect(edited.entry.revision).toBe(2); expect(h.store.size).toBe(1); expect(h.credentials.delete).toHaveBeenCalledTimes(1);
  const restarted = h.create(); expect(await h.call({ kind: "status" }, "local", restarted)).toMatchObject({ state: "locked" });
  await expect(h.call(h.target(edited.entry), "local", restarted)).rejects.toThrow("could not finish");
  expect(h.credentials.read).not.toHaveBeenCalled(); expect(JSON.stringify(h.sinks)).not.toContain(unique);
});

it("confirms exact origin before reading, fills privately, and emits no secret into operation results or errors", async () => {
  const h = fixture(), entry = await h.saved();
  vi.mocked(h.browser.destination).mockResolvedValueOnce("https://other.invalid");
  await expect(h.call(h.target(entry))).rejects.toThrow("could not finish"); expect(h.credentials.read).not.toHaveBeenCalled();
  h.credentials.read.mockImplementationOnce(async (reference) => { expect(h.private()).toBe(true); return { ...h.store.get(reference)! }; });
  expect(await h.call(h.target(entry))).toEqual({ status: "filled", privateEpoch: 4 });
  expect(h.browser.fill).toHaveBeenCalledWith("bot", 4, { username: "", password: "" }, expect.any(Function));
  expect(h.private()).toBe(true);
  await h.call({ kind: "lock" }); expect(h.private()).toBe(true);
  expect(await h.call({ kind: "finish", botId: "bot", generation: "g", privateEpoch: 4, mode: "clear" })).toEqual({ status: "cleared" });
  expect(h.private()).toBe(false); expect(JSON.stringify(h.sinks)).not.toContain(unique);
});

it("rechecks destination after confirmation and refuses a wrong profile without secret access", async () => {
  const h = fixture(), entry = await h.saved();
  vi.mocked(h.browser.destination).mockResolvedValueOnce(entry.origin).mockResolvedValueOnce("https://navigation.invalid");
  await expect(h.call(h.target(entry))).rejects.toThrow("could not finish");
  await expect(h.call({ ...h.target(entry), botId: "another-bot" })).rejects.toThrow("could not finish");
  expect(h.credentials.read).not.toHaveBeenCalled(); expect(h.browser.begin).not.toHaveBeenCalled();
});

it("cancels an entry arriving after lock and clears the dedicated input values without saving", async () => {
  const h = fixture(), waiting = deferred<VaultEntryInput | null>();
  await h.call({ kind: "unlock" }); h.entry.entry.mockReturnValueOnce(waiting.promise);
  const pending = h.call({ kind: "entry", botId: "bot" }); await vi.waitFor(() => expect(h.entry.entry).toHaveBeenCalled());
  await h.call({ kind: "lock" });
  const input = { name: "Late entry", origin: "https://login.invalid", secret: { username: "synthetic", password: unique } }; waiting.resolve(input);
  await expect(pending).rejects.toThrow("could not finish");
  expect(input.secret).toEqual({ username: "", password: "" }); expect(h.credentials.write).not.toHaveBeenCalled();
});

it("fences a late OS read when lock or isolation loss occurs and keeps its browser private", async () => {
  const h = fixture(), entry = await h.saved(), waiting = deferred<VaultSecret | null>();
  h.credentials.read.mockReturnValueOnce(waiting.promise);
  const pending = h.call(h.target(entry)); await vi.waitFor(() => expect(h.credentials.read).toHaveBeenCalled());
  h.unavailable(); await h.call({ kind: "lock" });
  const secret = { username: "synthetic", password: unique }; waiting.resolve(secret);
  await expect(pending).rejects.toThrow("could not finish"); expect(secret.password).toBe("");
  expect(h.browser.fill).not.toHaveBeenCalled(); expect(h.private()).toBe(true); expect(h.held()).toBe(0);
  expect(JSON.stringify(h.sinks)).not.toContain(unique);
});

it("tombstones a disconnected RPC controller before a delayed fill starts, while cancel permits a new operation", async () => {
  const h = fixture(), entry = await h.saved();
  await h.call({ kind: "disconnect" }, "rpc:retired");
  await expect(h.call(h.target(entry), "rpc:retired")).rejects.toThrow("cancelled");
  expect(h.browser.destination).not.toHaveBeenCalled(); expect(h.credentials.read).not.toHaveBeenCalled();
  await h.call({ kind: "cancel" }, "rpc:current");
  expect(await h.call(h.target(entry), "rpc:current")).toMatchObject({ status: "filled" });
});

it("journals failed secret deletion across restart without removing a browser profile or reporting success", async () => {
  const h = fixture(), entry = await h.saved(); await h.call(h.target(entry));
  h.credentials.delete.mockRejectedValueOnce(new Error(unique));
  expect(await h.call({ kind: "delete", id: entry.id, revision: entry.revision })).toEqual({ status: "pending" });
  expect(await h.call({ kind: "list" })).toEqual([]); expect(h.store.size).toBe(1); expect(h.private()).toBe(true);
  const restarted = h.create(); expect(await restarted.status()).toMatchObject({ state: "locked", pendingDeletes: 1 });
  await h.call({ kind: "unlock" }, "local", restarted);
  expect(await restarted.status()).toMatchObject({ state: "unlocked", pendingDeletes: 0 }); expect(h.store.size).toBe(0);
  expect(h.browser.finish).not.toHaveBeenCalled(); expect(JSON.stringify(h.sinks)).not.toContain(unique);
});

it("uses a separate login namespace and permits website credentials that are not API keys", async () => {
  const values = new Map<string, string>(), touched: string[] = [];
  const credentials = new WindowsLoginCredentials("C:\\synthetic-vault", async (input) => { touched.push(input.target); if (input.operation === "write") values.set(input.target, input.key!); if (input.operation === "delete") values.delete(input.target); return input.operation === "read" ? values.get(input.target) ?? null : null; });
  const reference = "00000000-0000-4000-8000-000000000123", secret = { username: "Person With Spaces", password: unique + " with spaces" };
  await credentials.write(reference, secret); expect(await credentials.read(reference)).toEqual(secret); await credentials.delete(reference);
  expect(touched.every((target) => /^Tjommie\/[a-f0-9]{24}\/login\/[a-f0-9-]{36}$/.test(target))).toBe(true); expect(values.size).toBe(0);
});
