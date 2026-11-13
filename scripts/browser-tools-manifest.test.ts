import { spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, it } from "vitest";
import { nativeBrowserTools } from "../packages/desktop/src/native-browser-automation.ts";
import { browserTools, browserToolsText } from "./browser-tools-manifest.ts";

const script = fileURLToPath(new URL("./browser-tools-manifest.ts", import.meta.url));
const run = (args: string[]) => spawnSync(process.execPath, [script, ...args], { encoding: "utf8", windowsHide: true });

it("lists every native browser tool as the MCP proxy does, in order", () => {
  const tools = browserTools();
  expect(tools.map((tool) => tool.name)).toEqual(Object.keys(nativeBrowserTools));
  expect(tools).toHaveLength(26);
  expect(tools.find((tool) => tool.name === "browser_click")).toMatchObject({ inputSchema: { type: "object", additionalProperties: false } });
  expect(JSON.stringify(tools)).not.toContain("browser_run_code");
});

it("keeps the listed schema and the accepted arguments apart where a field has a default", () => {
  const messages = browserTools().find((tool) => tool.name === "browser_console_messages");
  expect(messages?.inputSchema).toMatchObject({ required: ["level"] });
  expect(messages?.argumentsSchema).not.toHaveProperty("required");
});

it("fails --check on a stale file and passes on a fresh one", () => {
  const dir = mkdtempSync(join(tmpdir(), "browser-tools-")), file = join(dir, "browser-tools.json");
  try {
    writeFileSync(file, "[]\n");
    expect(run(["--check", file]).status).toBe(1);
    writeFileSync(file, browserToolsText());
    expect(run(["--check", file]).status).toBe(0);
  } finally { rmSync(dir, { recursive: true, force: true }); }
});
