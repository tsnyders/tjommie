import { afterEach, describe, expect, it } from "vitest";
import { NativeBrowserBroker } from "./native-browser-broker.ts";
import { startServer, type Server } from "./server.ts";

const secret = "h".repeat(43);
const bot = { id: "bot-one", name: "One", homeFolder: "C:/synthetic/bot-one" };
let server: Server | undefined;
let broker: NativeBrowserBroker<{ kind: string }>;
afterEach(async () => { broker?.close(); await server?.close(); });
async function fixture(timeout = 1000): Promise<string> {
  broker = new NativeBrowserBroker(secret, timeout);
  server = await startServer({ port: 0, token: "renderer-token", handlers: {}, allowedOrigins: [], routes: { "/browser-host": broker.handle } });
  return `http://127.0.0.1:${server.port}/browser-host`;
}
const auth = { Authorization: `Bearer ${secret}` };
describe("private native browser host broker", () => {
  it("does not accept the renderer's daemon token or browser Origin", async () => {
    const url = await fixture();
    expect((await fetch(url, { headers: { Authorization: "Bearer renderer-token" } })).status).toBe(401);
    expect((await fetch(url, { headers: { ...auth, Origin: "app://tjommie" } })).status).toBe(403);
  });
  it("delivers only to the private host and settles each command once", async () => {
    const url = await fixture();
    const result = broker.request(bot, { kind: "state" });
    const job = await (await fetch(url, { headers: auth })).json() as { id: string; bot: typeof bot; command: { kind: string } };
    expect(job.bot).toEqual(bot);
    expect(job.command).toEqual({ kind: "state" });
    const reply = { method: "POST", headers: auth, body: JSON.stringify({ id: job.id, ok: true, result: { title: "Synthetic page" } }) };
    expect((await fetch(url, reply)).status).toBe(204);
    await expect(result).resolves.toEqual({ title: "Synthetic page" });
    expect((await fetch(url, reply)).status).toBe(404);
  });
  it("does not accept unsolicited or malformed replies", async () => {
    const url = await fixture();
    expect((await fetch(url, { method: "POST", headers: auth, body: "{}" })).status).toBe(400);
    expect((await fetch(url, { method: "POST", headers: auth, body: JSON.stringify({ id: "unknown", ok: true }) })).status).toBe(404);
  });
  it("bounds unavailable-host waits and cancels only the closed bot's jobs", async () => {
    await fixture(25);
    const timeout = expect(broker.request(bot, { kind: "state" })).rejects.toThrow("not started");
    await timeout;
    const closed = expect(broker.request(bot, { kind: "state" })).rejects.toThrow("closed");
    broker.closeBot(bot.id);
    await closed;
  });
});
