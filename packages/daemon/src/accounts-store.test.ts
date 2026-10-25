import { afterEach, beforeEach, expect, it } from "vitest";
import { join } from "node:path";
import { rmSync } from "node:fs";
import { openDb, type Db } from "./db.ts";
import { accountBudget, bindLegacyBots, claimCapNotification, createAccount, getDefaultAccount, getAccount, getFallbackAccount, listAccountLimits, listAccounts, removeAccount, replaceAccountLimits, updateAccount } from "./accounts-store.ts";
import { recordUsage } from "./usage.ts";
import { tempRoot } from "./testing.ts";

let db: Db;
beforeEach(() => { db = openDb(":memory:"); });
afterEach(() => { db.close(); });
const subscription = (id = "main") => createAccount(db, { id, provider: "claude", label: id, kind: "subscription", folder: `test/${id}` });
const key = (id = "key") => createAccount(db, { id, provider: "claude", label: id, kind: "api_key", folder: `test/${id}`, credentialRef: `vault:${id}`, spendingCapUsd: 10 });

it("assigns defaults to accounts inserted after fresh migration without reviving tombstones", () => {
  db.exec(`INSERT INTO accounts (id, provider, label, kind, folder, created_at, deleted_at)
    VALUES ('old', 'claude', 'Removed', 'subscription', 'test/old', '2020-01-01', '2026-01-01');
    INSERT INTO accounts (id, provider, label, kind, folder, created_at)
    VALUES ('claude-main', 'claude', 'Claude', 'subscription', 'test/claude-main', '2026-01-01'),
      ('claude-second', 'claude', 'Second', 'subscription', 'test/claude-second', '2026-02-01'),
      ('codex-main', 'codex', 'Codex', 'subscription', 'test/codex-main', '2026-01-01');
    INSERT INTO bots (id, name, color, shape, instructions, provider, model, autonomy, home_folder, created_at)
    VALUES ('bot', 'Test', 'blue', 'circle', '', 'claude', 'haiku', 'folder', 'synthetic', 'then');`);
  expect(listAccounts(db).every((entry) => !entry.isDefault)).toBe(true);
  bindLegacyBots(db);
  expect(listAccounts(db).filter((entry) => entry.isDefault).map((entry) => entry.id)).toEqual(["claude-main", "codex-main"]);
  expect(getAccount(db, "old", { includeDeleted: true })).toMatchObject({ isDefault: false, deletedAt: "2026-01-01" });
  expect(db.prepare("SELECT account_id FROM bots WHERE id = 'bot'").get()).toEqual({ account_id: "claude-main" });
  updateAccount(db, "claude-second", { isDefault: true });
  bindLegacyBots(db);
  expect(getDefaultAccount(db, "claude").id).toBe("claude-second");
  expect(db.prepare("SELECT account_id FROM bots WHERE id = 'bot'").get()).toEqual({ account_id: "claude-main" });
});

it("keeps defaults deterministic and tombstones without removing history metadata", () => {
  subscription();
  subscription("other");
  expect(getDefaultAccount(db, "claude").id).toBe("main");
  removeAccount(db, "main");
  expect(() => getAccount(db, "main")).toThrow();
  expect(getAccount(db, "main", { includeDeleted: true }).folder).toBe("test/main");
  expect(getDefaultAccount(db, "claude").id).toBe("other");
  expect(listAccounts(db)).toHaveLength(1);
  expect(listAccounts(db, { includeDeleted: true })).toHaveLength(2);
  removeAccount(db, "main");
});

it("allows one active API fallback and requires a positive finite cap", () => {
  subscription();
  key();
  key("other");
  updateAccount(db, "key", { isFallback: true });
  updateAccount(db, "other", { isFallback: true });
  expect(getFallbackAccount(db, "claude")?.id).toBe("other");
  expect(getAccount(db, "key").isFallback).toBe(false);
  expect(() => updateAccount(db, "main", { isFallback: true })).toThrow();
  for (const cap of [null, 0, -1, Infinity, NaN]) expect(() => updateAccount(db, "key", { spendingCapUsd: cap })).toThrow();
});

it("changes defaults atomically and refuses removal of pinned accounts", () => {
  subscription();
  subscription("second");
  updateAccount(db, "second", { isDefault: true });
  expect(getDefaultAccount(db, "claude").id).toBe("second");
  expect(getAccount(db, "main").isDefault).toBe(false);
  db.prepare(`INSERT INTO bots (id, name, color, shape, instructions, provider, model, autonomy, home_folder, created_at)
    VALUES ('bot', 'Test', 'blue', 'circle', '', 'claude', 'haiku', 'folder', 'synthetic', 'then')`).run();
  bindLegacyBots(db);
  expect(db.prepare("SELECT account_id FROM bots WHERE id = 'bot'").get()).toEqual({ account_id: "second" });
  expect(() => removeAccount(db, "second")).toThrow(/Move/);
  updateAccount(db, "main", { isDefault: true });
  bindLegacyBots(db);
  expect(db.prepare("SELECT account_id FROM bots WHERE id = 'bot'").get()).toEqual({ account_id: "second" });
  expect(() => createAccount(db, { id: "main", provider: "claude", label: "Duplicate", kind: "subscription", folder: "test/duplicate", isDefault: true })).toThrow();
  expect(getDefaultAccount(db, "claude").id).toBe("main");
});

