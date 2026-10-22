import { existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { randomBytes } from "node:crypto";
import { homedir } from "node:os";
import { join } from "node:path";

export const DEFAULT_PORT = 47100;

export type Paths = { root: string; data: string; db: string; token: string; bots: string; accounts: string; logs: string; sharedFiles: string };

const defaultRoot = () => process.env.TJOMMIE_HOME ?? join(process.env.LOCALAPPDATA ?? join(homedir(), ".local", "share"), "Tjommie");

export function dataPaths(root: string = defaultRoot()): Paths {
  const p = {
    root,
    data: join(root, "data"),
    db: join(root, "data", "tjommie.db"),
    token: join(root, "daemon-token"),
    bots: join(root, "bots"),
    accounts: join(root, "accounts"),
    logs: join(root, "logs"),
    sharedFiles: join(root, "shared-files"),
  };
  for (const dir of [p.root, p.bots, p.accounts, p.logs, p.data, p.sharedFiles]) mkdirSync(dir, { recursive: true });
  return p;
}

// The port lock is held by the caller. Move sidecars first and the main database last. A failed
// rename leaves each whole file at one of the two locations; a later start resumes the move.
// Never open SQLite until this completes, since a split WAL and database must not be used.
export function migrateLegacyDb(paths: Paths): void {
  const names = readdirSync(paths.root).filter((name) => /^tjommie\.db(?:$|[-.])/.test(name));
  const legacyMain = join(paths.root, "tjommie.db");
  if (names.length > 0 && !existsSync(legacyMain)) {
    throw new Error("Legacy SQLite sidecars exist without the legacy database. The database was left untouched.");
  }
  names.sort((a, b) => Number(a === "tjommie.db") - Number(b === "tjommie.db") || a.localeCompare(b));
  for (const name of names) {
    const from = join(paths.root, name);
    const to = join(paths.data, name);
    if (!lstatSync(from).isFile()) throw new Error(`Legacy database path is not a regular file: ${from}`);
    if (existsSync(to)) throw new Error(`Database destination already exists: ${to}`);
  }
  for (const name of names) {
    const from = join(paths.root, name);
    const to = join(paths.data, name);
    renameSync(from, to);
  }
}

// ponytail: on Windows the file mode is ignored; the per-user ACL of %LOCALAPPDATA% protects the token.
export function readOrCreateToken(file: string): string {
  if (existsSync(file)) {
    const existing = readFileSync(file, "utf8").trim();
    if (existing.length >= 43) return existing;
  }
  const token = randomBytes(32).toString("base64url");
  const temporary = `${file}.${randomBytes(12).toString("hex")}.tmp`;
  try {
    writeFileSync(temporary, token, { mode: 0o600, flag: "wx" });
    renameSync(temporary, file);
  } finally {
    rmSync(temporary, { force: true });
  }
  return token;
}
