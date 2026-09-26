import { ClientMessage, type ClientMessage as Request } from "@tjommie/protocol";
import { afterEach, describe, expect, it } from "vitest";
import { WebSocketServer, type WebSocket } from "ws";
import { ClientRpcError, connect, type Client } from "./client.ts";
import { RpcError, startServer, type Server } from "./server.ts";

const clients: Client[] = [];
const peers: WebSocketServer[] = [];
let server: Server | undefined;

afterEach(async () => {
  await Promise.all(clients.splice(0).map((client) => client.close()));
  for (const peer of peers.splice(0)) {
    for (const socket of peer.clients) socket.terminate();
    await new Promise<void>((resolve) => peer.close(() => resolve()));
  }
  await server?.close();
  server = undefined;
});

async function peer(onMessage: (socket: WebSocket, message: Request) => void): Promise<number> {
  const listener = new WebSocketServer({ port: 0, host: "127.0.0.1" });
  peers.push(listener);
  listener.on("connection", (socket) => socket.on("message", (raw) => onMessage(socket, ClientMessage.parse(JSON.parse(String(raw))))));
  await new Promise<void>((resolve) => listener.once("listening", resolve));
  const address = listener.address();
  if (typeof address === "string" || !address) throw new Error("Expected a TCP listener");
  return address.port;
}

async function open(port: number): Promise<Client> {
  const client = await connect(port, "test-token");
  clients.push(client);
  return client;
}

