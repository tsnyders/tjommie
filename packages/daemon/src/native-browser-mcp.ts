import type { IncomingMessage, ServerResponse } from "node:http";
import { z } from "zod";
import type { NativeBrowserCommand } from "../../protocol/src/native-browser.ts";
import { nativeBrowserTools, validateNativeBrowserTool } from "../../desktop/src/native-browser-automation.ts";
import type { StoredBot } from "./bots.ts";
import type { McpTokens } from "./mcp.ts";

type Options = { tokens: McpTokens; getBot: (id: string) => StoredBot; request: (botId: string, command: NativeBrowserCommand, authorized?: () => boolean) => Promise<unknown>; cancelTools?: (botId: string) => void };
type Answer = { jsonrpc: "2.0"; id: string | number | null; result?: unknown; error?: { code: number; message: string } };
const record = (value: unknown): Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value) ? value as Record<string, unknown> : {};
const failed = (message: string) => ({ isError: true, content: [{ type: "text", text: message.slice(0, 4000) }] });
const resultSchema = z.object({ content: z.array(z.discriminatedUnion("type", [
  z.object({ type: z.literal("text"), text: z.string().max(128 * 1024 + 100) }).strict(),
  z.object({ type: z.literal("image"), data: z.string().max(Math.ceil(2 * 1024 * 1024 / 3) * 4).regex(/^[A-Za-z0-9+/]*={0,2}$/), mimeType: z.enum(["image/png", "image/jpeg", "image/webp"]) }).strict(),
])).max(8), isError: z.boolean().optional() }).strict();
const listed = Object.entries(nativeBrowserTools).map(([name, tool]) => ({ name, description: tool.description, inputSchema: z.toJSONSchema(tool.input, { target: "draft-7" }) }));

