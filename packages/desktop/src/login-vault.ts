import { randomUUID } from "node:crypto";
import { LoginEntry, VaultHostCommand, type LoginEntry as Entry, type VaultFillTarget, type VaultStatus } from "../../protocol/src/vault.ts";
import { validateVaultSecret, VaultRecords, type VaultCredentials, type VaultSecret } from "./vault-storage.ts";

export type VaultIsolationLease = { assert(): void; release(): void };
export type VaultIsolation = { unavailableReason(): Promise<string | null>; acquire(): Promise<VaultIsolationLease> };
export type VaultEntryInput = { name: string; origin: string; secret: VaultSecret };
export type VaultConfirmation = { kind: "unlock" | "delete" | "fill" | "finish"; entry?: Entry; destination?: string; mode?: "clear" | "close" };
export type TrustedVaultEntry = {
  entry(botId: string, previous: Entry | null, signal: AbortSignal): Promise<VaultEntryInput | null>;
  confirm(request: VaultConfirmation, signal: AbortSignal): Promise<boolean>;
};
export type VaultBrowser = {
  destination(target: VaultFillTarget, owner: string): string | Promise<string>;
  begin(target: VaultFillTarget, owner: string, origin: string, check: () => void): Promise<number>;
  fill(botId: string, privateEpoch: number, secret: VaultSecret, check: () => void): Promise<void>;
  finish(botId: string, generation: string, privateEpoch: number, mode: "clear" | "close", check: () => void): Promise<void>;
};
type Options = { records: VaultRecords; credentials: VaultCredentials; isolation: VaultIsolation; entry: TrustedVaultEntry; browser: VaultBrowser };
const locked = (): Error => new Error("Unlock Logins on the host before using a saved login.");
const changed = (): Error => new Error("The Logins operation was cancelled or its security boundary changed.");

