import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { randomUUID } from "node:crypto";
import { mkdirSync } from "node:fs";
import { createServer } from "node:http";
import { join } from "node:path";
import { createInterface } from "node:readline";
import { expect, it } from "vitest";
import { cliEnv, killTreeAndWait, resolveCli } from "./cli.ts";
import { tempRoot } from "./testing.ts";

function record(value: unknown): value is Record<string, unknown> { return typeof value === "object" && value !== null && !Array.isArray(value); }

// An installed CLI, a disposable profile, a synthetic key and an owned loopback API.
// No provider account/network is used. The fixture model requests only this exact read-only command.
it("Claude removes its explicit API key from a real Bash subprocess", async () => {
  const command = resolveCli("claude"); expect(command).not.toBeNull();
  const root = tempRoot(), config = join(root, "config"); mkdirSync(config);
  const key = `synthetic-${randomUUID()}`;
  const check = `"${process.execPath.replaceAll("\\", "/")}" -e "process.stdout.write(JSON.stringify({apiKeyPresent:typeof process.env.ANTHROPIC_API_KEY==='string',openaiKeyPresent:typeof process.env.OPENAI_API_KEY==='string'}))"`;
  let toolResult = "", messageCalls = 0, authenticated = false;
  const errors: string[] = [];
  const server = createServer((req, res) => {
    // Proxy requests are never forwarded. Every request is answered locally.
    if (!req.url?.startsWith("/")) { res.writeHead(403).end(); return; }
    let body = "";
    req.on("data", (part: Buffer) => { if (body.length > 2_000_000) req.destroy(); else body += part.toString("utf8"); });
    req.on("end", () => {
      if (req.url?.includes("count_tokens")) { res.writeHead(200, { "Content-Type": "application/json" }).end('{"input_tokens":1}'); return; }
      if (!req.url?.startsWith("/v1/messages")) { res.writeHead(404).end("{}"); return; }
      authenticated ||= req.headers["x-api-key"] === key;
      let input: unknown; try { input = JSON.parse(body); } catch { res.writeHead(400).end(); return; }
      if (!record(input)) { res.writeHead(400).end(); return; }
      messageCalls++;
      if (messageCalls > 5) { res.writeHead(429).end(); return; }
      for (const message of Array.isArray(input.messages) ? input.messages : []) {
        if (!record(message) || !Array.isArray(message.content)) continue;
        for (const content of message.content) {
          if (record(content) && content.type === "tool_result") toolResult = typeof content.content === "string" ? content.content : JSON.stringify(content.content);
        }
      }
      const complete = toolResult !== "";
      const block = complete ? { type: "text", text: "Synthetic check finished." } : { type: "tool_use", id: "synthetic-tool", name: "Bash", input: { command: check, description: "Report synthetic key-presence booleans" } };
      const message = { id: `msg_fixture_${messageCalls}`, type: "message", role: "assistant", model: "claude-haiku-4-5-20251001", content: [block], stop_reason: complete ? "end_turn" : "tool_use", stop_sequence: null, usage: { input_tokens: 1, output_tokens: 1 } };
      if (input.stream !== true) { res.writeHead(200, { "Content-Type": "application/json" }).end(JSON.stringify(message)); return; }
      res.writeHead(200, { "Content-Type": "text/event-stream" });
      const event = (type: string, data: Record<string, unknown>) => res.write(`event: ${type}\ndata: ${JSON.stringify({ type, ...data })}\n\n`);
      event("message_start", { message: { ...message, content: [], stop_reason: null } });
      event("content_block_start", { index: 0, content_block: complete ? { type: "text", text: "" } : { type: "tool_use", id: "synthetic-tool", name: "Bash", input: {} } });
      event("content_block_delta", { index: 0, delta: complete ? { type: "text_delta", text: "Synthetic check finished." } : { type: "input_json_delta", partial_json: JSON.stringify(block.input) } });
      event("content_block_stop", { index: 0 });
      event("message_delta", { delta: { stop_reason: message.stop_reason, stop_sequence: null }, usage: { output_tokens: 1 } });
      event("message_stop", {}); res.end();
    });
  });
  server.on("connect", (_req, socket) => socket.end("HTTP/1.1 403 Forbidden\r\n\r\n"));
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address(); if (!address || typeof address === "string") throw new Error("Missing fixture address");
  const endpoint = `http://127.0.0.1:${address.port}`;
  let child: ChildProcessWithoutNullStreams | undefined;
  let timer: NodeJS.Timeout | undefined;
  try {
    child = spawn(command!.file, [...command!.args, "-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose", "--permission-prompt-tool", "stdio", "--permission-mode", "default", "--tools", "Bash", "--model", "claude-haiku-4-5-20251001", "--max-turns", "3"], {
      cwd: root, windowsHide: true, stdio: ["pipe", "pipe", "pipe"], env: {
        ...cliEnv({ CLAUDE_CONFIG_DIR: config }), HOME: root, USERPROFILE: root, APPDATA: root, LOCALAPPDATA: root,
        ANTHROPIC_API_KEY: key, ANTHROPIC_BASE_URL: endpoint, CLAUDE_CODE_SUBPROCESS_ENV_SCRUB: "1", CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST: "1",
        CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1", DISABLE_AUTOUPDATER: "1", DISABLE_TELEMETRY: "1", DISABLE_ERROR_REPORTING: "1", CLAUDE_CODE_SIMPLE: "1",
        HTTP_PROXY: endpoint, HTTPS_PROXY: endpoint, ALL_PROXY: endpoint, NO_PROXY: "localhost,127.0.0.1",
      },
    });
    const owned = child;
    owned.stderr.on("data", () => {});
    owned.stdin.on("error", () => {});
    const finished = new Promise<void>((resolve, reject) => {
      owned.once("error", () => reject(new Error("Synthetic Claude fixture could not start")));
      owned.once("exit", (code) => code === 0 ? resolve() : reject(new Error(`Synthetic Claude fixture exited ${code}; stages=${messageCalls}, toolResult=${toolResult ? "present" : "absent"}`)));
      timer = setTimeout(() => reject(new Error(`Synthetic Claude fixture timed out; stages=${messageCalls}, toolResult=${toolResult ? "present" : "absent"}`)), 45_000);
      createInterface({ input: owned.stdout }).on("line", (line) => {
        let value: unknown; try { value = JSON.parse(line); } catch { return; }
        if (!record(value)) return;
        if (value.type === "control_request" && record(value.request) && value.request.subtype === "can_use_tool") {
          const allowed = value.request.tool_name === "Bash" && record(value.request.input) && value.request.input.command === check;
          if (!allowed) errors.push("Unexpected tool approval");
          owned.stdin.write(`${JSON.stringify({ type: "control_response", response: { subtype: "success", request_id: value.request_id,
            response: allowed ? { behavior: "allow", updatedInput: value.request.input } : { behavior: "deny", message: "Only the exact synthetic check is allowed." } } })}\n`);
        }
        if (value.type === "result") { owned.stdin.end(); resolve(); }
      });
    });
    owned.stdin.write(`${JSON.stringify({ type: "user", message: { role: "user", content: "Run the exact synthetic key-presence check once with Bash, then finish." } })}\n`);
    await finished;
    expect(errors).toEqual([]); expect(authenticated).toBe(true);
    // Assert only booleans. Never print credential values or request headers.
    expect(toolResult).toContain('"apiKeyPresent":false');
    expect(toolResult).toContain('"openaiKeyPresent":false');
  } finally {
    clearTimeout(timer);
    if (child && child.exitCode === null && child.signalCode === null) {
      const owned = child;
      await Promise.race([new Promise<void>((resolve) => owned.once("exit", () => resolve())), new Promise<void>((resolve) => setTimeout(resolve, 1000))]);
      if (owned.exitCode === null && owned.signalCode === null) await killTreeAndWait(owned.pid);
    }
    server.closeAllConnections(); await new Promise<void>((resolve) => server.close(() => resolve()));
  }
}, 60_000);
