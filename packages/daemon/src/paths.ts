import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { randomBytes } from "node:crypto";
import { homedir } from "node:os";
import { join } from "node:path";

export const DEFAULT_PORT = 47100;

export type Paths = { root: string; db: string; token: string; bots: string; accounts: string; logs: string };

const defaultRoot = () => process.env.TJOMMIE_HOME ?? join(process.env.LOCALAPPDATA ?? join(homedir(), ".local", "share"), "Tjommie");

export function dataPaths(root: string = defaultRoot()): Paths {
  const p = {
    root,
    db: join(root, "tjommie.db"),
    token: join(root, "daemon-token"),
    bots: join(root, "bots"),
    accounts: join(root, "accounts"),
    logs: join(root, "logs"),
  };
  for (const dir of [p.root, p.bots, p.accounts, p.logs]) mkdirSync(dir, { recursive: true });
  return p;
}

// ponytail: on Windows the file mode is ignored; the per-user ACL of %LOCALAPPDATA% protects the token.
export function readOrCreateToken(file: string): string {
  if (existsSync(file)) return readFileSync(file, "utf8").trim();
  const token = randomBytes(32).toString("base64url");
  writeFileSync(file, token, { mode: 0o600 });
  return token;
}
