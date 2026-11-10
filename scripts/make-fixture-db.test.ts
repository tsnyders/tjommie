import { readFileSync } from "node:fs";
import { expect, it } from "vitest";
import { freshSchema, schemaFile } from "./make-fixture-db.ts";

// Rust checks its fresh database against schema.json; this keeps schema.json equal to a fresh TypeScript one.
it("keeps schema.json equal to the sqlite_master of a fresh TypeScript database", () => {
  expect(JSON.parse(readFileSync(schemaFile, "utf8"))).toEqual(freshSchema());
});
