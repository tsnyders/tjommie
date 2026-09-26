import { existsSync, mkdirSync, readFileSync, rmSync, symlinkSync, unlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { openDb } from "./db.ts";
import { accountEnv, accountFor, checkSetup, ensureAccounts, parseClaudeStatus, parseVersion } from "./setup.ts";
import { tempRoot } from "./testing.ts";

type CliCommand = { file: string; args: string[] };
type CliResult = { code: number | null; stdout: string; stderr: string };

const cliMocks = vi.hoisted(() => ({
  resolveCli: vi.fn<(name: "claude" | "codex") => CliCommand | null>(),
  runCli: vi.fn<(cmd: CliCommand, args: string[], env: Record<string, string>) => Promise<CliResult>>(),
}));

const codexMocks = vi.hoisted(() => ({
  startCodexServer: vi.fn(),
}));

vi.mock("./cli.ts", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./cli.ts")>();
  return { ...actual, resolveCli: cliMocks.resolveCli, runCli: cliMocks.runCli };
});

vi.mock("./providers/codex-rpc.ts", () => codexMocks);

vi.mock("node:fs", async (importOriginal) => {
  const actual = await importOriginal<typeof import("node:fs")>();
  return { ...actual, rmSync: vi.fn(actual.rmSync) };
});

const successfulCliRun = async (cmd: CliCommand, args: string[]): Promise<CliResult> => {
  if (args[0] === "--version") {
    return { code: 0, stdout: cmd.file === "claude" ? "2.1.280 (Claude Code)\n" : "codex-cli 0.155.1\n", stderr: "" };
  }
  return { code: 0, stdout: '{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty"}', stderr: "" };
};

beforeEach(() => {
  vi.clearAllMocks();
  cliMocks.resolveCli.mockImplementation((name) => ({ file: name, args: [] }));
  cliMocks.runCli.mockImplementation(successfulCliRun);
  codexMocks.startCodexServer.mockResolvedValue({
    rpc: { request: vi.fn().mockResolvedValue({ account: null }) },
    close: vi.fn(),
  });
});

afterEach(() => vi.unstubAllEnvs());

