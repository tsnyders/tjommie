import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { z } from "zod";
import { nativeBrowserTools } from "../packages/desktop/src/native-browser-automation.ts";

// The native browser tools for the Rust daemon's /mcp/browser. inputSchema is exactly what native-browser-mcp.ts lists
// (zod's default output view, where a field with a default counts as required); argumentsSchema is zod's input view,
// what a caller may send, which the daemon validates against. The cross-field rules in validateNativeBrowserTool are
// ported by hand.
export function browserTools(): { name: string; description: string; inputSchema: unknown; argumentsSchema: unknown }[] {
  return Object.entries(nativeBrowserTools).map(([name, tool]) => ({
    name, description: tool.description,
    inputSchema: z.toJSONSchema(tool.input, { target: "draft-7" }),
    argumentsSchema: z.toJSONSchema(tool.input, { target: "draft-7", io: "input" }),
  }));
}

export function browserToolsText(): string {
  return `${JSON.stringify(browserTools(), null, 2)}\n`;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const check = args[0] === "--check";
  const file = resolve(args[check ? 1 : 0] ?? fileURLToPath(new URL("../tjommie-rs/protocol/browser-tools.json", import.meta.url)));
  const text = browserToolsText();
  if (check) {
    let current = "";
    try { current = readFileSync(file, "utf8").replaceAll("\r\n", "\n"); } catch { /* A missing file is stale. */ }
    if (current !== text) {
      console.error(`${file} is stale. Run npm run browser-tools:manifest and commit the result.`);
      process.exitCode = 1;
    }
  } else {
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, text);
  }
}
