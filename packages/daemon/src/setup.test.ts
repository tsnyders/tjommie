import { existsSync } from "node:fs";
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

  it("reports non-zero status and version exits", async () => {
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
