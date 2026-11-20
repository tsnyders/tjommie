import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, it } from "vitest";
import { aegisTheme, aegisThemeText } from "./aegis-theme.ts";

const script = fileURLToPath(new URL("./aegis-theme.ts", import.meta.url));
const run = (args: string[]) => spawnSync(process.execPath, [script, ...args], { encoding: "utf8", windowsHide: true });
const tokens: unknown = JSON.parse(readFileSync(new URL("../design/claude-design/aegis/tokens.json", import.meta.url), "utf8"));

// A design token is an object with a $value (W3C design tokens format); counted here independently of the converter.
const countTokens = (node: unknown): number =>
  typeof node !== "object" || node === null || Array.isArray(node) ? 0
    : "$value" in node ? 1
      : Object.values(node).reduce((sum: number, child: unknown) => sum + countTokens(child), 0);

const at = (node: unknown, path: string): unknown =>
  path.split(".").reduce((value: unknown, key) => typeof value === "object" && value !== null ? (value as Record<string, unknown>)[key] : undefined, node);

it("gives every Aegis token exactly one semantic key", () => {
  const { theme, keys } = aegisTheme(tokens);
  expect(countTokens(tokens)).toBe(221);
  expect(keys).toHaveLength(countTokens(tokens));
  expect(new Set(keys).size).toBe(keys.length);
  for (const key of keys) expect(at(theme, key), key).toBeDefined();
});

it("names colours by role, not by the Aegis palette", () => {
  const { keys } = aegisTheme(tokens);
  expect(keys).toContain("color.light.surface.paper");
  expect(keys).toContain("color.dark.surface.card");
  expect(keys.filter((key) => /stone|white/.test(key))).toEqual([]);
});

it("keeps the calm spring as stiffness and damping ratio", () => {
  expect(at(aegisTheme(tokens).theme, "motion.spring.calm")).toEqual({ stiffness: 260, dampingRatio: 1 });
});

it("fails --check on a stale file and passes on a fresh one", () => {
  const dir = mkdtempSync(join(tmpdir(), "aegis-theme-")), file = join(dir, "aegis.json");
  try {
    writeFileSync(file, "{}\n");
    expect(run(["--check", file]).status).toBe(1);
    writeFileSync(file, aegisThemeText(tokens));
    expect(run(["--check", file]).status).toBe(0);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
