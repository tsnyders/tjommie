import { existsSync, mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, expect, it, vi } from "vitest";
import { cleanupTempRoots, realAccount, realAccountsDir, tempRoot } from "./testing.ts";

afterEach(() => vi.unstubAllEnvs());

it("removes every tracked temp root when cleanup runs", () => {
  const roots = [tempRoot(), tempRoot()];
  for (const root of roots) expect(existsSync(root)).toBe(true);
  cleanupTempRoots();
  for (const root of roots) expect(existsSync(root)).toBe(false);
});

it("uses the configured Tjommie accounts directly for real CLI tests", () => {
  const root = tempRoot();
  vi.stubEnv("TJOMMIE_HOME", root);
  expect(realAccountsDir()).toBe(join(root, "accounts"));
  for (const provider of ["claude", "codex"] as const) {
    expect(realAccount(provider).folder).toBe(join(root, "accounts", `${provider}-main`));
  }
});

it("keeps an atomic CLI state replacement in the account folder the daemon uses", () => {
  const root = tempRoot();
  vi.stubEnv("TJOMMIE_HOME", root);
  const daemonAccount = join(root, "accounts", "codex-main");
  mkdirSync(daemonAccount, { recursive: true });
  // Generic fake state demonstrates temp-and-rename without touching credential files.
  const filename = "refresh-fixture.json";
  writeFileSync(join(daemonAccount, filename), "old fake state");
  const cliAccount = realAccount("codex").folder;
  mkdirSync(cliAccount, { recursive: true });
  const temporary = join(cliAccount, `${filename}.tmp`);
  writeFileSync(temporary, "new fake state");
  renameSync(temporary, join(cliAccount, filename));
  expect(readFileSync(join(daemonAccount, filename), "utf8")).toBe("new fake state");
});
