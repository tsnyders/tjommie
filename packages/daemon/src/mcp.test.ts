import { request } from "node:http";
import { afterEach, describe, expect, it, vi } from "vitest";
import { z } from "zod";
import { McpTokens, defineTool, mcpHandler, type McpTool } from "./mcp.ts";
import { startServer, type Route, type Server } from "./server.ts";

let server: Server | undefined;
afterEach(async () => {
  await server?.close();
  server = undefined;
  vi.restoreAllMocks();
});

const echo = defineTool({
  name: "echo",
  description: "Repeats the text back.",
  input: z.object({ text: z.string().describe("What to repeat") }),
  run: (botId, { text }) => `${botId} says ${text}`,
});
const refuses = defineTool({
  name: "refuses",
  description: "Always refuses.",
  input: z.object({}),
  run: () => {
    throw new Error("Limit reached, ask the user.");
  },
});

async function setup(tools: McpTool[] = [echo, refuses], tokens = new McpTokens()) {
  server = await startServer({
    port: 0,
    token: "t",
    allowedOrigins: ["app://tjommie"],
    handlers: {},
    routes: { "/mcp": mcpHandler({ tokens, tools }) },
  });
  return { tokens, port: server.port };
}

type HttpReply = { status: number; body: unknown };

// Plain node:http lets these tests send arbitrary Host and Origin headers.
function openRequest(port: number, headers: Record<string, string> = {}, method = "POST", path = "/mcp") {
  const req = request({ host: "127.0.0.1", port, path, method, headers: { "content-type": "application/json", ...headers } });
  const response = new Promise<HttpReply>((resolve, reject) => {
    req.on("error", reject);
    req.on("response", (res) => {
      let text = "";
      res.setEncoding("utf8");
      res.on("error", reject);
      res.on("data", (chunk: string) => (text += chunk));
      res.on("end", () => {
        try {
          const body: unknown = text ? JSON.parse(text) : null;
          resolve({ status: res.statusCode ?? 0, body });
        } catch (error) {
          reject(error);
        }
      });
    });
  });
  return { req, response };
}

function post(port: number, body: unknown, headers: Record<string, string> = {}, method = "POST", path = "/mcp"): Promise<HttpReply> {
  const { req, response } = openRequest(port, headers, method, path);
  req.end(typeof body === "string" ? body : JSON.stringify(body));
  return response;
}