it("keeps account limit windows separate and ignores older snapshots", () => {
  subscription();
  const observedAt = "2026-10-22T12:00:00.000Z";
  replaceAccountLimits(db, { accountId: "main", limitId: "codex", observedAt, windows: [
    { windowKey: "primary", durationMinutes: 300, usedPercent: 10, resetsAt: "2026-10-22T13:00:00.000Z", state: "ok" },
    { windowKey: "secondary", durationMinutes: 10080, usedPercent: 100, resetsAt: "2026-10-28T13:00:00.000Z", state: "reached" },
  ] });
  expect(getAccount(db, "main").limitedUntil).toBe("2026-10-28T13:00:00.000Z");
  replaceAccountLimits(db, { accountId: "main", limitId: "codex", observedAt: "2026-10-21T12:00:00.000Z", windows: [] });
  expect(listAccountLimits(db, "main")).toHaveLength(2);
  replaceAccountLimits(db, { accountId: "main", limitId: "codex", observedAt: "2026-10-22T13:00:00.000Z", windows: [
    { windowKey: "secondary", durationMinutes: null, usedPercent: 100, resetsAt: null, state: "reached" },
  ] });
  expect(listAccountLimits(db, "main")).toMatchObject([{ state: "reached", resetsAt: null }]);
  expect(getAccount(db, "main").limitedUntil).toBeNull();
  replaceAccountLimits(db, { accountId: "main", limitId: "codex", observedAt: "2026-10-22T14:00:00.000Z", windows: [] });
  replaceAccountLimits(db, { accountId: "main", limitId: "codex", observedAt: "2026-10-22T13:30:00.000Z", windows: [
    { windowKey: "secondary", durationMinutes: null, usedPercent: 100, resetsAt: null, state: "reached" },
  ] });
  expect(listAccountLimits(db, "main")).toEqual([]);
});

it("uses lifetime local spend, blocks unknown costs, and claims each cap threshold once per revision", () => {
  key();
  expect(accountBudget(db, "key").blocked).toBe(false);
  recordUsage(db, { botId: "deleted-bot", accountId: "key", provider: "claude", model: "m", input: 1, cachedInput: 0, output: 1, reasoning: 0, costUsd: 8, costSource: "reported" });
  expect(claimCapNotification(db, "key", 80)).toBe(true);
  expect(claimCapNotification(db, "key", 80)).toBe(false);
  expect(claimCapNotification(db, "key", 100)).toBe(false);
  recordUsage(db, { botId: "deleted-bot", accountId: "key", provider: "claude", model: "m", input: 1, cachedInput: 0, output: 1, reasoning: 0, costUsd: null, costSource: "unknown" });
  expect(accountBudget(db, "key")).toMatchObject({ knownUsd: 8, unknownRows: 1, blocked: true });
  expect(claimCapNotification(db, "key", 100)).toBe(true);
  updateAccount(db, "key", { spendingCapUsd: 20 });
  expect(claimCapNotification(db, "key", 100)).toBe(true);
  removeAccount(db, "key");
  expect(db.prepare("SELECT COUNT(*) AS n FROM usage").get()).toEqual({ n: 2 });
});

it("preserves cap notification claims across restart without resetting lifetime spend", () => {
  const root = tempRoot();
  const file = join(root, "test.db");
  db.close();
  db = openDb(file);
  try {
    key();
    recordUsage(db, { id: "cost-event", botId: "gone", accountId: "key", provider: "claude", model: "m", input: 1, cachedInput: 0, output: 1, reasoning: 0, costUsd: 10, costSource: "reported", createdAt: "2020-01-01T00:00:00Z" });
    expect(claimCapNotification(db, "key", 100)).toBe(true);
    db.close();
    db = openDb(file);
    expect(accountBudget(db, "key")).toEqual({ knownUsd: 10, unknownRows: 0, capUsd: 10, blocked: true });
    expect(claimCapNotification(db, "key", 100)).toBe(false);
    updateAccount(db, "key", { label: "Renamed", spendingCapUsd: 10 });
    expect(claimCapNotification(db, "key", 100)).toBe(false);
    updateAccount(db, "key", { spendingCapUsd: 5 });
    expect(claimCapNotification(db, "key", 100)).toBe(true);
  } finally {
    db.close();
    db = openDb(":memory:");
    rmSync(root, { recursive: true, force: true });
  }
});

it("rejects invalid quota replacements without losing the last valid account state", () => {
  subscription();
  subscription("other");
  const input = { accountId: "main", limitId: "model-1", observedAt: "2026-10-22T12:00:00Z", windows: [
    { windowKey: "primary", durationMinutes: 300, usedPercent: 100, resetsAt: "2026-10-22T14:00:00Z", state: "reached" as const },
  ] };
  replaceAccountLimits(db, input);
  expect(() => replaceAccountLimits(db, { ...input, observedAt: "2026-10-22T13:00:00Z", windows: [{ ...input.windows[0], usedPercent: 101 }] })).toThrow();
  expect(listAccountLimits(db, "main")).toMatchObject([{ usedPercent: 100, limitId: "model-1" }]);
  replaceAccountLimits(db, { ...input, limitId: "model-2", windows: [{ ...input.windows[0], windowKey: "weekly", resetsAt: "2026-10-28T00:00:00Z" }] });
  expect(getAccount(db, "main").limitedUntil).toBe("2026-10-28T00:00:00.000Z");
  expect(listAccountLimits(db, "other")).toEqual([]);
});
