import { existsSync, mkdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { dataPaths } from "./paths.ts";
import { tempRoot } from "./testing.ts";
import { buildAllowCommand, buildDenyCommand, protectCodexData, protectCodexDataAsync, type AclDriver, type AclPassResult, type AclTarget, type BotHome } from "./windows-acl.ts";

const sid = "S-1-5-21-1-2-3-1002";
const success = { sid, groupMissing: false, failures: [], applied: 0 };

function fixture() {
  const paths = dataPaths(tempRoot());
  mkdirSync(join(paths.accounts, "claude-main"));
  const codexHome = join(paths.accounts, "codex-main");
  mkdirSync(join(codexHome, ".sandbox"), { recursive: true });
  mkdirSync(join(codexHome, ".sandbox-bin"));
  writeFileSync(paths.token, "fixture");
  for (const name of ["tjommie.db", "tjommie.db-wal", "tjommie.db-shm"]) writeFileSync(join(paths.data, name), "fixture");
  const claude: BotHome = { id: "claude-bot", homeFolder: join(paths.bots, "claude-bot"), provider: "claude" };
  const codex: BotHome = { id: "codex-bot", homeFolder: join(paths.bots, "codex-bot"), provider: "codex" };
  mkdirSync(claude.homeFolder);
  mkdirSync(codex.homeFolder);
  mkdirSync(join(codex.homeFolder, "inbox"));
  writeFileSync(join(codex.homeFolder, "notes.md"), "fixture");
  return { paths, codexHome, claude, codex };
}

function driver() {
  const sync = vi.fn((_targets: AclTarget[]): AclPassResult => success);
  const asyncRun = vi.fn(async (_targets: AclTarget[]): Promise<AclPassResult> => success);
  return { runSync: sync, run: asyncRun } satisfies AclDriver;
}

describe("Codex data ACLs", () => {
  it("builds full-path direct and inheritable commands without a shell", () => {
    const deny = buildDenyCommand("C:\\Tjommie\\logs", sid, true);
    expect(deny.file.toLowerCase()).toMatch(/\\system32\\icacls\.exe$/);
    expect(deny.args).toEqual(["C:\\Tjommie\\logs", "/deny", `*${sid}:(OI)(CI)(RX,W)`]);
    expect(buildDenyCommand("C:\\Tjommie\\daemon-token", sid, false).args[2]).toBe(`*${sid}:(RX,W)`);
    expect(buildAllowCommand("C:\\Tjommie\\accounts\\codex-main\\.sandbox", sid, true).args[1]).toBe("/grant");
  });

  it("sends all protected folders through one pass, including shared files and Claude bot homes", () => {
    const { paths, codexHome, claude, codex } = fixture();
    expect(existsSync(paths.sharedFiles)).toBe(true);
    const acl = driver();
    expect(protectCodexData(paths, [claude, codex], acl, "win32").ready).toBe(true);
    expect(acl.runSync).toHaveBeenCalledOnce();
    const targets = acl.runSync.mock.calls[0][0];
    const at = (path: string) => targets.find((target) => target.path === path);
    expect(at(join(paths.accounts, "claude-main"))?.mode).toBe("deny");
    expect(at(paths.sharedFiles)).toMatchObject({ mode: "deny", folder: true });
    expect(at(claude.homeFolder)).toMatchObject({ mode: "deny", folder: true });
    expect(at(codex.homeFolder)?.mode).toBe("removeDeny");
    expect(at(join(codex.homeFolder, "notes.md"))?.mode).toBe("checkNoDeny");
    expect(at(codexHome)?.mode).toBe("deny");
    expect(at(join(codexHome, ".sandbox"))?.mode).toBe("allow");
    expect(at(join(codexHome, ".sandbox-bin"))?.mode).toBe("verifyBin");
    expect(at(paths.bots)).toBeUndefined();
    expect(targets.filter((target) => target.path.startsWith(paths.data))).toHaveLength(4);
  });

  it("runs refresh passes asynchronously through the same target set", async () => {
    const { paths, claude } = fixture();
    const acl = driver();
    expect((await protectCodexDataAsync(paths, [claude], acl, "win32")).ready).toBe(true);
    expect(acl.run).toHaveBeenCalledOnce();
    expect(acl.runSync).not.toHaveBeenCalled();
    expect(acl.run.mock.calls[0][0]).toContainEqual(expect.objectContaining({ path: claude.homeFolder, mode: "deny" }));
  });

  it("skips a missing Claude bot home without taking down Codex protection", () => {
    const { paths, claude } = fixture();
    rmSync(claude.homeFolder, { recursive: true });
    const acl = driver();
    const info = vi.spyOn(console, "info").mockImplementation(() => {});
    expect(protectCodexData(paths, [claude], acl, "win32")).toEqual({ status: "ready", ready: true, reason: null });
    expect(acl.runSync.mock.calls[0][0].some((target) => target.path === claude.homeFolder)).toBe(false);
    expect(info.mock.calls.some((call) => String(call[0]).includes(`${claude.homeFolder}: Claude bot folder is missing`))).toBe(true);
  });

  it("skips a missing Codex bot home without taking down other Codex bots", () => {
    const { paths, codex } = fixture();
    const other: BotHome = { id: "other-codex", homeFolder: join(paths.bots, "other-codex"), provider: "codex" };
    mkdirSync(join(other.homeFolder, "inbox"), { recursive: true });
    writeFileSync(join(other.homeFolder, "notes.md"), "fixture");
    rmSync(codex.homeFolder, { recursive: true });
    const acl = driver();
    const info = vi.spyOn(console, "info").mockImplementation(() => {});
    expect(protectCodexData(paths, [codex, other], acl, "win32")).toMatchObject({ ready: true, status: "ready" });
    expect(acl.runSync.mock.calls[0][0].some((target) => target.path.startsWith(codex.homeFolder))).toBe(false);
    expect(acl.runSync.mock.calls[0][0]).toContainEqual(expect.objectContaining({ path: other.homeFolder, mode: "removeDeny" }));
    expect(info.mock.calls.some((call) => String(call[0]).includes(`${codex.homeFolder}: Codex bot folder is missing`))).toBe(true);
  });

  it("distinguishes an absent group and setup marker from a failed PowerShell spawn", () => {
    const { paths } = fixture();
    const info = vi.spyOn(console, "info").mockImplementation(() => {});
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    const missing = driver();
    missing.runSync.mockReturnValue({ sid: null, groupMissing: true, failures: [], applied: 0 });
    expect(protectCodexData(paths, [], missing, "win32")).toMatchObject({ ready: false, status: "not_configured", reason: "Set up Codex's sandbox" });
    expect(info).toHaveBeenCalled();
    expect(error).not.toHaveBeenCalled();
    const broken = driver();
    broken.runSync.mockImplementation(() => { throw new Error("PowerShell ACL pass could not start: ENOENT"); });
    expect(protectCodexData(paths, [], broken, "win32")).toMatchObject({ ready: false, status: "check_failed", reason: expect.stringContaining("couldn't check Codex's sandbox") });
    expect(error.mock.calls.some((call) => String(call[0]).includes("could not start"))).toBe(true);
  });

  it("skips codex-main until the setup marker exists", () => {
    const paths = dataPaths(tempRoot());
    mkdirSync(join(paths.accounts, "claude-main"));
    const acl = driver();
    const info = vi.spyOn(console, "info").mockImplementation(() => {});
    expect(protectCodexData(paths, [], acl, "win32")).toMatchObject({ status: "not_configured", reason: "Set up Codex's sandbox" });
    expect(acl.runSync.mock.calls[0][0].some((target) => target.path.includes("codex-main"))).toBe(false);
    expect(info).toHaveBeenCalled();
  });

  it.each(["claude-main", "codex-main"])("rejects a non-directory %s account entry", (name) => {
    const paths = dataPaths(tempRoot());
    writeFileSync(join(paths.accounts, name), "fixture");
    const acl = driver();
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    expect(protectCodexData(paths, [], acl, "win32")).toMatchObject({ ready: false, status: "check_failed" });
    expect(log).toHaveBeenCalled();
  });

  it.skipIf(process.platform !== "win32")("refuses a junction in place of an account folder", (context) => {
    const paths = dataPaths(tempRoot());
    const outside = join(tempRoot(), "outside");
    mkdirSync(outside);
    try { symlinkSync(outside, join(paths.accounts, "claude-main"), "junction"); }
    catch { context.skip(); return; }
    const acl = driver();
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    expect(protectCodexData(paths, [], acl, "win32").ready).toBe(false);
    expect(acl.runSync.mock.calls[0][0].some((target) => target.path === outside)).toBe(false);
    expect(log.mock.calls.some((call) => String(call[0]).includes("unexpected path type"))).toBe(true);
  });
});


it("protects every additional account while a new unconfigured account does not block a ready sandbox", () => {
  const { paths, codexHome } = fixture();
  const accounts = [
    { id: "codex-main", provider: "codex" as const, folder: codexHome },
    { id: "second-codex", provider: "codex" as const, folder: join(paths.accounts, "second-codex") },
    { id: "second-claude", provider: "claude" as const, folder: join(paths.accounts, "second-claude") },
  ];
  mkdirSync(accounts[1].folder); mkdirSync(accounts[2].folder);
  const acl = driver(); expect(protectCodexData(paths, [], acl, "win32", accounts)).toMatchObject({ ready: true });
  const targets = acl.runSync.mock.calls[0][0];
  for (const account of accounts) expect(targets).toContainEqual(expect.objectContaining({ path: account.folder, mode: "deny" }));
  expect(targets).toContainEqual(expect.objectContaining({ path: join(codexHome, ".sandbox-bin"), mode: "verifyBin" }));
  expect(targets.some((target) => target.path === join(accounts[1].folder, ".sandbox"))).toBe(false);
});

it("rejects an account metadata path outside the account-id folder", () => {
  const { paths } = fixture(); const acl = driver();
  expect(protectCodexData(paths, [], acl, "win32", [{ id: "other", provider: "codex", folder: paths.root }])).toMatchObject({ ready: false, status: "check_failed" });
  expect(acl.runSync.mock.calls[0][0].some((target) => target.path === paths.root)).toBe(false);
});

it("protects an explicitly configured external account root in startup and refresh passes", async () => {
  const { paths, claude } = fixture();
  const originalAccounts = paths.accounts;
  paths.accounts = join(tempRoot(), "external-accounts");
  const claudeHome = join(paths.accounts, "claude-main"), codexHome = join(paths.accounts, "codex-main");
  mkdirSync(claudeHome, { recursive: true });
  mkdirSync(join(codexHome, ".sandbox"), { recursive: true }); mkdirSync(join(codexHome, ".sandbox-bin"));
  const accounts = [{ id: "claude-main", provider: "claude" as const, folder: claudeHome },
    { id: "codex-main", provider: "codex" as const, folder: codexHome }];
  const acl = driver();
  expect(protectCodexData(paths, [claude], acl, "win32")).toMatchObject({ ready: true });
  expect(await protectCodexDataAsync(paths, [claude], acl, "win32", accounts)).toMatchObject({ ready: true });
  const targets = acl.run.mock.calls[0][0];
  expect(acl.runSync.mock.calls[0][0]).toEqual(targets);
  for (const path of [claudeHome, codexHome, paths.data, paths.logs, paths.sharedFiles, claude.homeFolder]) {
    expect(targets).toContainEqual(expect.objectContaining({ path, mode: "deny" }));
  }
  expect(targets).toContainEqual(expect.objectContaining({ path: join(codexHome, ".sandbox"), mode: "allow" }));
  expect(targets).toContainEqual(expect.objectContaining({ path: join(codexHome, ".sandbox-bin"), mode: "verifyBin" }));
  expect(targets.some(({ path }) => path.startsWith(originalAccounts) || path === paths.accounts)).toBe(false);
});

it("keeps exact account identity and non-account data containment with an external account root", () => {
  const { paths } = fixture(); const acl = driver();
  paths.accounts = tempRoot();
  const wrong = join(paths.accounts, "wrong-id"); mkdirSync(wrong);
  paths.logs = join(paths.accounts, "logs"); mkdirSync(paths.logs);
  expect(protectCodexData(paths, [], acl, "win32", [{ id: "expected-id", provider: "claude", folder: wrong }])).toMatchObject({ ready: false, status: "check_failed" });
  const targets = acl.runSync.mock.calls[0][0];
  expect(targets.some(({ path }) => path === wrong || path === paths.logs)).toBe(false);
});

it.skipIf(process.platform !== "win32").each(["root", "account", "sandbox", "sandbox-bin"] as const)("rejects an external account %s junction before applying ACLs through it", (kind) => {
  const paths = dataPaths(tempRoot()); paths.accounts = join(tempRoot(), "accounts");
  const outside = tempRoot(), home = join(paths.accounts, "codex-main");
  const unsafe = kind === "root" ? paths.accounts : kind === "account" ? home : join(home, `.${kind}`);
  if (kind !== "root") mkdirSync(paths.accounts);
  if (kind === "sandbox" || kind === "sandbox-bin") mkdirSync(home);
  if (kind === "sandbox-bin") mkdirSync(join(home, ".sandbox"));
  symlinkSync(outside, unsafe, "junction");
  const acl = driver();
  expect(protectCodexData(paths, [], acl, "win32", [{ id: "codex-main", provider: "codex", folder: home }])).toMatchObject({ ready: false, status: "check_failed" });
  expect(acl.runSync.mock.calls[0][0].some(({ path }) => path === unsafe || path.startsWith(`${unsafe}\\`) || path.startsWith(outside))).toBe(false);
});
