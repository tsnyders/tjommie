import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { z } from "zod";
import {
  ClientMessage, DiagnosticReport, ErrorCode, HostToRelay, MessageData, NativeBrowserCommand, NativeBrowserMount, NativeBrowserStreamMessage, NativeBrowserUiCommand,
  PROTOCOL_VERSION, PairingCode, PushData, RelayToHost, RemoteConfirm, RemotePreface, RemoteReady, RemoteRefused, SpeechEvent, SpeechStart,
  SpeechStatus, VaultHostCommand, events, methods,
} from "../packages/protocol/src/index.ts";
import { VaultHostJob, VaultHostReply } from "../packages/protocol/src/vault-host.ts";

// The wire contract the Rust daemon is checked against: every method and event with its JSON Schema.
// Params are what clients send (before defaults apply); results and event data are what the daemon sends.
// Refinements (IANA time zones, https-only relay URLs, unique names) don't survive the export; Rust tests them by hand.
type Schema = z.core.JSONSchema.BaseSchema | "custom";

// Schemas parsed outside method calls: the WebSocket frame, message payloads, remote and relay frames, the desktop-host bridges.
const types = {
  ClientMessage, MessageData, PairingCode, RemotePreface, RemoteConfirm, RemoteReady, RemoteRefused, RelayToHost, HostToRelay, PushData,
  NativeBrowserCommand, NativeBrowserUiCommand, NativeBrowserMount, NativeBrowserStreamMessage, VaultHostCommand, VaultHostJob, VaultHostReply, DiagnosticReport,
  SpeechStatus, SpeechEvent, SpeechStart,
};

// Zod trims a string before it checks its length: with .trim(), and inside its URL check. JSON Schema can't say that, so
// trimmed strings carry "x-trim" and Rust trims them before it validates.
function markTrim(ctx: { zodSchema: z.core.$ZodTypes; jsonSchema: z.core.JSONSchema.BaseSchema }): void {
  const def = ctx.zodSchema._zod.def;
  if (def.type !== "string") return;
  const isUrl = (value: object): boolean => "format" in value && value.format === "url";
  const trims = isUrl(def) || (def.checks ?? []).some((check) => {
    const checkDef = (check as z.core.$ZodCheckOverwrite<string>)._zod.def;
    return isUrl(checkDef) || (checkDef.check === "overwrite" && checkDef.tx(" x ") === "x");
  });
  if (trims) ctx.jsonSchema["x-trim"] = true;
}

function schema(type: z.ZodType, io: "input" | "output"): Schema {
  try {
    return z.toJSONSchema(type, { io, unrepresentable: "throw", override: markTrim });
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
    types: Object.entries(types).map(([name, type]) => ({ name, schema: schema(type, "input") })),
  };
}

export function manifestText(): string {
  return `${JSON.stringify(protocolManifest(), null, 2)}\n`;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  const check = args[0] === "--check";
  const file = resolve(args[check ? 1 : 0] ?? fileURLToPath(new URL("../tjommie-rs/protocol/manifest.json", import.meta.url)));
  const text = manifestText();
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