/** A token-scoped MCP proxy. Native WebContents and input serialization belong to the desktop engine. */
export class NativeBrowserMcp {
  #options: Options;
  #epochs = new Map<string, number>();
  #tails = new Map<string, Promise<void>>();
  #closed = false;
  constructor(options: Options) { this.#options = options; }
  async #queued<T>(botId: string, bypass: boolean, task: () => Promise<T>): Promise<T> {
    if (bypass) return task();
    const pending = (this.#tails.get(botId) ?? Promise.resolve()).then(task);
    const tail = pending.then(() => {}, () => {});
    this.#tails.set(botId, tail);
    try { return await pending; }
    finally { if (this.#tails.get(botId) === tail) this.#tails.delete(botId); }
  }
  #authorized(token: string, botId: string, epoch: number): boolean {
    try { return !this.#closed && this.#options.tokens.botFor(token) === botId && !!this.#options.getBot(botId).browser && (this.#epochs.get(botId) ?? 0) === epoch; }
    catch { return false; }
  }
  async #answer(token: string, botId: string, epoch: number, raw: unknown): Promise<Answer | null> {
    const message = record(raw);
    if (!Object.keys(message).length) return { jsonrpc: "2.0", id: null, error: { code: -32600, message: "Invalid request" } };
    if (!Object.hasOwn(message, "id")) return null;
    const { id, method } = message;
    if ((typeof id !== "string" && typeof id !== "number") || typeof method !== "string") return { jsonrpc: "2.0", id: null, error: { code: -32600, message: "Invalid request" } };
    const ok = (result: unknown): Answer => ({ jsonrpc: "2.0", id, result });
    const unauthorized = (): Answer => ({ jsonrpc: "2.0", id, error: { code: -32001, message: "Browser session is no longer authorized" } });
    if (!this.#authorized(token, botId, epoch)) return unauthorized();
    const params = record(message.params);
    if (method === "initialize") return ok({ protocolVersion: typeof params.protocolVersion === "string" ? params.protocolVersion : "2025-06-18", capabilities: { tools: {} }, serverInfo: { name: "browser", version: "0.1.0" } });
    if (method === "ping") return ok({});
    if (method === "tools/list") return ok({ tools: listed });
    if (method !== "tools/call") return { jsonrpc: "2.0", id, error: { code: -32601, message: "Method not found" } };
    if (typeof params.name !== "string" || !Object.hasOwn(nativeBrowserTools, params.name)) return { jsonrpc: "2.0", id, error: { code: -32602, message: "Unknown native browser tool" } };
    try {
      const command = validateNativeBrowserTool(params.name, params.arguments ?? {});
      if (!this.#authorized(token, botId, epoch)) return unauthorized();
      const rawResult = await this.#queued(botId, command.name === "browser_handle_dialog", async () => {
        if (!this.#authorized(token, botId, epoch)) throw new Error("Browser session is no longer authorized");
        return this.#options.request(botId, command, () => this.#authorized(token, botId, epoch));
      });
      if (!this.#authorized(token, botId, epoch)) return unauthorized();
      // Do not trust arbitrary bridge output or expose raw CDP objects to a model.
      const parsed = resultSchema.safeParse(rawResult);
      if (!parsed.success || JSON.stringify(parsed.data).length > 4 * 1024 * 1024) return ok(failed("The desktop returned an invalid or oversized browser result."));
      return ok(parsed.data);
    } catch (error) {
      if (!this.#authorized(token, botId, epoch)) return unauthorized();
      return ok(failed(error instanceof Error ? error.message : "The native browser tool failed."));
    }
  }
  handle = async (req: IncomingMessage, res: ServerResponse): Promise<void> => {
    const header = req.headers.authorization ?? "", token = header.startsWith("Bearer ") ? header.slice(7) : "";
    const botId = this.#options.tokens.botFor(token);
    if (!botId || this.#closed) { res.writeHead(401).end(); return; }
    let bot: StoredBot;
    try { bot = this.#options.getBot(botId); } catch { res.writeHead(401).end(); return; }
    if (!bot.browser) { res.writeHead(403).end(); return; }
    if (req.method !== "POST") { res.writeHead(405, { allow: "POST" }).end(); return; }
    const epoch = this.#epochs.get(botId) ?? 0, chunks: Buffer[] = [];
    this.#epochs.set(botId, epoch);
    let size = 0;
    const body: AsyncIterable<unknown> = req;
    for await (const chunk of body) {
      if (!Buffer.isBuffer(chunk)) { res.writeHead(400).end(); return; }
      size += chunk.length;
      if (size > 1024 * 1024) { res.writeHead(413).end(); return; }
      chunks.push(chunk);
    }
    let input: unknown;
    try { input = JSON.parse(Buffer.concat(chunks).toString("utf8")); } catch { res.writeHead(400).end(); return; }
    const messages: unknown[] = Array.isArray(input) ? input : [input];
    if (!messages.length || messages.length > 20) { res.writeHead(400).end(); return; }
    // Queue normal calls to recheck authorization immediately before dispatch. Dialog dismissal
    // bypasses that queue so it can unblock an action. Close invalidates queued and late replies.
    let replyBytes = 0;
    const answers = (await Promise.all(messages.map((message) => this.#answer(token, botId, epoch, message)))).filter((answer) => answer !== null).map((answer) => {
      replyBytes += Buffer.byteLength(JSON.stringify(answer));
      return replyBytes > 8 * 1024 * 1024 ? { jsonrpc: "2.0", id: answer.id, result: failed("This batch exceeds the 8 MiB browser reply limit. Request large results individually.") } : answer;
    });
    if (!answers.length) { res.writeHead(202).end(); return; }
    res.writeHead(200, { "content-type": "application/json", "cache-control": "no-store" }).end(JSON.stringify(Array.isArray(input) ? answers : answers[0]));
  };
  close(botId: string): Promise<void> { this.#epochs.set(botId, (this.#epochs.get(botId) ?? 0) + 1); this.#options.cancelTools?.(botId); return Promise.resolve(); }
  closeAll(): Promise<void> { this.#closed = true; for (const id of new Set([...this.#epochs.keys(), ...this.#tails.keys()])) this.#options.cancelTools?.(id); return Promise.resolve(); }
}
