import { randomUUID } from "node:crypto";
import { mkdirSync, lstatSync, realpathSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { DatabaseSync } from "node:sqlite";
import { LoginEntry, type LoginEntry as Entry } from "../../protocol/src/vault.ts";
import { credentialNamespace, windowsCredentialTransport, type CredentialTransport } from "./credential-transport.ts";

export type VaultSecret = { username: string; password: string };
export interface VaultCredentials {
  write(reference: string, secret: VaultSecret): Promise<void>;
  read(reference: string): Promise<VaultSecret | null>;
  delete(reference: string): Promise<void>;
}
export const vaultStorageError = (): Error => new Error("Logins could not access its protected storage. No secret was returned.");
export function validateVaultSecret(value: VaultSecret): void {
  if (!value.username || !value.password || /[\u0000\r\n]/.test(value.username + value.password)
    || Buffer.byteLength(value.username, "utf8") > 320 || Buffer.byteLength(JSON.stringify(value), "utf8") > 2500) {
    throw new Error("Enter a username and password without line breaks, within the protected storage limit.");
  }
}

/** Only the owner desktop uses this namespace. No OS credential enumeration is needed. */
export class WindowsLoginCredentials implements VaultCredentials {
  readonly #namespace: string;
  readonly #transport: CredentialTransport;
  constructor(dataRoot: string, transport: CredentialTransport = windowsCredentialTransport) {
    this.#namespace = credentialNamespace(dataRoot);
    this.#transport = transport;
  }
  #target(reference: string): string {
    if (!/^[0-9a-f-]{36}$/.test(reference)) throw vaultStorageError();
    return `Tjommie/${this.#namespace}/login/${reference}`;
  }
  async write(reference: string, secret: VaultSecret): Promise<void> {
    validateVaultSecret(secret);
    try { await this.#transport({ operation: "write", target: this.#target(reference), key: JSON.stringify(secret) }); }
    catch { throw vaultStorageError(); }
  }
  async read(reference: string): Promise<VaultSecret | null> {
    try {
      const raw = await this.#transport({ operation: "read", target: this.#target(reference) });
      if (raw === null) return null;
      const parsed: unknown = JSON.parse(raw);
      if (!parsed || typeof parsed !== "object" || !("username" in parsed) || typeof parsed.username !== "string" || !("password" in parsed) || typeof parsed.password !== "string") throw vaultStorageError();
      const secret = { username: parsed.username, password: parsed.password }; validateVaultSecret(secret); return secret;
    } catch { throw vaultStorageError(); }
  }
  async delete(reference: string): Promise<void> {
    try { await this.#transport({ operation: "delete", target: this.#target(reference) }); }
    catch { throw vaultStorageError(); }
  }
}

/** Metadata and opaque references only. Pending deletion survives crashes and failed OS writes. */
export class VaultRecords {
  readonly #db: DatabaseSync;
  constructor(path: string) {
    if (path !== ":memory:") {
      const parent = dirname(resolve(path)); mkdirSync(parent, { recursive: true });
      if (lstatSync(parent).isSymbolicLink() || realpathSync(parent).toLowerCase() !== parent.toLowerCase()) throw vaultStorageError();
      try { if (lstatSync(path).isSymbolicLink()) throw vaultStorageError(); } catch (error) { if (!(error && typeof error === "object" && "code" in error && error.code === "ENOENT")) throw error; }
    }
    this.#db = new DatabaseSync(path);
    this.#db.exec("PRAGMA journal_mode=WAL; PRAGMA trusted_schema=OFF; CREATE TABLE IF NOT EXISTS logins (id TEXT PRIMARY KEY, data TEXT NOT NULL, reference TEXT NOT NULL UNIQUE); CREATE TABLE IF NOT EXISTS login_deletions (reference TEXT PRIMARY KEY);");
  }
  list(botId?: string): Entry[] {
    const entries = (this.#db.prepare("SELECT data FROM logins ORDER BY rowid").all() as { data: string }[]).map((row) => LoginEntry.parse(JSON.parse(row.data)));
    return botId === undefined ? entries : entries.filter((entry) => entry.botId === botId);
  }
  get(id: string, revision?: number): { entry: Entry; reference: string } {
    const row = this.#db.prepare("SELECT data,reference FROM logins WHERE id=?").get(id) as { data: string; reference: string } | undefined;
    if (!row) throw new Error("This login no longer exists.");
    const entry = LoginEntry.parse(JSON.parse(row.data));
    if (revision !== undefined && entry.revision !== revision) throw new Error("This login changed. Refresh Logins before continuing.");
    return { entry, reference: row.reference };
  }
  pending(): string[] { return (this.#db.prepare("SELECT reference FROM login_deletions").all() as { reference: string }[]).map((row) => row.reference); }
  prepare(): string { const reference = randomUUID(); this.#db.prepare("INSERT INTO login_deletions VALUES (?)").run(reference); return reference; }
  saved(entry: Entry, reference: string, previousRevision?: number): void {
    entry = LoginEntry.parse(entry);
    this.#db.exec("BEGIN IMMEDIATE");
    try {
      if (previousRevision !== undefined) {
        const old = this.get(entry.id, previousRevision);
        this.#db.prepare("INSERT OR IGNORE INTO login_deletions VALUES (?)").run(old.reference);
        this.#db.prepare("UPDATE logins SET data=?,reference=? WHERE id=?").run(JSON.stringify(entry), reference, entry.id);
      } else {
        if (this.list().length >= 1000) throw new Error("Logins has reached its entry limit.");
        this.#db.prepare("INSERT INTO logins VALUES (?,?,?)").run(entry.id, JSON.stringify(entry), reference);
      }
      this.#db.prepare("DELETE FROM login_deletions WHERE reference=?").run(reference); this.#db.exec("COMMIT");
    } catch (error) { this.#db.exec("ROLLBACK"); throw error; }
  }
  deleting(id: string, revision: number): void {
    const { reference } = this.get(id, revision);
    this.#db.exec("BEGIN IMMEDIATE");
    try {
      this.#db.prepare("INSERT OR IGNORE INTO login_deletions VALUES (?)").run(reference);
      this.#db.prepare("DELETE FROM logins WHERE id=?").run(id); this.#db.exec("COMMIT");
    } catch (error) { this.#db.exec("ROLLBACK"); throw error; }
  }
  deleted(reference: string): void { this.#db.prepare("DELETE FROM login_deletions WHERE reference=?").run(reference); }
  close(): void { this.#db.close(); }
}
