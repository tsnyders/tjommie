import { readFileSync, readdirSync } from "node:fs";
import { expect, it } from "vitest";
import { migrations } from "../packages/daemon/src/db.ts";
import { migrationFileName, migrationsDir } from "./export-migrations.ts";

it("keeps one .sql file per TypeScript migration, byte for byte", () => {
  // Rust may not add migration 13 until one release after R1 (spec §5), so twelve is the whole set.
  expect(migrations).toHaveLength(12);
  expect(readdirSync(migrationsDir).sort()).toEqual(migrations.map((_, index) => migrationFileName(index)));
  for (const [index, sql] of migrations.entries()) {
    expect(readFileSync(`${migrationsDir}/${migrationFileName(index)}`).equals(Buffer.from(sql, "utf8")), migrationFileName(index)).toBe(true);
  }
});
