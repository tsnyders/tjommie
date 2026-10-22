import { afterEach, expect, it, vi } from "vitest";
import { startNativeBrowserHost } from "./native-browser-host.ts";
import { NativeBrowserBroker } from "../../daemon/src/native-browser-broker.ts";
import { startServer, type Server } from "../../daemon/src/server.ts";
import type { NativeBrowserCommand } from "../../protocol/src/native-browser.ts";

let host: ReturnType<typeof startNativeBrowserHost> | undefined;
let broker: NativeBrowserBroker<NativeBrowserCommand>;
let server: Server | undefined;
afterEach(async () => { await host?.close(); broker?.close(); await server?.close(); });
it("bridges a private native command and result without sending jobs through renderer events", async () => {
  const token = "h".repeat(43);
  broker = new NativeBrowserBroker(token, 1000);
  server = await startServer({ port: 0, token: "renderer-token", handlers: {}, allowedOrigins: [], routes: { "/browser-host": broker.handle } });
  const execute = vi.fn(async (_bot: { id: string; name: string; homeFolder: string }, command: NativeBrowserCommand) => ({ kind: command.kind, title: "Synthetic native page" }));
  host = startNativeBrowserHost({ port: server.port, token, execute });
  await expect(broker.request({ id: "one", name: "One", homeFolder: "C:/synthetic/one" }, { kind: "open" })).resolves.toEqual({ kind: "open", title: "Synthetic native page" });
  expect(execute).toHaveBeenCalledOnce();
});
it("returns a bounded tool error and can execute the next job", async () => {
  const token = "h".repeat(43);
  broker = new NativeBrowserBroker(token, 1000);
  server = await startServer({ port: 0, token: "renderer-token", handlers: {}, allowedOrigins: [], routes: { "/browser-host": broker.handle } });
  let first = true;
  host = startNativeBrowserHost({ port: server.port, token, execute: async () => { if (first) { first = false; throw new Error("Synthetic failure"); } return { ok: true }; } });
  const bot = { id: "one", name: "One", homeFolder: "C:/synthetic/one" };
  await expect(broker.request(bot, { kind: "state" })).rejects.toThrow("Synthetic failure");
  await expect(broker.request(bot, { kind: "state" })).resolves.toEqual({ ok: true });
});
it("does not let one bot's pending operation stall a different bot", async () => {
  const token = "h".repeat(43);
  broker = new NativeBrowserBroker(token, 1000);
  server = await startServer({ port: 0, token: "renderer-token", handlers: {}, allowedOrigins: [], routes: { "/browser-host": broker.handle } });
  let release: () => void = () => {};
  const gate = new Promise<void>((done) => { release = done; });
  host = startNativeBrowserHost({ port: server.port, token, execute: async (bot) => { if (bot.id === "one") await gate; return bot.id; } });
  const one = broker.request({ id: "one", name: "One", homeFolder: "C:/synthetic/one" }, { kind: "state" });
  try {
    await expect(broker.request({ id: "two", name: "Two", homeFolder: "C:/synthetic/two" }, { kind: "state" })).resolves.toBe("two");
  } finally { release(); }
  await expect(one).resolves.toBe("one");
});
it("keeps commands for the same bot in dispatch order", async () => {
  const token = "h".repeat(43);
  broker = new NativeBrowserBroker(token, 1000);
  server = await startServer({ port: 0, token: "renderer-token", handlers: {}, allowedOrigins: [], routes: { "/browser-host": broker.handle } });
  let release: () => void = () => {};
  const gate = new Promise<void>((done) => { release = done; });
  const order: string[] = [];
  host = startNativeBrowserHost({ port: server.port, token, execute: async (_bot, command) => { order.push(command.kind); if (command.kind === "open") await gate; return command.kind; } });
  const bot = { id: "one", name: "One", homeFolder: "C:/synthetic/one" };
  const first = broker.request(bot, { kind: "open" });
  const second = broker.request(bot, { kind: "state" });
  try { await new Promise((done) => setTimeout(done, 30)); expect(order).toEqual(["open"]); }
  finally { release(); }
  await expect(first).resolves.toBe("open");
  await expect(second).resolves.toBe("state");
  expect(order).toEqual(["open", "state"]);
});
it("keeps cancellation responsive when all four normal workers are occupied", async () => {
  const token = "h".repeat(43);
  broker = new NativeBrowserBroker(token, 1000);
  server = await startServer({ port: 0, token: "renderer-token", handlers: {}, allowedOrigins: [], routes: { "/browser-host": broker.handle } });
  let release: () => void = () => {};
  const gate = new Promise<void>((done) => { release = done; });
  host = startNativeBrowserHost({ port: server.port, token, execute: async (_bot, command) => { if (command.kind === "open") await gate; return command.kind; } });
  const jobs = ["one", "two", "three", "four"].map((id) => broker.request({ id, name: id, homeFolder: `C:/synthetic/${id}` }, { kind: "open" }));
  try {
    await new Promise((done) => setTimeout(done, 30));
    await expect(broker.request({ id: "one", name: "one", homeFolder: "C:/synthetic/one" }, { kind: "cancel_tools" })).resolves.toBe("cancel_tools");
  } finally { release(); }
  await Promise.all(jobs);
});
