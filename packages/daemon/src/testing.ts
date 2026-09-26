import { mkdirSync, mkdtempSync, symlinkSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import type { Provider } from "@tjommie/protocol";
import type { Account } from "./setup.ts";

// Helpers shared by the *.test.ts and *.real.test.ts files. Not used by the daemon itself.

// A fresh folder whose path has a space and an accented letter, like many Windows profile paths.
export const tempRoot = (): string => mkdtempSync(join(tmpdir(), "tjommie test ñ-"));

// Each test worker gets disposable config and session storage. Only the official CLIs access the
// existing sign-in files through symlinks. Tests never read or copy credential contents.
let isolatedAccounts: string | undefined;
export const realAccountsDir = (): string => {
  if (isolatedAccounts) return isolatedAccounts;
  const accounts = join(tempRoot(), "accounts");
  const signedInRoot = process.env.TJOMMIE_HOME ?? join(process.env.LOCALAPPDATA ?? join(homedir(), ".local", "share"), "Tjommie");
  for (const [provider, filename] of [["claude", ".credentials.json"], ["codex", "auth.json"]] as const) {
    const folder = join(accounts, `${provider}-main`);
    mkdirSync(folder, { recursive: true });
    symlinkSync(join(signedInRoot, "accounts", `${provider}-main`, filename), join(folder, filename), "file");
  }
  isolatedAccounts = accounts;
  return accounts;
};

export const realAccount = (provider: Provider): Account => ({
  id: `${provider}-main`,
  provider,
  label: provider === "claude" ? "Claude" : "Codex",
  folder: join(realAccountsDir(), `${provider}-main`),
});

export async function waitFor<T>(check: () => T | null | undefined | false, timeoutMs = 180_000): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = check();
    if (value) return value;
    if (Date.now() > deadline) throw new Error(`waitFor timed out after ${timeoutMs} ms`);
    await new Promise((resolve) => setTimeout(resolve, 200));
  }
}

export function firstOf<E extends { type: string }, T extends E["type"]>(events: E[], type: T): Extract<E, { type: T }> | undefined {
  return events.find((event): event is Extract<E, { type: T }> => event.type === type);
}
