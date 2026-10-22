import { createServer } from "node:http";
import { afterEach, expect, it, vi } from "vitest";
import type { StoredBot } from "./bots.ts";
import { BrowserManager } from "./browser-manager.ts";
import { BrowserMcp, BrowserTransport } from "./browser-mcp.ts";
import { McpTokens } from "./mcp.ts";

const cleanups: (() => Promise<void>)[] = [];
afterEach(async () => { for (const close of cleanups.splice(0).reverse()) await close(); });

it("uses private transport request ids and gives MCP only the bot home as its root", async () => {
  const transport = new BrowserTransport("C:\\bot-home");
  const received: Record<string, unknown>[] = [];
  transport.onmessage = (message) => { received.push(message); };
  const first = transport.request("tools/list");
  const second = transport.request("tools/list");
  expect(received.map((entry) => entry.id)).toEqual([1, 2]);
  await transport.send({ jsonrpc: "2.0", id: 2, result: "second" });
  await transport.send({ jsonrpc: "2.0", id: 1, result: "first" });
  expect(await Promise.all([first, second])).toEqual(["first", "second"]);
  await transport.send({ jsonrpc: "2.0", id: "roots", method: "roots/list" });
  expect(received.at(-1)).toMatchObject({ id: "roots", result: { roots: [{ uri: "file:///C:/bot-home", name: "Bot home" }] } });
  await transport.close();
  await expect(transport.request("tools/list")).rejects.toThrow(/closed/);
});

it("closes the browser only on a request timeout, never on an ordinary MCP disconnect", async () => {
  const timedOut = vi.fn();
  const transport = new BrowserTransport("C:\\bot-home", timedOut);
  const pending = transport.request("tools/list");
  const rejected = expect(pending).rejects.toThrow(/closed/);
  await transport.close();
  await rejected;
  expect(timedOut).not.toHaveBeenCalled();
  vi.useFakeTimers();
  const stalled = new BrowserTransport("C:\\bot-home", timedOut);
  try {
    const cancelled = vi.fn();
    stalled.onmessage = cancelled;
    const result = expect(stalled.request("tools/call")).rejects.toThrow(/timed out/);
    await vi.advanceTimersByTimeAsync(90_000);
    await result;
    expect(timedOut).toHaveBeenCalledOnce();
    expect(cancelled).toHaveBeenLastCalledWith(expect.objectContaining({ method: "notifications/cancelled" }));
  } finally { await stalled.close(); vi.useRealTimers(); }
});

it("authorizes the bot before creating MCP or a browser and rejects revocation/disable", async () => {
  const tokens = new McpTokens();
  const bot = { id: "one", homeFolder: "C:\\bot-home", browser: true } as StoredBot;
  const launch = vi.fn();
  const manager = new BrowserManager(launch);
  const mcp = new BrowserMcp({ tokens, getBot: () => bot, manager });
  const server = createServer((req, res) => { void mcp.handle(req, res); });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("Expected local server");
  cleanups.push(async () => { await mcp.closeAll(); await new Promise<void>((resolve) => server.close(() => resolve())); });
  const url = `http://127.0.0.1:${address.port}`;
  const body = JSON.stringify({ jsonrpc: "2.0", id: 1, method: "initialize" });
  expect((await fetch(url, { method: "POST", body })).status).toBe(401);
  const token = tokens.issue(bot.id);
  const headers = { Authorization: `Bearer ${token}` };
  expect((await fetch(url, { method: "POST", headers, body })).status).toBe(200);
  bot.browser = false;
  expect((await fetch(url, { method: "POST", headers, body })).status).toBe(403);
  bot.browser = true;
  tokens.revoke(bot.id);
  expect((await fetch(url, { method: "POST", headers, body })).status).toBe(401);
  expect(launch).not.toHaveBeenCalled();
});

it("can close a connection whose pending MCP initialization fails", async () => {
  const tokens = new McpTokens();
  const bot = { id: "failed-opening", homeFolder: "C:\\bot-home", browser: true } as StoredBot;
  const opening = Promise.withResolvers<never>();
  const started = Promise.withResolvers<void>();
  const mcp = new BrowserMcp({ tokens, getBot: () => bot, createConnection: () => { started.resolve(); return opening.promise; } });
  const server = createServer((req, res) => { void mcp.handle(req, res).catch(() => { res.writeHead(500).end(); }); });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("Expected local server");
  cleanups.push(async () => { await mcp.closeAll(); await new Promise<void>((resolve) => server.close(() => resolve())); });
  const response = fetch(`http://127.0.0.1:${address.port}`, {
    method: "POST", headers: { Authorization: `Bearer ${tokens.issue(bot.id)}` },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/list" }),
  });
  await started.promise;
  const closing = mcp.close(bot.id);
  opening.reject(new Error("MCP failed to initialize"));
  await expect(closing).resolves.toBeUndefined();
  expect((await response).status).toBe(500);
});
