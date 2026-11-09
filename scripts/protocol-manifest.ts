import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { z } from "zod";
import { ErrorCode, PROTOCOL_VERSION, events, methods } from "../packages/protocol/src/index.ts";

// The wire contract the Rust daemon is checked against: every method and event with its JSON Schema.
// Params are what clients send (before defaults apply); results and event data are what the daemon sends.
// Refinements (IANA time zones, https-only relay URLs, unique names) don't survive the export; Rust tests them by hand.
type Schema = z.core.JSONSchema.BaseSchema | "custom";

function schema(type: z.ZodType, io: "input" | "output"): Schema {
  try {
    return z.toJSONSchema(type, { io, unrepresentable: "throw" });
  } catch {
    return "custom"; // Zod cannot express it (for example a transform); the Rust side gets a hand-written test.
  }
}

export function protocolManifest() {
  return {
    protocolVersion: PROTOCOL_VERSION,
    errorCodes: ErrorCode.options,
    methods: Object.entries(methods).map(([name, method]) => ({ name, params: schema(method.params, "input"), result: schema(method.result, "output") })),
    events: Object.entries(events).map(([name, data]) => ({ name, data: schema(data, "output") })),
  };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const check = args[0] === "--check";
  const file = resolve(args[check ? 1 : 0] ?? fileURLToPath(new URL("../tjommie-rs/protocol/manifest.json", import.meta.url)));
  const text = `${JSON.stringify(protocolManifest(), null, 2)}\n`;
  if (check) {
    let current = "";
    try { current = readFileSync(file, "utf8").replaceAll("\r\n", "\n"); } catch { /* A missing manifest is stale. */ }
    if (current !== text) {
      console.error(`${file} is stale. Run npm run protocol:manifest and commit the result.`);
      process.exitCode = 1;
    }
  } else {
    mkdirSync(dirname(file), { recursive: true });
    writeFileSync(file, text);
  }
}