/** Secrets stay between the dedicated native entry surface, the OS store and the native fill seam. */
export class NativeLoginVault {
  readonly #options: Options;
  #locked = true;
  #epoch = 0;
  #operation: { owner: string; abort: AbortController } | null = null;
  readonly #closedControllers = new Set<string>();
  #remoteClosed = false;
  #closed = false;
  constructor(options: Options) { this.#options = options; }
  async status(): Promise<VaultStatus> {
    const reason = await this.#options.isolation.unavailableReason();
    return { state: reason ? "unavailable" : this.#locked ? "locked" : "unlocked", reason, pendingDeletes: this.#options.records.pending().length };
  }
  lock(): void { this.#locked = true; this.#epoch++; this.#operation?.abort.abort(); }
  async #cleanup(check: () => void): Promise<void> {
    for (const reference of this.#options.records.pending()) {
      check();
      try { await this.#options.credentials.delete(reference); check(); this.#options.records.deleted(reference); }
      catch { check(); }
    }
  }
  async command(raw: VaultHostCommand): Promise<unknown> {
    const { controllerId: owner, action } = VaultHostCommand.parse(raw);
    if (this.#closed) throw changed();
    if (action.kind === "cancel") { if (this.#operation?.owner === owner) this.#operation.abort.abort(); return {}; }
    if (action.kind === "disconnect") {
      if (owner.startsWith("rpc:")) {
        if (this.#closedControllers.size >= 4096) this.#remoteClosed = true;
        else this.#closedControllers.add(owner);
      }
      if (this.#operation?.owner === owner) this.#operation.abort.abort(); return {};
    }
    if (owner.startsWith("rpc:") && (this.#remoteClosed || this.#closedControllers.has(owner))) throw changed();
    if (action.kind === "lock") { this.lock(); return this.status(); }
    if (action.kind === "status") return this.status();
    if (action.kind === "list") return this.#options.records.list(action.botId);
    if (this.#operation) throw new Error("Finish the current Logins operation on the host first.");
    const operation = { owner, abort: new AbortController() }, epoch = this.#epoch;
    this.#operation = operation;
    let lease: VaultIsolationLease | undefined;
    const check = (): void => {
      if (this.#closed || epoch !== this.#epoch || operation.abort.signal.aborted) throw changed();
      lease?.assert();
    };
    try {
      // Deletion and closing a private browser remain possible if containment becomes unavailable.
      if (action.kind !== "delete" && action.kind !== "finish") lease = await this.#options.isolation.acquire();
      check();
      if (action.kind === "unlock") {
        if (await this.#options.entry.confirm({ kind: "unlock" }, operation.abort.signal)) {
          check(); this.#locked = false; await this.#cleanup(check);
        }
        return this.status();
      }
      if (action.kind === "delete") {
        const { entry } = this.#options.records.get(action.id, action.revision);
        if (!await this.#options.entry.confirm({ kind: "delete", entry }, operation.abort.signal)) return { status: "cancelled" };
        check(); this.#options.records.deleting(entry.id, entry.revision); await this.#cleanup(check);
        return { status: this.#options.records.pending().length ? "pending" : "deleted" };
      }
      if (action.kind === "finish") {
        if (!await this.#options.entry.confirm({ kind: "finish", mode: action.mode }, operation.abort.signal)) return { status: "cancelled" };
        check(); await this.#options.browser.finish(action.botId, action.generation, action.privateEpoch, action.mode, check); check();
        return { status: action.mode === "close" ? "closed" : "cleared" };
      }
      if (this.#locked) throw locked();
      if (action.kind === "entry") {
        if (action.id !== undefined && action.revision === undefined || action.id === undefined && action.revision !== undefined) throw changed();
        const previous = action.id ? this.#options.records.get(action.id, action.revision).entry : null;
        if (previous && previous.botId !== action.botId) throw changed();
        const input = await this.#options.entry.entry(action.botId, previous, operation.abort.signal);
        if (!input) { check(); return { status: "cancelled", entry: null }; }
        let reference: string | undefined;
        try {
          check();
          validateVaultSecret(input.secret);
          const now = new Date().toISOString();
          const entry = LoginEntry.parse({ id: previous?.id ?? randomUUID(), name: input.name, origin: input.origin, botId: action.botId, revision: (previous?.revision ?? 0) + 1, createdAt: previous?.createdAt ?? now, updatedAt: now });
          check(); reference = this.#options.records.prepare();
          await this.#options.credentials.write(reference, input.secret); check();
          this.#options.records.saved(entry, reference, previous?.revision); await this.#cleanup(check);
          return { status: "saved", entry };
        } finally {
          input.secret.username = ""; input.secret.password = "";
          if (reference && this.#options.records.pending().includes(reference)) {
            try { await this.#options.credentials.delete(reference); this.#options.records.deleted(reference); } catch { /* Retry the opaque pending reference after restart. */ }
          }
        }
      }
      if (action.kind === "fill") {
        const { entry, reference } = this.#options.records.get(action.id, action.revision);
        if (entry.botId !== action.botId) throw changed();
        const destination = await this.#options.browser.destination(action, owner); check();
        if (destination !== entry.origin) throw changed();
        if (!await this.#options.entry.confirm({ kind: "fill", entry, destination }, operation.abort.signal)) return { status: "cancelled", privateEpoch: null };
        check(); if (await this.#options.browser.destination(action, owner) !== entry.origin) throw changed(); check();
        const privateEpoch = await this.#options.browser.begin(action, owner, entry.origin, check); check();
        const secret = await this.#options.credentials.read(reference);
        if (!secret) throw changed();
        try { check(); await this.#options.browser.fill(action.botId, privateEpoch, secret, check); check(); }
        finally { secret.username = ""; secret.password = ""; }
        return { status: "filled", privateEpoch };
      }
      throw changed();
    } catch {
      // Never surface CDP, website, OS-store, native-window or provider error payloads.
      throw new Error("Logins could not finish this operation safely. Refresh Logins or check the host. Any private browser stays private until you clear its fields or close it.");
    } finally {
      lease?.release(); if (this.#operation === operation) this.#operation = null;
    }
  }
  close(): void { this.lock(); this.#closed = true; }
}
