import * as childProcess from "node:child_process";
import { mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { expect, it, vi } from "vitest";
import { assertOwnerUnchanged, ownerChangedPaths, ownerLastWriteSnapshot } from "./owner-safety.ts";

vi.mock("node:child_process", async (original) => ({ ...await original<typeof import("node:child_process")>(), execFileSync: vi.fn() }));

it("exempts only the owner accounts subtree while the owner app runs at both snapshots", () => {
  const root = mkdtempSync(join(tmpdir(), "tjommie-owner-safety-"));
  const local = join(root, "Local"), roaming = join(root, "Roaming");
  const data = join(local, "Tjommie");
  const accounts = join(data, "accounts");
  const account = join(accounts, "claude-main", ".last-cleanup");
  const nested = join(accounts, "claude-main", "projects", "session.jsonl");
  const other = join(data, "accounts-backup", "state.json");
  try {
    for (const folder of [dirname(account), dirname(nested), dirname(other)]) mkdirSync(folder, { recursive: true });
    writeFileSync(account, "before");
    const beforeRunning = ownerLastWriteSnapshot(local, roaming, true, []);
    const beforeClosed = ownerLastWriteSnapshot(local, roaming, false, []);
    utimesSync(account, new Date(), new Date(Date.now() + 10_000));
    writeFileSync(nested, "new owner session");
    writeFileSync(other, "unrelated change");
    const afterRunning = ownerLastWriteSnapshot(local, roaming, true, []);
    const afterClosed = ownerLastWriteSnapshot(local, roaming, false, []);
    expect(ownerChangedPaths(beforeRunning, afterRunning)).not.toContain(account);
    expect(ownerChangedPaths(beforeRunning, afterRunning)).not.toContain(nested);
    expect(ownerChangedPaths(beforeRunning, afterRunning)).not.toContain(accounts);
    expect(ownerChangedPaths(beforeRunning, afterRunning)).toContain(other);
    expect(ownerChangedPaths(beforeClosed, afterClosed)).toContain(account);
    expect(ownerChangedPaths(beforeClosed, afterClosed)).toContain(nested);
    expect(ownerChangedPaths(beforeRunning, afterClosed)).toContain(account);
  } finally {
    if (dirname(resolve(root)) !== resolve(tmpdir())) throw new Error("Refusing owner-safety fixture cleanup outside Temp.");
    rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

it("keeps registry presence and value types strict, and saves every changed path", () => {
  const root = mkdtempSync(join(tmpdir(), "tjommie-owner-evidence-"));
  const local = join(root, "Local"), roaming = join(root, "Roaming");
  const key = { hive: "HKCU" as const, view: 64 as const, key: "Software\\TjommieGuardFixtures\\synthetic" };
  try {
    const before = ownerLastWriteSnapshot(local, roaming, true, [{ ...key, present: false, values: [] }]);
    const present = ownerLastWriteSnapshot(local, roaming, true, [{ ...key, present: true, values: [] }]);
    expect(ownerChangedPaths(before, present)).toHaveLength(1);
    const string = ownerLastWriteSnapshot(local, roaming, true, [{ ...key, present: true, values: [{ name: "InstallLocation", type: "String", value: "1" }] }]);
    const expanded = ownerLastWriteSnapshot(local, roaming, true, [{ ...key, present: true, values: [{ name: "InstallLocation", type: "ExpandString", value: "1" }] }]);
    expect(ownerChangedPaths(string, expanded)).toHaveLength(1);
    for (let i = 0; i < 20; i++) { present.strict.set(`changed-${i}`, "1"); present.live.set(`changed-${i}`, "1"); }
    let failure: unknown;
    try { assertOwnerUnchanged(before, present, root); } catch (error) { failure = error; }
    expect(failure).toBeInstanceOf(Error);
    const evidence = join(root, readdirSync(root).find((name) => name.endsWith(".json"))!);
    expect(String(failure)).toContain(evidence);
    const saved = JSON.parse(readFileSync(evidence, "utf8")) as { changed: string[] };
    expect(saved.changed).toEqual(ownerChangedPaths(before, present));
    expect(saved.changed).toContain("changed-19");
    expect(String(failure)).toContain("changed-19");
  } finally { rmSync(root, { recursive: true, force: true }); }
});

it("refuses to take a usable start snapshot when the owner process query fails or is inconclusive", () => {
  const root = mkdtempSync(join(tmpdir(), "tjommie-owner-query-"));
  const query = vi.mocked(childProcess.execFileSync);
  try {
    const snapshot = () => ownerLastWriteSnapshot(root, root, undefined, []);
    query.mockImplementation(() => { throw new Error("CIM query failed"); });
    expect(snapshot).toThrow(/owner.*may be running/i);
    query.mockReturnValue("");
    expect(snapshot).toThrow(/owner.*may be running/i);
    query.mockReturnValue("False\r\n");
    expect(snapshot().running).toBe(false);
    query.mockReturnValue("True\r\n");
    expect(snapshot().running).toBe(true);
  } finally { query.mockReset(); rmSync(root, { recursive: true, force: true }); }
});
