import type { ServerMessage } from "@tjommie/protocol";
import { afterEach, describe, expect, it } from "vitest";
import WebSocket from "ws";
import { RpcError, startServer, type Server } from "./server.ts";

let server: Server | undefined;
afterEach(async () => {
  await server?.close();
  server = undefined;
});

function connect(port: number, headers: Record<string, string> = {}) {
  const ws = new WebSocket(`ws://127.0.0.1:${port}/`, { headers });
  const inbox: ServerMessage[] = [];
  const waiting: ((message: ServerMessage) => void)[] = [];
  ws.on("message", (raw) => {
    const message = JSON.parse(String(raw)) as ServerMessage;
    const waiter = waiting.shift();
    if (waiter) waiter(message);
    else inbox.push(message);
  });
  return {
    ws,
    opened: new Promise<void>((resolve, reject) => {
      ws.on("open", () => resolve());
      ws.on("error", reject);
    }),
    closed: new Promise<number>((resolve) => ws.on("close", (code) => resolve(code))),
    next: () => new Promise<ServerMessage>((resolve) => (inbox.length ? resolve(inbox.shift()!) : waiting.push(resolve))),
    send: (message: unknown) => ws.send(JSON.stringify(message)),
  };
}

async function hello(client: ReturnType<typeof connect>, token: string) {
  await client.opened;
  client.send({ type: "hello", protocol: 1, token });
  return client.next();
}

describe("startServer", () => {
  it("answers requests after a valid hello", async () => {
    server = await startServer({ port: 0, token: "secret", allowedOrigins: [], handlers: { "bots.list": () => [] } });
    const client = connect(server.port);
    expect(await hello(client, "secret")).toEqual({ type: "welcome", protocol: 1 });
    client.send({ type: "req", id: 7, method: "bots.list", params: {} });
    expect(await client.next()).toEqual({ type: "res", id: 7, ok: true, result: [] });
    client.ws.close();
  });

  it("closes with 4401 on a wrong or missing token and 4003 on a wrong protocol", async () => {
    server = await startServer({ port: 0, token: "secret", allowedOrigins: [], handlers: {} });
    const wrongToken = connect(server.port);
    await wrongToken.opened;
    wrongToken.send({ type: "hello", protocol: 1, token: "nope" });
    expect(await wrongToken.closed).toBe(4401);
    const missingToken = connect(server.port);
    await missingToken.opened;
    missingToken.send({ type: "hello", protocol: 1 });
    expect(await missingToken.closed).toBe(4401);
    const wrongProtocol = connect(server.port);
    await wrongProtocol.opened;
    wrongProtocol.send({ type: "hello", protocol: 99, token: "secret" });
    expect(await wrongProtocol.closed).toBe(4003);
  });

  it("closes with 4400 on malformed JSON", async () => {
    server = await startServer({ port: 0, token: "secret", allowedOrigins: [], handlers: {} });
    const client = connect(server.port);
    await client.opened;
    client.ws.send("{not json");
    expect(await client.closed).toBe(4400);
  });

  it("survives a bad WebSocket frame", async () => {
    server = await startServer({ port: 0, token: "secret", allowedOrigins: [], handlers: {} });
    const badFrame = connect(server.port);
    await badFrame.opened;
    badFrame.ws.send("x", { mask: false });
    expect(await badFrame.closed).toBe(1002);

    const fresh = connect(server.port);
    expect(await hello(fresh, "secret")).toEqual({ type: "welcome", protocol: 1 });
    fresh.ws.close();
  });

  it("reports bad params and handler errors without closing the connection", async () => {
    server = await startServer({
      port: 0,
      token: "t",
      allowedOrigins: [],
      handlers: {
        "bots.delete": () => {
          throw new RpcError("not_found", "No such bot");
        },
      },
    });
    const client = connect(server.port);
    await hello(client, "t");
    client.send({ type: "req", id: 1, method: "bots.delete", params: {} });
    expect(await client.next()).toMatchObject({ id: 1, ok: false, error: { code: "bad_params" } });
    client.send({ type: "req", id: 2, method: "bots.delete", params: { id: "x" } });
    expect(await client.next()).toEqual({ type: "res", id: 2, ok: false, error: { code: "not_found", message: "No such bot" } });
    client.send({ type: "req", id: 3, method: "bots.nope", params: {} });
    expect(await client.next()).toMatchObject({ id: 3, ok: false, error: { code: "unknown_method" } });
    client.ws.close();
  });

  it("refuses a foreign Origin and a foreign Host, and accepts the app origin", async () => {
    server = await startServer({ port: 0, token: "t", allowedOrigins: ["app://tjommie"], handlers: {} });
    await expect(connect(server.port, { Origin: "https://evil.example" }).opened).rejects.toThrow(/403/);
    await expect(connect(server.port, { Host: `evil.example:${server.port}` }).opened).rejects.toThrow(/403/);
    const app = connect(server.port, { Origin: "app://tjommie" });
    await app.opened;
    app.ws.close();
  });

  it("broadcasts events to authenticated clients", async () => {
    server = await startServer({ port: 0, token: "t", allowedOrigins: [], handlers: {} });
    const client = connect(server.port);
    await hello(client, "t");
    server.broadcast("bot.deleted", { id: "b1" });
    expect(await client.next()).toEqual({ type: "event", event: "bot.deleted", data: { id: "b1" } });
    client.ws.close();
  });
});