describe("accounts", () => {
  it("sweeps only confirmed stale session folders and preserves live or uncertain owners", () => {
    const root = tempRoot();
    const db = openDb(join(root, "t.db"));
    const accountsDir = join(root, "accounts");
    ensureAccounts(db, accountsDir);
    const folder = join(accountsDir, "claude-main");
    const cases: { name: string; owner: unknown; removed: boolean }[] = [
      { name: "stale", owner: { version: 1, ownerPid: 100, phase: "started", childPid: 200 }, removed: true },
      { name: "preparing", owner: { version: 1, ownerPid: 100, phase: "preparing", childPid: null }, removed: true },
      { name: "live-owner", owner: { version: 1, ownerPid: 101, phase: "started", childPid: 200 }, removed: false },
      { name: "surviving-child", owner: { version: 1, ownerPid: 100, phase: "started", childPid: 201 }, removed: false },
      { name: "unknown-process", owner: { version: 1, ownerPid: 102, phase: "started", childPid: 200 }, removed: false },
      { name: "starting", owner: { version: 1, ownerPid: 100, phase: "starting", childPid: null }, removed: false },
      { name: "malformed", owner: { version: 1, ownerPid: "wrong", phase: "started", childPid: 200 }, removed: false },
      { name: "markerless", owner: undefined, removed: false },
    ];
    for (const item of cases) {
      const dir = join(folder, `.tjommie-session-${item.name}`);
      mkdirSync(dir);
      writeFileSync(join(dir, "instructions.txt"), "Preserve unless confirmed stale");
      if (item.owner !== undefined) writeFileSync(join(dir, "owner.json"), JSON.stringify(item.owner));
    }
    const outside = join(root, "outside");
    mkdirSync(outside);
    writeFileSync(join(outside, "owner.json"), JSON.stringify(cases[0].owner));
    writeFileSync(join(outside, "keep.txt"), "outside");
    symlinkSync(outside, join(folder, ".tjommie-session-junction"), process.platform === "win32" ? "junction" : "dir");
    mkdirSync(join(folder, "unrelated"));
    writeFileSync(join(folder, "unrelated", "owner.json"), JSON.stringify(cases[0].owner));
    const liveness = vi.spyOn(process, "kill").mockImplementation((pid, signal) => {
      expect(signal).toBe(0);
      if (pid === 101 || pid === 201) return true;
      throw Object.assign(new Error("Process check"), { code: pid === 102 ? "EPERM" : "ESRCH" });
    });
    try {
      ensureAccounts(db, accountsDir);
      for (const item of cases) expect(existsSync(join(folder, `.tjommie-session-${item.name}`))).toBe(!item.removed);
      expect(readFileSync(join(outside, "keep.txt"), "utf8")).toBe("outside");
      expect(existsSync(join(folder, "unrelated"))).toBe(true);
    } finally {
      liveness.mockRestore();
    }
  });
  it("merges Claude deny rules without dropping existing settings or permissions", () => {
    const root = tempRoot();
    const db = openDb(join(root, "t.db"));
    const accountsDir = join(root, "accounts");
    ensureAccounts(db, accountsDir);
    const file = join(accountsDir, "claude-main", "settings.json");
    writeFileSync(file, JSON.stringify({ theme: "dark", permissions: { allow: ["Read"], deny: ["Bash(rm *)"], defaultMode: "manual" } }));
    ensureAccounts(db, accountsDir);
    const settings = JSON.parse(readFileSync(file, "utf8")) as { theme: string; permissions: { allow: string[]; deny: string[]; defaultMode: string } };
    expect(settings.theme).toBe("dark");
    expect(settings.permissions.allow).toEqual(["Read"]);
    expect(settings.permissions.defaultMode).toBe("manual");
    const absolute = root.replace(/\\/g, "/").replace(/^([A-Za-z]):/, (_match, drive: string) => `/${drive.toLowerCase()}`);
    expect(settings.permissions.deny).toEqual(["Bash(rm *)", "Edit(**/.claude/**)", "Write(**/.claude/**)", `Read(/${absolute}/accounts/**)`, `Read(/${absolute}/daemon-token)`, `Read(/${absolute}/tjommie.db*)`]);
    ensureAccounts(db, accountsDir);
    expect(JSON.parse(readFileSync(file, "utf8"))).toEqual(settings);
  });

  it("prunes only managed deny rules for missing data roots and preserves live roots and user rules", () => {
    const root = tempRoot();
    const db = openDb(join(root, "t.db"));
    const accountsDir = join(root, "accounts");
    const staleRoot = join(root, "removed data");
    const liveRoot = join(root, "live data");
    mkdirSync(liveRoot);
    const rulesFor = (folder: string): string[] => {
      const absolute = folder.replace(/\\/g, "/").replace(/^([A-Za-z]):/, (_match, drive: string) => `/${drive.toLowerCase()}`);
      return [`Read(/${absolute}/accounts/**)`, `Read(/${absolute}/daemon-token)`, `Read(/${absolute}/tjommie.db*)`];
    };
    try {
      ensureAccounts(db, accountsDir);
      const file = join(accountsDir, "claude-main", "settings.json");
      const staleRules = rulesFor(staleRoot);
      const liveRules = rulesFor(liveRoot);
      const userRules = ["Bash(rm *)", "Read(//unrelated/private/**)", "Read(**/daemon-token)",
        ...rulesFor(join(root, "user-only")).slice(0, 1),
        ...rulesFor(join(root, "user-*")),
        staleRules[0].replace("Read(", "Write("), staleRules[2].replace("db*", "db")];
      writeFileSync(file, JSON.stringify({ theme: "dark", permissions: { allow: ["Read"], deny: [...staleRules, ...liveRules, ...userRules] } }));
      ensureAccounts(db, accountsDir);
      const settings = JSON.parse(readFileSync(file, "utf8")) as { theme: string; permissions: { allow: string[]; deny: string[] } };
      expect(settings.theme).toBe("dark");
      expect(settings.permissions.allow).toEqual(["Read"]);
      expect(settings.permissions.deny).toEqual([...liveRules, ...userRules, "Edit(**/.claude/**)", "Write(**/.claude/**)", ...rulesFor(root)]);
      ensureAccounts(db, accountsDir);
      expect(JSON.parse(readFileSync(file, "utf8"))).toEqual(settings);
    } finally {
      db.close();
    }
  });

  it("logs stale folder removal failures and continues account setup", () => {
    const root = tempRoot();
    const db = openDb(join(root, "t.db"));
    const accountsDir = join(root, "accounts");
    ensureAccounts(db, accountsDir);
    const folder = join(accountsDir, "claude-main", ".tjommie-session-locked");
    mkdirSync(folder);
    writeFileSync(join(folder, "owner.json"), JSON.stringify({ version: 1, ownerPid: 100, phase: "started", childPid: 200 }));
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    const liveness = vi.spyOn(process, "kill").mockImplementation(() => { throw Object.assign(new Error("Gone"), { code: "ESRCH" }); });
    vi.mocked(rmSync).mockImplementationOnce(() => { throw Object.assign(new Error("Locked"), { code: "EBUSY" }); });
    try {
      expect(ensureAccounts(db, accountsDir)).toHaveLength(2);
      expect(existsSync(folder)).toBe(true);
      expect(logged).toHaveBeenCalledWith("Could not remove stale Claude session files.", folder);
    } finally {
      logged.mockRestore();
      liveness.mockRestore();
    }
  });

  it.each([
    "not JSON", "null", "[]", "3", '"text"',
    '{"permissions":null}', '{"permissions":[]}', '{"permissions":"wrong"}',
    '{"permissions":{"deny":null}}', '{"permissions":{"deny":"wrong"}}',
    '{"permissions":{"deny":{}}}', '{"permissions":{"deny":[3]}}',
  ])("keeps malformed Claude settings intact and continues account setup: %s", (invalid) => {
    const root = tempRoot();
    const db = openDb(join(root, "t.db"));
    ensureAccounts(db, join(root, "accounts"));
    const file = join(root, "accounts", "claude-main", "settings.json");
    expect(existsSync(file)).toBe(true);
    const bytes = Buffer.from(`  ${invalid}\r\n`, "utf8");
    writeFileSync(file, bytes);
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      expect(ensureAccounts(db, join(root, "accounts"))).toHaveLength(2);
      expect(readFileSync(file)).toEqual(bytes);
      expect(logged).toHaveBeenCalledOnce();
    } finally {
      logged.mockRestore();
    }
  });

  it("still reports filesystem failures while reading Claude settings", () => {
    const root = tempRoot();
    const db = openDb(join(root, "t.db"));
    const accountsDir = join(root, "accounts");
    ensureAccounts(db, accountsDir);
    const file = join(accountsDir, "claude-main", "settings.json");
    unlinkSync(file);
    mkdirSync(file);
    expect(() => ensureAccounts(db, accountsDir)).toThrow();
  });
  it("creates one account per provider, once, each with its folder", () => {
    const root = tempRoot();
    const db = openDb(join(root, "t.db"));
    const first = ensureAccounts(db, join(root, "accounts"));
    expect(ensureAccounts(db, join(root, "accounts"))).toEqual(first);
    expect(first.map((a) => a.id)).toEqual(["claude-main", "codex-main"]);
    for (const account of first) expect(existsSync(account.folder)).toBe(true);
  });

  it("points each CLI at its account folder and never passes inherited keys", () => {
    vi.stubEnv("ANTHROPIC_API_KEY", "sk-inherited");
    vi.stubEnv("CODEX_HOME", "C:\\somewhere-else");
    const root = tempRoot();
    const db = openDb(join(root, "t.db"));
    ensureAccounts(db, join(root, "accounts"));
    const claude = accountEnv(accountFor(db, "claude"));
    expect(claude.CLAUDE_CONFIG_DIR).toBe(join(root, "accounts", "claude-main"));
    expect(claude.ANTHROPIC_API_KEY).toBeUndefined();
    expect(accountEnv(accountFor(db, "codex")).CODEX_HOME).toBe(join(root, "accounts", "codex-main"));
  });
});