describe("typed daemon client", () => {
  it("sends the hello and typed calls, and subscribes to validated events", async () => {
    const requests: Request[] = [];
    const port = await peer((socket, message) => {
      requests.push(message);
      if (message.type === "hello") socket.send(JSON.stringify({ type: "welcome", protocol: 1 }));
      else {
        socket.send(JSON.stringify({ type: "event", event: "settings.changed", data: { chainLimit: 4 } }));
        socket.send(JSON.stringify({ type: "res", id: message.id, ok: true, result: { chainLimit: 4 } }));
      }
    });
    const client = await open(port);
    const changes: number[] = [];
    const unsubscribe = client.subscribe((event) => {
      if (event.event === "settings.changed") changes.push(event.data.chainLimit);
    });
    expect(await client.call("settings.set", { chainLimit: 4 })).toEqual({ chainLimit: 4 });
    expect(requests).toEqual([
      { type: "hello", protocol: 1, token: "test-token" },
      { type: "req", id: 1, method: "settings.set", params: { chainLimit: 4 } },
    ]);
    expect(changes).toEqual([4]);
    expect(client.events).toEqual([{ type: "event", event: "settings.changed", data: { chainLimit: 4 } }]);
    unsubscribe();
    await client.call("settings.get", {});
    expect(changes).toEqual([4]);
    expect(client.events).toHaveLength(2);
  });

  it("reports RPC errors and leaves the connection usable", async () => {
    server = await startServer({
      port: 0,
      token: "test-token",
      allowedOrigins: [],
      handlers: {
        "bots.delete": () => { throw new RpcError("not_found", "No such bot"); },
        "settings.get": () => ({ chainLimit: 3 }),
      },
    });
    const client = await open(server.port);
    const failure = client.call("bots.delete", { id: "missing" });
    await expect(failure).rejects.toThrow("not_found: No such bot");
    await expect(failure).rejects.toBeInstanceOf(ClientRpcError);
    await expect(failure).rejects.toMatchObject({ code: "not_found" });
    expect(await client.call("settings.get", {})).toEqual({ chainLimit: 3 });
  });

  it("rejects every pending call on disconnect, including the close code and reason", async () => {
    let calls = 0;
    const port = await peer((socket, message) => {
      if (message.type === "hello") socket.send(JSON.stringify({ type: "welcome", protocol: 1 }));
      else if (++calls === 2) socket.close(4401, "session ended");
    });
    const client = await open(port);
    await Promise.all([
      expect(client.call("bots.list", {})).rejects.toThrow(/4401.*session ended/),
      expect(client.call("settings.get", {})).rejects.toThrow(/4401.*session ended/),
    ]);
    await expect(client.call("bots.list", {})).rejects.toThrow(/4401.*session ended/);
    await expect(client.disconnected).resolves.toMatchObject({ code: 4401, reason: "session ended", opened: true });
  });

  it("rejects welcome when authentication closes the socket", async () => {
    server = await startServer({ port: 0, token: "different-token", allowedOrigins: [], handlers: {} });
    await expect(connect(server.port, "test-token")).rejects.toMatchObject({ code: 4401, reason: "unauthorized", opened: true });
  });

  it("rejects welcome on a connection error", async () => {
    const listener = new WebSocketServer({ port: 0, host: "127.0.0.1", verifyClient: () => false });
    peers.push(listener);
    await new Promise<void>((resolve) => listener.once("listening", resolve));
    const address = listener.address();
    if (typeof address === "string" || !address) throw new Error("Expected a TCP listener");
    await expect(connect(address.port, "test-token")).rejects.toMatchObject({ code: 1006, reason: "WebSocket error", opened: false });
  });

  it("rejects every pending call on a WebSocket error", async () => {
    let calls = 0;
    const port = await peer((socket, message) => {
      if (message.type === "hello") socket.send(JSON.stringify({ type: "welcome", protocol: 1 }));
      // Server frames must not be masked. This triggers the native client's error event.
      else if (++calls === 2) socket.send("invalid frame", { mask: true });
    });
    const client = await open(port);
    await Promise.all([
      expect(client.call("bots.list", {})).rejects.toThrow(/1006.*error/i),
      expect(client.call("settings.get", {})).rejects.toThrow(/1006.*error/i),
    ]);
    await expect(client.disconnected).resolves.toMatchObject({ code: 1006, reason: "WebSocket error", opened: true });
  });

  it("reports a normal remote close while idle", async () => {
    const port = await peer((socket, message) => {
      if (message.type === "hello") {
        socket.send(JSON.stringify({ type: "welcome", protocol: 1 }));
        socket.close(1000, "Daemon stopped");
      }
    });
    const client = await open(port);
    await expect(client.disconnected).resolves.toMatchObject({ code: 1000, reason: "Daemon stopped", opened: true });
  });

  it("rejects an incompatible welcome", async () => {
    const port = await peer((socket) => socket.send(JSON.stringify({ type: "welcome", protocol: 99 })));
    await expect(connect(port, "test-token")).rejects.toThrow(/protocol/i);
  });

  it.each([
    "{broken json",
    JSON.stringify({ type: "res", id: "1", ok: true, result: [] }),
    JSON.stringify({ type: "res", id: 1, ok: "true", result: [] }),
    JSON.stringify({ type: "res", id: 1, ok: true }),
    JSON.stringify({ type: "res", id: 1, ok: false, error: { message: "missing code" } }),
    JSON.stringify({ type: "res", id: 1, ok: false, error: { code: "unexpected", message: "Unknown code" } }),
    JSON.stringify({ type: "event", event: "settings.changed", data: { chainLimit: "invalid" } }),
    JSON.stringify({ type: "event", event: "unknown", data: {} }),
    JSON.stringify({ type: "mystery" }),
  ])("rejects pending calls for an invalid incoming frame: %s", async (frame) => {
    const port = await peer((socket, message) => {
      if (message.type === "hello") socket.send(JSON.stringify({ type: "welcome", protocol: 1 }));
      else socket.send(frame);
    });
    const client = await open(port);
    await expect(client.call("settings.get", {})).rejects.toThrow(/invalid.*frame/i);
  });

  it("validates successful results against the requested method schema", async () => {
    const port = await peer((socket, message) => {
      if (message.type === "hello") socket.send(JSON.stringify({ type: "welcome", protocol: 1 }));
      else socket.send(JSON.stringify({ type: "res", id: message.id, ok: true, result: { chainLimit: "invalid" } }));
    });
    const client = await open(port);
    await expect(client.call("settings.get", {})).rejects.toThrow(/invalid.*frame/i);
  });

  it("settles pending calls when the caller closes the client", async () => {
    const port = await peer((socket, message) => {
      if (message.type === "hello") socket.send(JSON.stringify({ type: "welcome", protocol: 1 }));
    });
    const client = await open(port);
    const pending = expect(client.call("settings.get", {})).rejects.toThrow(/1000.*client closed/i);
    await client.close();
    await pending;
    await client.close();
  });
});
