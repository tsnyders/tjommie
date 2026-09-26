import { existsSync, readFileSync, readdirSync, renameSync, writeFileSync } from "node:fs";
import { afterEach, expect, it, vi } from "vitest";
import { dataPaths, readOrCreateToken } from "./paths.ts";
import { tempRoot } from "./testing.ts";

vi.mock("node:fs", async (original) => {
  const fs = await original<typeof import("node:fs")>();
  return { ...fs, renameSync: vi.fn(fs.renameSync) };
});
afterEach(() => vi.clearAllMocks());

it.each(["", "  \n", "x".repeat(42)])("regenerates an empty or short token file", (previous) => {
  const paths = dataPaths(tempRoot());
  writeFileSync(paths.token, previous);
  const token = readOrCreateToken(paths.token);
  expect(token).toMatch(/^[A-Za-z0-9_-]{43}$/);
  expect(readFileSync(paths.token, "utf8")).toBe(token);
  expect(readOrCreateToken(paths.token)).toBe(token);
});

it("installs a complete token with a temp file and rename", () => {
  const paths = dataPaths(tempRoot());
  vi.mocked(renameSync).mockImplementationOnce((from, to) => {
    expect(String(from)).not.toBe(paths.token);
    expect(String(to)).toBe(paths.token);
    expect(existsSync(paths.token)).toBe(false);
    expect(readFileSync(from, "utf8")).toMatch(/^[A-Za-z0-9_-]{43}$/);
    writeFileSync(to, readFileSync(from));
  });
  const token = readOrCreateToken(paths.token);
  expect(renameSync).toHaveBeenCalledOnce();
  expect(readFileSync(paths.token, "utf8")).toBe(token);
  expect(readdirSync(paths.root).filter((name) => name.startsWith("daemon-token."))).toEqual([]);
});
