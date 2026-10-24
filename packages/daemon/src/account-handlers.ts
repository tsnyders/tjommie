import { mkdirSync } from "node:fs";
import { join } from "node:path";
import type { AccountInfo } from "@tjommie/protocol";
import type { AccountRuntime } from "./account-runtime.ts";
import { createAccount, getAccount, removeAccount, updateAccount } from "./accounts-store.ts";
import { newId, type Db } from "./db.ts";
import { RpcError, type Handlers } from "./server.ts";
import { queryUsage } from "./usage.ts";

export type AccountService = Pick<AccountRuntime, "info" | "list" | "check" | "checkAll" | "retryLimit" | "signIn" | "cancelSignIn" | "prepareSandbox" | "setApiKey" | "deleteCredential" | "changed" | "metadataChanged" | "hold" | "inUse">;

export function accountHandlers(options: {
  db: Db;
  accountsDir: string;
  runtime: AccountService;
  changing: Set<string>;
  protect(): Promise<void>;
}): Handlers {
  const { db, runtime, changing } = options;
  async function change<T>(id: string, operation: () => Promise<T>, idle = false, hold = true): Promise<T> {
    if (changing.has(id)) throw new RpcError("busy", "This account is already changing. Try again shortly.");
    if (idle) notRunning(id);
    changing.add(id);
    let release: (() => void) | undefined;
    try { if (hold) release = runtime.hold(id); return await operation(); }
    finally { changing.delete(id); release?.(); }
  }
  function notRunning(id: string): void {
    if (runtime.inUse(id)) throw new RpcError("busy", "Stop the bots using this account before changing its sign-in or key.");
  }
  const info = (id: string): AccountInfo => runtime.info(id);
  return {
    "accounts.list": () => runtime.list(),
    "accounts.check": ({ id }) => id ? runtime.check(id).then(() => runtime.list()) : runtime.checkAll(),
    "accounts.retryLimit": ({ id }) => change(id, async () => runtime.retryLimit(id), true),
    "accounts.create": async (input) => {
      const id = newId();
      const folder = join(options.accountsDir, id);
      mkdirSync(folder, { recursive: true });
      createAccount(db, { id, folder, provider: input.provider, label: input.label, kind: input.kind,
        ...(input.kind === "api_key" ? { spendingCapUsd: input.spendingCapUsd, isFallback: input.isFallback } : {}) });
      return change(id, async () => {
        try {
          await options.protect();
          if (input.kind === "api_key") await runtime.setApiKey(id, input.apiKey);
          runtime.changed(id);
          return info(id);
        } catch {
          // Retain discoverable metadata if vault cleanup fails, so the owner can retry removal.
          try { await runtime.deleteCredential(id); removeAccount(db, id); } catch { runtime.changed(id); }
          throw new RpcError("bad_state", "Account setup did not finish. Check Accounts before trying again.");
        }
      });
    },
    "accounts.update": ({ id, patch }) => change(id, async () => {
      const account = getAccount(db, id);
      const { apiKey, ...metadata } = patch;
      if (apiKey !== undefined) {
        if (account.kind !== "api_key") throw new RpcError("bad_params", "Only API-key accounts can store an API key.");
        if (Object.keys(metadata).length) throw new RpcError("bad_params", "Save a replacement key separately from account settings.");
        notRunning(id);
        await runtime.setApiKey(id, apiKey);
      } else {
        updateAccount(db, id, metadata);
        runtime.metadataChanged(id);
      }
      if (apiKey !== undefined) runtime.changed(id);
      return info(id);
    }, patch.apiKey !== undefined, patch.apiKey !== undefined),
    "accounts.remove": ({ id }) => change(id, async () => {
      getAccount(db, id);
      notRunning(id);
      if (db.prepare("SELECT id FROM bots WHERE account_id = ? LIMIT 1").get(id)) {
        throw new RpcError("busy", "Choose another account for the bots assigned to this account before removing it.");
      }
      await runtime.deleteCredential(id);
      removeAccount(db, id);
      runtime.changed(id);
      return {};
    }, true),
    "accounts.signIn": ({ id }) => change(id, async () => {
      notRunning(id);
      await options.protect();
      return runtime.signIn(id);
    }, true),
    "accounts.cancelSignIn": ({ id, attempt }) => change(id, async () => {
      await runtime.cancelSignIn(id, attempt);
      return {};
    }),
    "accounts.prepareCodexSandbox": ({ id }) => change(id, async () => {
      notRunning(id);
      try { await runtime.prepareSandbox(id); }
      finally { await options.protect(); }
      return info(id);
    }, true),
    "usage.query": (query) => queryUsage(db, query),
  };
}