describe("parsers", () => {
  it("reads both CLIs' version output (real output)", () => {
    expect(parseVersion("2.1.280 (Claude Code)\n")).toBe("2.1.280");
    expect(parseVersion("codex-cli 0.155.1\n")).toBe("0.155.1");
    expect(parseVersion("command not found")).toBeNull();
  });

  it("reads claude auth status JSON (real output, email replaced)", () => {
    expect(parseClaudeStatus('{"loggedIn": false, "authMethod": "none", "apiProvider": "firstParty"}')).toEqual({ signedIn: false, account: null, plan: null });
    expect(parseClaudeStatus('{"loggedIn": true, "authMethod": "claude.ai", "email": "person@example.com", "subscriptionType": "max"}')).toEqual({
      signedIn: true,
      account: "person@example.com",
      plan: "max",
    });
    expect(parseClaudeStatus("not json")).toEqual({ signedIn: false, account: null, plan: null });
  });
});

describe("setup checks", () => {
  it("treats Claude's exit-1 signed-out JSON as a successful check", async () => {
    cliMocks.runCli.mockImplementation(async (cmd, args) => {
      if (cmd.file === "claude" && args[0] === "auth") {
        return { code: 1, stdout: '{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty"}', stderr: "" };
      }
      return successfulCliRun(cmd, args);
    });
    const root = tempRoot();
    const db = openDb(join(root, "t.db"));
    ensureAccounts(db, join(root, "accounts"));

    const [claude] = await checkSetup(db);

    expect(claude).toEqual({
      provider: "claude",
      installed: true,
      version: "2.1.280",
      signedIn: false,
      account: null,
      plan: null,
      error: null,
    });
  });

  it("reports a CLI spawn failure", async () => {
    cliMocks.runCli.mockImplementation(async (cmd, args) => {
      if (cmd.file === "claude" && args[0] === "--version") return { code: null, stdout: "", stderr: "spawn ENOENT" };
      return successfulCliRun(cmd, args);
    });
    const root = tempRoot();
    const db = openDb(join(root, "t.db"));
    ensureAccounts(db, join(root, "accounts"));

    const [claude] = await checkSetup(db);

    expect(claude).toEqual({
      provider: "claude",
      installed: true,
      version: null,
      signedIn: false,
      account: null,
      plan: null,
      error: "Claude Code version check failed",
    });
  });

  it("reports garbage status output and a non-zero version exit", async () => {
    cliMocks.runCli.mockImplementation(async (cmd, args) => {
      if (cmd.file === "claude" && args[0] === "auth") return { code: 2, stdout: "", stderr: "not signed in" };
      if (cmd.file === "codex" && args[0] === "--version") return { code: 4, stdout: "", stderr: "failed" };
      return successfulCliRun(cmd, args);
    });
    const root = tempRoot();
    const db = openDb(join(root, "t.db"));
    ensureAccounts(db, join(root, "accounts"));

    const [claude, codex] = await checkSetup(db);

    expect(claude).toMatchObject({
      installed: true,
      version: "2.1.280",
      signedIn: false,
      error: "Claude sign-in check failed",
    });
    expect(codex).toMatchObject({
      installed: true,
      version: null,
      signedIn: false,
      error: "Codex version check failed",
    });
    expect(codexMocks.startCodexServer).not.toHaveBeenCalled();
  });
});
