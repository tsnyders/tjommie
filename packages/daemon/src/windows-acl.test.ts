import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import { dataPaths } from "./paths.ts";
import { tempRoot } from "./testing.ts";
import { buildAllowCommand, buildDenyCommand, protectCodexData, type AclDriver } from "./windows-acl.ts";

const sid = "S-1-5-21-1-2-3-1002";

describe("Codex data ACLs", () => {
  it("builds direct and inheritable icacls deny commands without a shell", () => {
    expect(buildDenyCommand("C:\\Tjommie\\logs", sid, true)).toEqual({
      file: "icacls.exe", args: ["C:\\Tjommie\\logs", "/deny", `*${sid}:(OI)(CI)(RX,W)`],
    });
    expect(buildDenyCommand("C:\\Tjommie\\daemon-token", sid, false)).toEqual({
      file: "icacls.exe", args: ["C:\\Tjommie\\daemon-token", "/deny", `*${sid}:(RX,W)`],
    });
    expect(buildAllowCommand("C:\\Tjommie\\accounts\\codex-main\\.sandbox", sid, true)).toEqual({
      file: "icacls.exe", args: ["C:\\Tjommie\\accounts\\codex-main\\.sandbox", "/grant", `*${sid}:(OI)(CI)(RX,W)`],
    });
  });

  it("applies the account deny and sandbox allow, verifies them, and stays idempotent", () => {
    const paths = dataPaths(tempRoot());
    mkdirSync(join(paths.accounts, "claude-main"));
    const codexHome = join(paths.accounts, "codex-main");
    const sandbox = join(codexHome, ".sandbox");
    const sandboxBin = join(codexHome, ".sandbox-bin");
    mkdirSync(sandbox, { recursive: true });
    mkdirSync(sandboxBin);
    writeFileSync(paths.token, "fake token");
    const applied = new Set<string>();
    const commands: ReturnType<typeof buildDenyCommand>[] = [];
    const driver: AclDriver = {
      lookupSid: () => sid,
      hasDeny: (path) => applied.has(`deny:${path}`),
      hasAllow: (path) => applied.has(`allow:${path}`),
      sandboxBinReady: (path) => path === sandboxBin,
      apply: (command) => { commands.push(command); applied.add(`${command.args[1] === "/grant" ? "allow" : "deny"}:${command.args[0]}`); },
    };
    expect(protectCodexData(paths, driver, "win32").ready).toBe(true);
    expect(commands.map((command) => command.args[0])).toEqual(expect.arrayContaining([
      join(paths.accounts, "claude-main"), codexHome, sandbox, paths.logs, paths.data, paths.token,
    ]));
    expect(commands).toContainEqual(buildDenyCommand(codexHome, sid, true));
    expect(commands).toContainEqual(buildAllowCommand(sandbox, sid, true));
    expect(commands.some((command) => command.args[0] === sandboxBin)).toBe(false);
    expect(commands.some((command) => command.args[0] === paths.bots)).toBe(false);
    const count = commands.length;
    expect(protectCodexData(paths, driver, "win32").ready).toBe(true);
    expect(commands).toHaveLength(count);
  });

  it("fails closed before touching codex-main if sandbox-bin lacks its protected group allow", () => {
    const paths = dataPaths(tempRoot());
    mkdirSync(join(paths.accounts, "claude-main"));
    const codexHome = join(paths.accounts, "codex-main");
    mkdirSync(join(codexHome, ".sandbox"), { recursive: true });
    mkdirSync(join(codexHome, ".sandbox-bin"));
    const commands: ReturnType<typeof buildDenyCommand>[] = [];
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    const driver: AclDriver = { lookupSid: () => sid, hasDeny: () => true, hasAllow: () => true,
      sandboxBinReady: () => false, apply: (command) => { commands.push(command); } };
    expect(protectCodexData(paths, driver, "win32").ready).toBe(false);
    expect(commands.some((command) => command.args[0] === codexHome)).toBe(false);
    expect(log.mock.calls.some((call) => String(call[0]).includes(".sandbox-bin"))).toBe(true);
  });

  it.each(["claude-main", "codex-main"])("rejects a non-directory %s account entry", (name) => {
    const paths = dataPaths(tempRoot());
    writeFileSync(join(paths.accounts, name), "fake account marker");
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    const driver: AclDriver = { lookupSid: () => sid, hasDeny: () => true, hasAllow: () => true,
      sandboxBinReady: () => true, apply: () => {} };
    expect(protectCodexData(paths, driver, "win32").ready).toBe(false);
    expect(log).toHaveBeenCalled();
  });

  it("fails closed if verification fails or the group is missing", () => {
    const paths = dataPaths(tempRoot());
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    const driver: AclDriver = { lookupSid: () => sid, hasDeny: () => false, hasAllow: () => false, sandboxBinReady: () => false, apply: () => {} };
    expect(protectCodexData(paths, driver, "win32").ready).toBe(false);
    expect(log).toHaveBeenCalled();
    expect(protectCodexData(paths, { ...driver, lookupSid: () => null }, "win32").ready).toBe(false);
    log.mockRestore();
  });

  it("logs a no-op off Windows", () => {
    const log = vi.spyOn(console, "info").mockImplementation(() => {});
    expect(protectCodexData(dataPaths(tempRoot()), { lookupSid: () => { throw new Error("should not run"); }, hasDeny: () => false, hasAllow: () => false, sandboxBinReady: () => false, apply: () => {} }, "linux").ready).toBe(true);
    expect(log).toHaveBeenCalled();
    log.mockRestore();
  });
});