describe("the app's MCP endpoint", () => {
  it("rejects a batch over 20 calls before executing anything", async () => {
    const run = vi.fn(() => "ran");
    const { tokens, port } = await setup([defineTool({ name: "record", description: "Records a call.", input: z.object({}), run })]);
    const auth = { authorization: `Bearer ${tokens.issue("bot-1")}` };
    const calls = Array.from({ length: 21 }, (_, id) => ({ id, method: "tools/call", params: { name: "record", arguments: {} } }));
    expect(await post(port, calls, auth)).toMatchObject({ status: 200, body: { id: null, error: { code: -32600, message: "A batch can contain at most 20 calls." } } });
    expect(run).not.toHaveBeenCalled();
    const response = await post(port, calls.slice(0, 20), auth);
    expect(response.status).toBe(200);
    expect(response.body).toHaveLength(20);
    expect(run).toHaveBeenCalledTimes(20);
  });
  it("echoes the client's protocol version, lists tools and runs them as the token's bot", async () => {
    const { tokens, port } = await setup();
    const auth = { authorization: `Bearer ${tokens.issue("bot-1")}` };
    const init = await post(port, { jsonrpc: "2.0", id: 0, method: "initialize", params: { protocolVersion: "2025-11-25", capabilities: {}, clientInfo: { name: "claude-code" } } }, auth);
    expect(init).toEqual({
      status: 200,
      body: { jsonrpc: "2.0", id: 0, result: { protocolVersion: "2025-11-25", capabilities: { tools: {} }, serverInfo: { name: "tjommie", version: "0.1.0" } } },
    });
    const list = await post(port, { jsonrpc: "2.0", id: 1, method: "tools/list", params: {} }, auth);
    expect(list).toEqual({
      status: 200,
      body: {
        jsonrpc: "2.0", id: 1, result: { tools: [
          {
            name: "echo",
            description: "Repeats the text back.",
            inputSchema: { type: "object", properties: { text: { type: "string", description: "What to repeat" } }, required: ["text"] },
          },
          { name: "refuses", description: "Always refuses.", inputSchema: { type: "object", properties: {} } },
        ] },
      },
    });
    const call = await post(port, { jsonrpc: "2.0", id: 2, method: "tools/call", params: { name: "echo", arguments: { text: "hi ñ" } } }, auth);
    expect(call).toEqual({
      status: 200,
      body: { jsonrpc: "2.0", id: 2, result: { content: [{ type: "text", text: "bot-1 says hi ñ" }], isError: false } },
    });
  });

  it.each([
    { params: { protocolVersion: "2025-06-18" }, expected: "2025-06-18" },
    { params: { protocolVersion: "future-client-version" }, expected: "future-client-version" },
    { params: {}, expected: "2025-06-18" },
  ])("initializes without jsonrpc and preserves protocol behavior for $params", async ({ params, expected }) => {
    const { tokens, port } = await setup();
    const auth = { authorization: `Bearer ${tokens.issue("bot-1")}` };
    expect(await post(port, { id: "init", method: "initialize", params }, auth)).toMatchObject({
      status: 200, body: { id: "init", result: { protocolVersion: expected } },
    });
  });

  it("turns bad arguments and refusals into failed calls the bot can read", async () => {
    const { tokens, port } = await setup();
    const auth = { authorization: `Bearer ${tokens.issue("bot-1")}` };
    const bad = await post(port, { jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "echo", arguments: {} } }, auth);
    expect(bad).toMatchObject({ status: 200, body: { result: { isError: true, content: [{ type: "text", text: expect.stringContaining("text") }] } } });
    const refused = await post(port, { jsonrpc: "2.0", id: 2, method: "tools/call", params: { name: "refuses", arguments: {} } }, auth);
    expect(refused).toEqual({
      status: 200,
      body: { jsonrpc: "2.0", id: 2, result: { content: [{ type: "text", text: "Limit reached, ask the user." }], isError: true } },
    });
    expect(await post(port, { id: 3, method: "tools/call", params: { name: "missing" } }, auth)).toMatchObject({
      status: 200, body: { error: { code: -32602, message: "Unknown tool missing" } },
    });
  });

  it("awaits tool results and reports rejected promises", async () => {
    const tool = defineTool({
      name: "async",
      description: "Asynchronous tool.",
      input: z.object({ refuse: z.boolean().default(false) }),
      run: async (botId, { refuse }) => {
        if (refuse) throw new Error("Not allowed.");
        return botId;
      },
    });
    const { tokens, port } = await setup([tool]);
    const auth = { authorization: `Bearer ${tokens.issue("bot-1")}` };
    expect(await post(port, { id: 1, method: "tools/call", params: { name: "async" } }, auth)).toMatchObject({
      body: { result: { content: [{ type: "text", text: "bot-1" }], isError: false } },
    });
    expect(await post(port, { id: 2, method: "tools/call", params: { name: "async", arguments: { refuse: true } } }, auth)).toMatchObject({
      body: { result: { content: [{ type: "text", text: "Not allowed." }], isError: true } },
    });
  });

  it("accepts notifications with 202, refuses unknown methods, and answers batches", async () => {
    const { tokens, port } = await setup();
    const auth = { authorization: `Bearer ${tokens.issue("bot-1")}` };
    expect(await post(port, { jsonrpc: "2.0", method: "notifications/initialized" }, auth)).toEqual({ status: 202, body: null });
    // Claude omits jsonrpc on this probe.
    expect(await post(port, { id: 3, method: "server/discover", params: {} }, auth)).toMatchObject({ body: { error: { code: -32601 } } });
    expect((await post(port, [{ jsonrpc: "2.0", id: 4, method: "ping" }, { jsonrpc: "2.0", method: "notifications/cancelled" }], auth)).body).toEqual([
      { jsonrpc: "2.0", id: 4, result: {} },
    ]);
    expect(await post(port, [{ method: "notifications/initialized" }, {}], auth)).toEqual({ status: 202, body: null });
    expect(await post(port, "{not json", auth)).toEqual({ status: 400, body: { jsonrpc: "2.0", id: null, error: { code: -32700, message: "Parse error" } } });
  });

  it.each([{ id: 7 }, { id: 7, method: 42 }])("rejects a malformed object with an id: %j", async (message) => {
    const { tokens, port } = await setup();
    const auth = { authorization: `Bearer ${tokens.issue("bot-1")}` };
    expect(await post(port, message, auth)).toEqual({
      status: 200, body: { jsonrpc: "2.0", id: 7, error: { code: -32600, message: "Invalid request" } },
    });
  });

  it("answers each invalid entry in a mixed batch without executing notifications", async () => {
    const run = vi.fn(() => "ran");
    const { tokens, port } = await setup([defineTool({ name: "record", description: "Records a call.", input: z.object({}), run })]);
    const auth = { authorization: `Bearer ${tokens.issue("bot-1")}` };
    const reply = await post(port, [
      { id: 1, method: "ping" },
      { id: 2 },
      null,
      42,
      [],
      { id: {}, method: "ping" },
      { method: "tools/call", params: { name: "record" } },
    ], auth);
    expect(reply).toEqual({ status: 200, body: [
      { jsonrpc: "2.0", id: 1, result: {} },
      { jsonrpc: "2.0", id: 2, error: { code: -32600, message: "Invalid request" } },
      ...Array.from({ length: 4 }, () => ({ jsonrpc: "2.0", id: null, error: { code: -32600, message: "Invalid request" } })),
    ] });
    expect(run).not.toHaveBeenCalled();
  });

  it("refuses browsers, other hosts, bad tokens, revoked tokens and huge bodies", async () => {
    const { tokens, port } = await setup();
    const token = tokens.issue("bot-1");
    const auth = { authorization: `Bearer ${token}` };
    const ping = { jsonrpc: "2.0", id: 1, method: "ping" };
    for (const origin of ["https://evil.example", "app://tjommie", ""]) {
      expect((await post(port, ping, { ...auth, origin })).status).toBe(403);
    }
    for (const host of [`evil.example:${port}`, "127.0.0.1:1", "localhost"]) {
      expect((await post(port, ping, { ...auth, host })).status).toBe(403);
    }
    expect((await post(port, ping, { ...auth, host: `localhost:${port}` })).status).toBe(200);
    expect((await post(port, ping)).status).toBe(401);
    expect((await post(port, ping, { authorization: "Bearer nope" })).status).toBe(401);
    expect((await post(port, ping, { authorization: `Basic ${token}` })).status).toBe(401);
    expect((await post(port, "", auth, "GET")).status).toBe(405);
    expect((await post(port, "x".repeat(1024 * 1024 + 1), auth)).status).toBe(413);
    tokens.revoke("bot-1");
    expect((await post(port, ping, auth)).status).toBe(401);
  });

  it("caps the body in bytes and accepts exactly 1 MB", async () => {
    const { tokens, port } = await setup();
    const auth = { authorization: `Bearer ${tokens.issue("bot-1")}` };
    const envelope = { id: 1, method: "ping", padding: "" };
    const paddingBytes = 1024 * 1024 - Buffer.byteLength(JSON.stringify(envelope));
    expect((await post(port, { ...envelope, padding: "x".repeat(paddingBytes) }, auth)).status).toBe(200);
    expect((await post(port, { ...envelope, padding: "x".repeat(paddingBytes - 1) + "ñ" }, auth)).status).toBe(413);
  });

  it("answers an oversized streaming upload before the client finishes the body", async () => {
    const { tokens, port } = await setup();
    const { req, response } = openRequest(port, { authorization: `Bearer ${tokens.issue("bot-1")}` });
    try {
      req.write("x".repeat(1024 * 1024 + 1));
      expect(await response).toEqual({ status: 413, body: null });
    } finally {
      req.destroy();
    }
  });

  it("gives each session a new token and forgets the old one", () => {
    const tokens = new McpTokens();
    const first = tokens.issue("bot-1");
    const other = tokens.issue("bot-2");
    const second = tokens.issue("bot-1");
    expect(second).not.toBe(first);
    expect(second).toMatch(/^[A-Za-z0-9_-]{32}$/);
    expect(tokens.botFor(first)).toBeNull();
    expect(tokens.botFor(second)).toBe("bot-1");
    tokens.revoke("bot-1");
    expect(tokens.botFor(second)).toBeNull();
    expect(tokens.botFor(other)).toBe("bot-2");
  });

  it("refuses a tool call when the token is revoked after authentication but before the body finishes", async () => {
    const run = vi.fn(() => "ran");
    const { tokens, port } = await setup([defineTool({ name: "record", description: "Records a call.", input: z.object({}), run })]);
    const token = tokens.issue("bot-1");
    const authenticated = Promise.withResolvers<void>();
    const botFor = tokens.botFor.bind(tokens);
    vi.spyOn(tokens, "botFor").mockImplementation((rawToken) => {
      const botId = botFor(rawToken);
      if (botId) authenticated.resolve();
      return botId;
    });
    const { req, response } = openRequest(port, { authorization: `Bearer ${token}` });
    try {
      req.write('{"id":1,"method":"tools/call",');
      await authenticated.promise;
      tokens.revoke("bot-1");
      req.end('"params":{"name":"record","arguments":{}}}');
      expect(await response).toEqual({
        status: 200, body: { jsonrpc: "2.0", id: 1, error: { code: -32001, message: "Session token is no longer valid" } },
      });
      expect(run).not.toHaveBeenCalled();
    } finally {
      req.destroy();
    }
  });

  it("rechecks the token before every tool in a batch", async () => {
    const tokens = new McpTokens();
    const run = vi.fn(() => "ran");
    const close = defineTool({
      name: "close",
      description: "Closes the session.",
      input: z.object({}),
      run: (botId) => {
        tokens.revoke(botId);
        return "closed";
      },
    });
    const { port } = await setup([close, defineTool({ name: "record", description: "Records a call.", input: z.object({}), run })], tokens);
    const auth = { authorization: `Bearer ${tokens.issue("bot-1")}` };
    expect(await post(port, [
      { id: 1, method: "tools/call", params: { name: "close" } },
      { id: 2, method: "tools/call", params: { name: "record" } },
    ], auth)).toEqual({ status: 200, body: [
      { jsonrpc: "2.0", id: 1, result: { content: [{ type: "text", text: "closed" }], isError: false } },
      { jsonrpc: "2.0", id: 2, error: { code: -32001, message: "Session token is no longer valid" } },
    ] });
    expect(run).not.toHaveBeenCalled();
  });

  it("requires the token to still identify the same bot before a tool runs", async () => {
    const run = vi.fn(() => "ran");
    const { tokens, port } = await setup([defineTool({ name: "record", description: "Records a call.", input: z.object({}), run })]);
    const token = tokens.issue("bot-1");
    vi.spyOn(tokens, "botFor").mockReturnValueOnce("bot-1").mockReturnValue("bot-2");
    expect(await post(port, { id: 1, method: "tools/call", params: { name: "record" } }, { authorization: `Bearer ${token}` })).toMatchObject({
      status: 200, body: { error: { code: -32001 } },
    });
    expect(run).not.toHaveBeenCalled();
  });

  it("routes query strings and keeps the health and missing-route responses", async () => {
    const { tokens, port } = await setup();
    const auth = { authorization: `Bearer ${tokens.issue("bot-1")}` };
    expect(await post(port, { id: 1, method: "ping" }, auth, "POST", "/mcp?client=codex")).toEqual({
      status: 200, body: { jsonrpc: "2.0", id: 1, result: {} },
    });
    expect(await post(port, "", {}, "GET", "/health")).toEqual({ status: 200, body: { ok: true, protocol: 1 } });
    expect(await post(port, "", {}, "GET", "/missing")).toEqual({ status: 404, body: null });
  });
});

describe("extra HTTP route errors", () => {
  it.each(["synchronous throw", "rejected promise"])("logs and answers a %s with 500", async (kind) => {
    const failure = new Error(`Route failure: ${kind}`);
    const logged = vi.spyOn(console, "error").mockImplementation(() => {});
    const route: Route = () => {
      if (kind === "synchronous throw") throw failure;
      return Promise.reject(failure);
    };
    server = await startServer({ port: 0, token: "t", allowedOrigins: [], handlers: {}, routes: { "/mcp": route } });
    expect(await post(server.port, {})).toEqual({ status: 500, body: null });
    expect(logged).toHaveBeenCalledExactlyOnceWith(failure);
    expect(await post(server.port, "", {}, "GET", "/health")).toEqual({ status: 200, body: { ok: true, protocol: 1 } });
  });
});
