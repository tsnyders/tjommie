import { randomBytes } from "node:crypto";
import type { IncomingMessage, ServerResponse } from "node:http";
import { z } from "zod";

const MAX_BODY_BYTES = 1024 * 1024;
const FALLBACK_PROTOCOL = "2025-06-18";

export type McpTool = {
  name: string;
  description: string;
  input: z.ZodObject;
  // Arguments are validated with input before run. Thrown errors become failed calls for the bot.
  run(botId: string, args: unknown): string | Promise<string>;
};

// Keeps each tool's run typed by its own input schema.
export const defineTool = <S extends z.ZodObject>(tool: {
  name: string;
  description: string;
  input: S;
  run(botId: string, args: z.output<S>): string | Promise<string>;
}): McpTool => tool;

// One bearer token per running CLI session. It tells the endpoint which bot is calling.
export class McpTokens {
  #bots = new Map<string, string>(); // token -> bot id

  issue(botId: string): string {
    this.revoke(botId);
    const token = randomBytes(24).toString("base64url");
    this.#bots.set(token, botId);
    return token;
  }

  revoke(botId: string): void {
    for (const [token, id] of this.#bots) if (id === botId) this.#bots.delete(token);
  }

  botFor(token: string): string | null {
    return this.#bots.get(token) ?? null;
  }
}

type Answer = { jsonrpc: "2.0"; id: string | number | null; result?: unknown; error?: { code: number; message: string } };

// A stateless MCP endpoint (Streamable HTTP, plain JSON answers). Both CLIs only need initialize, ping,
// tools/list and tools/call. Notifications get 202 and no body.
export function mcpHandler(opts: { tokens: McpTokens; tools: McpTool[] }): (req: IncomingMessage, res: ServerResponse) => Promise<void> {
  const listed = opts.tools.map((tool) => ({ name: tool.name, description: tool.description, inputSchema: jsonSchema(tool.input) }));

  async function answer(botId: string, token: string, message: unknown): Promise<Answer | null> {
    if (!isRecord(message)) return invalidRequest(null);
    if (!Object.hasOwn(message, "id")) return null; // A notification, including clients that omit jsonrpc.
    const { id, method } = message;
    if (!isId(id) || typeof method !== "string") return invalidRequest(isId(id) ? id : null);
    const params = isRecord(message.params) ? message.params : undefined;
    const ok = (result: unknown): Answer => ({ jsonrpc: "2.0", id, result });
    switch (method) {
      case "initialize":
        return ok({
          protocolVersion: typeof params?.protocolVersion === "string" ? params.protocolVersion : FALLBACK_PROTOCOL,
          capabilities: { tools: {} },
          serverInfo: { name: "tjommie", version: "0.1.0" },
        });
      case "ping":
        return ok({});
      case "tools/list":
        return ok({ tools: listed });
      case "tools/call": {
        const name = typeof params?.name === "string" ? params.name : undefined;
        const tool = opts.tools.find((t) => t.name === name);
        if (!tool) return { jsonrpc: "2.0", id, error: { code: -32602, message: `Unknown tool ${String(name)}` } };
        const args = tool.input.safeParse(params?.arguments ?? {});
        if (!args.success) return ok(text(z.prettifyError(args.error), true));
        if (opts.tokens.botFor(token) !== botId) {
          return { jsonrpc: "2.0", id, error: { code: -32001, message: "Session token is no longer valid" } };
        }
        try {
          return ok(text(await tool.run(botId, args.data), false));
        } catch (error) {
          return ok(text(error instanceof Error ? error.message : String(error), true));
        }
      }
      default:
        return { jsonrpc: "2.0", id, error: { code: -32601, message: `Method not found: ${method}` } };
    }
  }

  return async (req, res) => {
    if (req.method !== "POST") {
      res.writeHead(405, { allow: "POST" }).end();
      return;
    }
    const auth = req.headers.authorization ?? "";
    const token = auth.startsWith("Bearer ") ? auth.slice(7) : "";
    const botId = opts.tokens.botFor(token);
    if (!botId) {
      res.writeHead(401).end();
      return;
    }
    let body: unknown;
    try {
      body = JSON.parse(await readBody(req));
    } catch (error) {
      if (error instanceof TooLarge) res.writeHead(413).end();
      else send(res, 400, { jsonrpc: "2.0", id: null, error: { code: -32700, message: "Parse error" } });
      return;
    }
    const messages: unknown[] = Array.isArray(body) ? body : [body];
    const answers = (await Promise.all(messages.map((message) => answer(botId, token, message)))).filter((a) => a !== null);
    if (answers.length === 0) res.writeHead(202).end();
    else send(res, 200, Array.isArray(body) ? answers : answers[0]);
  };
}

const text = (value: string, isError: boolean) => ({ content: [{ type: "text", text: value }], isError });

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isId(value: unknown): value is Answer["id"] {
  return value === null || typeof value === "string" || (typeof value === "number" && Number.isFinite(value));
}

function invalidRequest(id: Answer["id"]): Answer {
  return { jsonrpc: "2.0", id, error: { code: -32600, message: "Invalid request" } };
}

function jsonSchema(schema: z.ZodObject): Record<string, unknown> {
  const out = z.toJSONSchema(schema, { io: "input" }) as Record<string, unknown>;
  delete out.$schema;
  return out;
}

function send(res: ServerResponse, status: number, body: unknown): void {
  res.writeHead(status, { "content-type": "application/json" }).end(JSON.stringify(body));
}

class TooLarge extends Error {}

async function readBody(req: IncomingMessage): Promise<string> {
  const chunks: Buffer[] = [];
  let size = 0;
  const stream: AsyncIterable<unknown> = req;
  for await (const chunk of stream) {
    if (!Buffer.isBuffer(chunk)) throw new Error("Expected a request body buffer");
    size += chunk.length;
    if (size > MAX_BODY_BYTES) throw new TooLarge();
    chunks.push(chunk);
  }
  return Buffer.concat(chunks).toString("utf8");
}
