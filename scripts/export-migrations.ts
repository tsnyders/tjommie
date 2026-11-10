import { mkdirSync, writeFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { migrations } from "../packages/daemon/src/db.ts";

// Writes db.ts `migrations` as the .sql files the Rust daemon embeds, byte for byte (SQLite keeps the text in sqlite_master).
export const migrationsDir = fileURLToPath(new URL("../tjommie-rs/crates/daemon/migrations", import.meta.url));
export const migrationFileName = (index: number): string => `${String(index + 1).padStart(2, "0")}.sql`;

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  mkdirSync(migrationsDir, { recursive: true });
  for (const [index, sql] of migrations.entries()) writeFileSync(`${migrationsDir}/${migrationFileName(index)}`, sql);
}
