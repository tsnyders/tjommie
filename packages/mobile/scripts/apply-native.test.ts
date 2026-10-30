import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { expect, it } from "vitest";

const script = fileURLToPath(new URL("./apply-native.mjs", import.meta.url));

it("accepts a mapped Android release version without applying native changes", () => {
  const result = spawnSync(process.execPath, [script, "--check", "0.3.2"], { encoding: "utf8" });
  expect(result.status).toBe(0);
  expect(result.stdout).toContain("Android versionCode mapping found for 0.3.2.");
});

it("rejects a release version without a versionCode", () => {
  const result = spawnSync(process.execPath, [script, "--check", "0.3.99"], { encoding: "utf8" });
  expect(result.status).not.toBe(0);
  expect(result.stderr).toContain("Android versionCode needs a release-version rule for 0.3.99.");
});
