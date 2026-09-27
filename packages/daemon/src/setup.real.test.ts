import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { openDb } from "./db.ts";
import { checkSetup, ensureAccounts, listModels } from "./setup.ts";
import { realAccountsDir, tempRoot } from "./testing.ts";

describe("setup against the installed CLIs", () => {
  it("finds both CLIs installed and fresh account folders signed out", async () => {
    const root = tempRoot();
    const db = openDb(join(root, "t.db"));
    ensureAccounts(db, join(root, "accounts"));
    const [claude, codex] = await checkSetup(db);
    expect(claude).toMatchObject({ provider: "claude", installed: true, signedIn: false, error: null });
    expect(claude.version).toMatch(/^\d+\.\d+\.\d+$/);
    expect(codex).toMatchObject({ provider: "codex", installed: true, signedIn: false, error: null });
    expect(codex.version).toMatch(/^\d+\.\d+\.\d+$/);
  });

  it("lists Codex models before sign-in, including a default", async () => {
    const root = tempRoot();
    const db = openDb(join(root, "t.db"));
    ensureAccounts(db, join(root, "accounts"));
    const models = await listModels(db, "codex");
    expect(models.length).toBeGreaterThan(0);
    expect(models.some((m) => m.isDefault)).toBe(true);
  });

  it("sees the one-time sign-in from `npm run signin -w @tjommie/daemon`", async () => {
    const db = openDb(join(tempRoot(), "t.db"));
    ensureAccounts(db, realAccountsDir());
    const setup = await checkSetup(db);
    expect(setup.map((s) => [s.provider, s.signedIn])).toEqual([
      ["claude", true],
      ["codex", true],
    ]);
    expect(setup[1]).toMatchObject({ provider: "codex", sandbox: "ready" });
  });
});
