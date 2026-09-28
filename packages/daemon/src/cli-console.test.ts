import { spawn } from "node:child_process";
import { EventEmitter } from "node:events";
import { expect, it, vi } from "vitest";
import { openConsole } from "./cli.ts";

vi.mock("node:child_process", async (importOriginal) => ({
  ...await importOriginal<typeof import("node:child_process")>(),
  spawn: vi.fn(),
}));

it("returns a launcher that waits for the visible console to close", () => {
  const child = Object.assign(new EventEmitter(), { unref: vi.fn() });
  vi.mocked(spawn).mockReturnValue(child as unknown as ReturnType<typeof spawn>);

  expect(openConsole("Sign in to Claude", "claude auth login", { CLAUDE_CONFIG_DIR: "account" })).toBe(child);
  expect(spawn).toHaveBeenCalledWith("cmd.exe", ["/c", "start", "/wait", '"Sign in to Claude"', "cmd.exe", "/k", "claude auth login"], expect.objectContaining({
    detached: true,
    stdio: "ignore",
    windowsVerbatimArguments: true,
  }));
  expect(child.unref).toHaveBeenCalledOnce();
});
