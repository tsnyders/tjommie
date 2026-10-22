import { createRequire } from "node:module";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import type { IncomingMessage, ServerResponse } from "node:http";
import type { BrowserContext } from "playwright-core";
import type { StoredBot } from "./bots.ts";
import { BrowserManager, browsers } from "./browser-manager.ts";
import type { McpTokens } from "./mcp.ts";
import { settleCleanup } from "./cleanup.ts";

const require = createRequire(import.meta.url);
const isRecord = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
type Pending = { resolve: (value: unknown) => void; reject: (error: Error) => void; timer: NodeJS.Timeout };
type NativeServer = { connect(transport: BrowserTransport): Promise<void>; close(): Promise<void> };
type McpRuntime = { createConnection(config: Record<string, unknown>, contextGetter: () => Promise<BrowserContext>): Promise<NativeServer> };

// In-memory transport: neither a CDP TCP port nor an unauthenticated MCP socket is opened.
// The standalone MCP SDK is only a devDependency upstream; Playwright bundles its own SDK runtime.
export class BrowserTransport {
  onmessage?: (message: Record<string, unknown>) => void;
  onclose?: () => void;
  onerror?: (error: Error) => void;
  #pending = new Map<number, Pending>();
  #next = 0;
  #closed = false;
  #root: string;
  #timeout: () => void;
  constructor(root: string, timeout: () => void = () => {}) { this.#root = root; this.#timeout = timeout; }
  async start(): Promise<void> {}
  async send(message: unknown): Promise<void> {
    if (!isRecord(message)) return;
    if (typeof message.method === "string" && (typeof message.id === "number" || typeof message.id === "string")) {
      const result = message.method === "roots/list" ? { roots: [{ uri: pathToFileURL(this.#root).href, name: "Bot home" }] } : {};
      this.onmessage?.({ jsonrpc: "2.0", id: message.id, result });
      return;
    }
    if (typeof message.id !== "number") return;
    const pending = this.#pending.get(message.id);
    if (!pending) return;
    this.#pending.delete(message.id);
    clearTimeout(pending.timer);
    if (isRecord(message.error)) pending.reject(new Error(typeof message.error.message === "string" ? message.error.message : "Browser tool failed."));
    else pending.resolve(message.result);
  }
  request(method: string, params: Record<string, unknown> = {}): Promise<unknown> {
    if (this.#closed) return Promise.reject(new Error("The browser tool session closed."));
    const id = ++this.#next;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.onmessage?.({ jsonrpc: "2.0", method: "notifications/cancelled", params: { requestId: id, reason: "Browser tool timed out" } });
        this.#pending.delete(id);
        reject(new Error("The browser tool timed out."));
        this.#timeout(); // close the owned context so an abandoned operation cannot compete with later input
      }, 90_000);
      this.#pending.set(id, { resolve, reject, timer });
      this.onmessage?.({ jsonrpc: "2.0", id, method, params });
    });
  }
  async close(): Promise<void> {
    if (this.#closed) return;
    this.#closed = true;
    for (const pending of this.#pending.values()) { clearTimeout(pending.timer); pending.reject(new Error("The browser tool session closed.")); }
    this.#pending.clear();
    this.onclose?.();
  }
}

type Connection = { server: NativeServer; transport: BrowserTransport; releaseDownloads: () => void };
type Options = { tokens: McpTokens; getBot: (id: string) => StoredBot; manager?: BrowserManager; createConnection?: McpRuntime["createConnection"] };

export class BrowserMcp {
  #options: Options;
  #manager: BrowserManager;
  #connections = new Map<string, Promise<Connection>>();
  #closing = new Set<Promise<void>>();
  constructor(options: Options) { this.#options = options; this.#manager = options.manager ?? browsers; }

  #connection(bot: StoredBot): Promise<Connection> {
    const current = this.#connections.get(bot.id);
    if (current) return current;
    const opening = (async () => {
      const runtime = require("@playwright/mcp") as McpRuntime;
      const leases = new Map<BrowserContext, () => void>();
      const releaseDownloads = (): void => { for (const release of leases.values()) release(); leases.clear(); };
      const server = await (this.#options.createConnection ?? runtime.createConnection)({
        browser: { isolated: false }, sharedBrowserContext: true,
        outputDir: join(bot.homeFolder, "browser-files"), saveSession: false,
        timeouts: { action: 10_000, navigation: 20_000, idle: 0 },
      }, async () => {
        const context = await this.#manager.context(this.#options.getBot(bot.id));
        if (!leases.has(context)) leases.set(context, this.#manager.delegateDownloads(context));
        return context;
      });
      const transport = new BrowserTransport(bot.homeFolder, () => { void this.#manager.close(bot.id).catch(() => {}); });
      try {
        await server.connect(transport);
        await transport.request("initialize", { protocolVersion: "2025-06-18", capabilities: { roots: {} }, clientInfo: { name: "tjommie", version: "0.1.0" } });
        transport.onmessage?.({ jsonrpc: "2.0", method: "notifications/initialized" });
        return { server, transport, releaseDownloads };
      } catch (error) { try { await server.close(); } finally { releaseDownloads(); } throw error; }
    })();
    this.#connections.set(bot.id, opening);
    void opening.catch(() => { if (this.#connections.get(bot.id) === opening) this.#connections.delete(bot.id); });
    return opening;
  }

  async #answer(token: string, bot: StoredBot, message: unknown): Promise<unknown | null> {
    if (!isRecord(message)) return { jsonrpc: "2.0", id: null, error: { code: -32600, message: "Invalid request" } };
    if (!("id" in message)) return null;
    const id = message.id;
    if ((typeof id !== "string" && typeof id !== "number") || typeof message.method !== "string") return { jsonrpc: "2.0", id: null, error: { code: -32600, message: "Invalid request" } };
    const ok = (result: unknown) => ({ jsonrpc: "2.0", id, result });
    const params = isRecord(message.params) ? message.params : {};
    if (this.#options.tokens.botFor(token) !== bot.id || !this.#options.getBot(bot.id).browser) return { jsonrpc: "2.0", id, error: { code: -32001, message: "Browser session is no longer authorized" } };
    if (message.method === "initialize") return ok({ protocolVersion: typeof params.protocolVersion === "string" ? params.protocolVersion : "2025-06-18", capabilities: { tools: {} }, serverInfo: { name: "browser", version: "0.1.0" } });
    if (message.method === "ping") return ok({});
    if (message.method === "tools/list") return ok(await (await this.#connection(bot)).transport.request("tools/list"));
    if (message.method !== "tools/call") return { jsonrpc: "2.0", id, error: { code: -32601, message: "Method not found" } };
    try {
      const result = await this.#manager.automation(bot, async () => {
        // Revocation or disable can happen while the call is queued behind another operation.
        if (this.#options.tokens.botFor(token) !== bot.id || !this.#options.getBot(bot.id).browser) throw new Error("Browser session is no longer authorized");
        const { transport } = await this.#connection(bot);
        const index = await this.#manager.activeIndex(bot);
        if (index >= 0) await transport.request("tools/call", { name: "browser_tabs", arguments: { action: "select", index } });
        const value = await transport.request("tools/call", params);
        const tabs = await transport.request("tools/call", { name: "browser_tabs", arguments: { action: "list" } });
        if (isRecord(tabs) && Array.isArray(tabs.content)) {
          for (const item of tabs.content) {
            if (!isRecord(item) || typeof item.text !== "string") continue;
            const match = /^- (\d+): \(current\)/m.exec(item.text);
            if (match) await this.#manager.selectAutomationIndex(bot, Number(match[1]));
          }
        }
        return value;
      });
      return ok(result);
    } catch (error) {
      return ok({ isError: true, content: [{ type: "text", text: error instanceof Error ? error.message : "Browser tool failed." }] });
    }
  }

  handle = async (req: IncomingMessage, res: ServerResponse): Promise<void> => {
    const header = req.headers.authorization ?? "";
    const token = header.startsWith("Bearer ") ? header.slice(7) : "";
    const botId = this.#options.tokens.botFor(token);
    if (!botId) { res.writeHead(401).end(); return; }
    let bot: StoredBot;
    try { bot = this.#options.getBot(botId); } catch { res.writeHead(401).end(); return; }
    if (!bot.browser) { res.writeHead(403).end(); return; }
    if (req.method !== "POST") { res.writeHead(405, { allow: "POST" }).end(); return; }
    const chunks: Buffer[] = [];
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
    if (messages.length > 20) { res.writeHead(400).end(); return; }
    const answers: unknown[] = [];
    for (const message of messages) {
      const answer = await this.#answer(token, bot, message);
      if (answer !== null) answers.push(answer);
    }
    if (!answers.length) { res.writeHead(202).end(); return; }
    res.writeHead(200, { "content-type": "application/json", "cache-control": "no-store" }).end(JSON.stringify(Array.isArray(input) ? answers : answers[0]));
  };

  close(botId: string): Promise<void> {
    const pending = this.#connections.get(botId);
    this.#connections.delete(botId);
    if (!pending) return Promise.resolve();
    const closing = (async () => {
      const connection = await pending.catch(() => null);
      if (!connection) return; // failed initialization has no live server to tear down
      try { await connection.server.close(); }
      finally {
        // The SDK schedules backend listener disposal from its close notification. Drain those
        // microtasks before handing downloads back to the manager (session/video recording are off).
        await new Promise<void>((resolve) => setImmediate(resolve));
        connection.releaseDownloads();
      }
    })();
    this.#closing.add(closing);
    void closing.then(() => this.#closing.delete(closing), () => this.#closing.delete(closing));
    return closing;
  }
  async closeAll(): Promise<void> {
    const pending = [...this.#connections.keys()].map((id) => this.close(id));
    await settleCleanup([...new Set([...pending, ...this.#closing])], "Some browser tool connections could not close.");
  }
}
