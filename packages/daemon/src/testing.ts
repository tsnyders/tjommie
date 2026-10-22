import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { Provider } from "@tjommie/protocol";
import { dataPaths } from "./paths.ts";
import type { Account } from "./setup.ts";

// Helpers shared by the *.test.ts and *.real.test.ts files. Not used by the daemon itself.

// A fresh folder whose path has a space and an accented letter, like many Windows profile paths.
export const tempRoot = (): string => mkdtempSync(join(tmpdir(), "tjommie test ñ-"));

// Use the real Tjommie accounts so CLI credential refreshes stay where the daemon reads them.
// Tests never read or copy credential contents. Databases, bots and browser profiles stay disposable.
export const realAccountsDir = (): string => dataPaths().accounts;

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
