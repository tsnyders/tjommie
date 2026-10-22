import { afterEach, describe, expect, it, vi } from "vitest";
import type { StoredBot } from "./bots.ts";
import { McpTokens } from "./mcp.ts";
import { NativeBrowserMcp } from "./native-browser-mcp.ts";
import { startServer, type Server } from "./server.ts";
import type { NativeBrowserCommand } from "../../protocol/src/native-browser.ts";

const servers: Server[] = [];
afterEach(async () => { await Promise.all(servers.splice(0).map((server) => server.close())); });
const ok = { content: [{ type: "text", text: "native result" }] };
type Requester = (botId: string, command: NativeBrowserCommand, authorized?: () => boolean) => Promise<unknown>;
async function fixture(run: Requester = async () => ok) {
  const bot = { id: "bot-one", homeFolder: "C:/synthetic/one", browser: true } as StoredBot;
  const tokens = new McpTokens(), token = tokens.issue(bot.id), request = vi.fn(run), cancelTools = vi.fn();
  const mcp = new NativeBrowserMcp({ tokens, getBot: () => bot, request, cancelTools });
  const server = await startServer({ port: 0, token: "renderer", handlers: {}, allowedOrigins: [], routes: { "/mcp/browser": mcp.handle } }); servers.push(server);
  const url = `http://127.0.0.1:${server.port}/mcp/browser`;
  const post = async (body: unknown, bearer = token, method = "POST") => {
    const response = await fetch(url, { method, headers: { Authorization: `Bearer ${bearer}` }, ...(method === "GET" ? {} : { body: typeof body === "string" ? body : JSON.stringify(body) }) });
    const value = await response.text(); return { status: response.status, body: value ? JSON.parse(value) as unknown : null };
  };
  const call = (name: string, args: unknown = {}, id = 1) => post({ id, method: "tools/call", params: { name, arguments: args } });
  return { bot, tokens, token, request, cancelTools, mcp, post, call };
}
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>((done) => { resolve = done; }); return { promise, resolve }; }

describe("native browser MCP proxy", () => {
  it("initializes/lists without opening a browser and only forwards validated tool commands", async () => {
    const f = await fixture();
    expect(await f.post({ id: 1, method: "initialize", params: { protocolVersion: "future" } })).toMatchObject({ status: 200, body: { result: { protocolVersion: "future" } } });
    const listed = await f.post({ id: 2, method: "tools/list" }); expect(JSON.stringify(listed)).toContain("browser_take_screenshot"); expect(JSON.stringify(listed)).not.toContain("browser_run_code"); expect(f.request).not.toHaveBeenCalled();
    expect(await f.call("browser_snapshot")).toMatchObject({ body: { result: ok } });
    expect(f.request).toHaveBeenCalledWith(f.bot.id, { kind: "tool", name: "browser_snapshot", args: {} }, expect.any(Function));
    expect(f.request.mock.calls[0][2]?.()).toBe(true);
  });
  it("rejects renderer/bad tokens, disabled bots and non-POST requests", async () => {
    const f = await fixture(); expect((await f.post({}, "renderer")).status).toBe(401); expect((await f.post({}, f.token, "GET")).status).toBe(405);
    f.bot.browser = false; expect((await f.post({})).status).toBe(403); expect(f.request).not.toHaveBeenCalled();
  });
  it("rejects oversized/bad input before dispatch and handles notifications", async () => {
    const f = await fixture();
    expect((await f.post("x".repeat(1024 * 1024 + 1))).status).toBe(413);
    expect((await f.post("not-json")).status).toBe(400);
    expect((await f.post(Array.from({ length: 21 }, (_, id) => ({ id, method: "ping" })))).status).toBe(400);
    expect(await f.post({ method: "notifications/initialized" })).toEqual({ status: 202, body: null });
    expect(await f.call("browser_run_code", { code: "process.env" })).toMatchObject({ body: { error: { code: -32602 } } });
    expect(await f.call("browser_click", { target: "#save", injected: "bad" })).toMatchObject({ body: { result: { isError: true } } });
    expect(f.request).not.toHaveBeenCalled();
  });
  it("serializes ordinary calls, checks revoked tokens when queued work starts, and drops late output", async () => {
    const pending = deferred<unknown>(), f = await fixture(async () => pending.promise);
    const first = f.call("browser_snapshot"); await vi.waitFor(() => expect(f.request).toHaveBeenCalledTimes(1));
    const second = f.call("browser_snapshot", {}, 2); await new Promise<void>((done) => setTimeout(done, 20));
    f.tokens.revoke(f.bot.id); expect(f.request.mock.calls[0][2]?.()).toBe(false); pending.resolve(ok);
    expect(await first).toMatchObject({ body: { error: { code: -32001 } } }); expect(await second).toMatchObject({ body: { error: { code: -32001 } } }); expect(f.request).toHaveBeenCalledTimes(1);
  });
  it("close invalidates queued/in-flight calls and cancels tools without closing the native session", async () => {
    const pending = deferred<unknown>(), f = await fixture(async () => pending.promise);
    const first = f.call("browser_snapshot"); await vi.waitFor(() => expect(f.request).toHaveBeenCalledTimes(1));
    const second = f.call("browser_click", { target: "#save" }, 2); await new Promise<void>((done) => setTimeout(done, 20));
    await f.mcp.close(f.bot.id); expect(f.cancelTools).toHaveBeenCalledWith(f.bot.id); pending.resolve(ok);
    expect(await first).toMatchObject({ body: { error: { code: -32001 } } }); expect(await second).toMatchObject({ body: { error: { code: -32001 } } });
    expect(f.request).toHaveBeenCalledTimes(1); expect(f.request.mock.calls.some(([, command]) => command.kind === "close")).toBe(false);
    expect(await f.call("browser_snapshot")).toMatchObject({ body: { result: ok } });
  });
  it("lets dialog dismissal bypass a pending tool queue", async () => {
    const pending = deferred<unknown>(), f = await fixture(async (_bot, command) => command.kind === "tool" && command.name === "browser_handle_dialog" ? ok : pending.promise);
    const first = f.call("browser_evaluate", { function: "() => alert('local')" }); await vi.waitFor(() => expect(f.request).toHaveBeenCalledTimes(1));
    expect(await f.call("browser_handle_dialog", { accept: false }, 2)).toMatchObject({ body: { result: ok } });
    expect(f.request).toHaveBeenCalledTimes(2); pending.resolve(ok); await first;
  });
  it("bounds the combined reply when a batch requests several large screenshots", async () => {
    const image = { content: [{ type: "image", mimeType: "image/png", data: Buffer.alloc(2 * 1024 * 1024).toString("base64") }] };
    const f = await fixture(async () => image);
    const response = await f.post(Array.from({ length: 4 }, (_, id) => ({ id, method: "tools/call", params: { name: "browser_take_screenshot", arguments: {} } })));
    expect(Buffer.byteLength(JSON.stringify(response.body))).toBeLessThan(8 * 1024 * 1024);
    expect(JSON.stringify(response.body)).toContain("Request large results individually");
  });
  it.each([{ raw: { arbitrary: "CDP object" } }, { raw: { content: [{ type: "text", text: "x".repeat(128 * 1024 + 101) }] } }, { raw: { content: [{ type: "image", mimeType: "image/svg+xml", data: "abc" }] } }])("bounds and validates bridge results", async ({ raw }) => {
    const f = await fixture(async () => raw); expect(await f.call("browser_snapshot")).toMatchObject({ body: { result: { isError: true, content: [{ type: "text", text: expect.stringContaining("invalid or oversized") }] } } });
  });
});
