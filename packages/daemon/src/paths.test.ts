import { existsSync, readFileSync, readdirSync, renameSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, expect, it, vi } from "vitest";
import { dataPaths, migrateLegacyDb, readOrCreateToken } from "./paths.ts";
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

it("moves a legacy database and all SQLite sidecars under data", () => {
  const paths = dataPaths(tempRoot());
  expect(paths.db).toBe(join(paths.data, "tjommie.db"));
  for (const suffix of ["", "-wal", "-shm", "-journal"]) writeFileSync(join(paths.root, `tjommie.db${suffix}`), `old${suffix}`);
  migrateLegacyDb(paths);
  for (const suffix of ["", "-wal", "-shm", "-journal"]) {
    expect(existsSync(join(paths.root, `tjommie.db${suffix}`))).toBe(false);
    expect(readFileSync(join(paths.data, `tjommie.db${suffix}`), "utf8")).toBe(`old${suffix}`);
  }
});

it("leaves every database part in one location after a failed move and resumes safely", () => {
  const paths = dataPaths(tempRoot());
  writeFileSync(join(paths.root, "tjommie.db"), "main");
  writeFileSync(join(paths.root, "tjommie.db-wal"), "wal");
  writeFileSync(join(paths.root, "tjommie.db-shm"), "shm");
  const actualRename = vi.mocked(renameSync).getMockImplementation()!;
  vi.mocked(renameSync).mockImplementation((from, to) => {
    if (String(from) === join(paths.root, "tjommie.db-wal")) throw new Error("move failed");
    actualRename(from, to);
  });
  expect(() => migrateLegacyDb(paths)).toThrow("move failed");
  expect(readFileSync(join(paths.root, "tjommie.db"), "utf8")).toBe("main");
  expect(readFileSync(join(paths.data, "tjommie.db-shm"), "utf8")).toBe("shm");
  vi.mocked(renameSync).mockImplementation(actualRename);
  migrateLegacyDb(paths);
  expect(readFileSync(paths.db, "utf8")).toBe("main");
  expect(readFileSync(join(paths.data, "tjommie.db-wal"), "utf8")).toBe("wal");
  expect(() => migrateLegacyDb(paths)).not.toThrow();
});

it("refuses to overwrite a database already in data", () => {
  const paths = dataPaths(tempRoot());
  writeFileSync(paths.db, "new");
  writeFileSync(join(paths.root, "tjommie.db"), "old");
  writeFileSync(join(paths.root, "tjommie.db-wal"), "old-wal");
  expect(() => migrateLegacyDb(paths)).toThrow(/already exists/);
  expect(readFileSync(paths.db, "utf8")).toBe("new");
  expect(readFileSync(join(paths.root, "tjommie.db"), "utf8")).toBe("old");
  expect(readFileSync(join(paths.root, "tjommie.db-wal"), "utf8")).toBe("old-wal");
});
