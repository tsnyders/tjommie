import { once } from "node:events";
import { createConnection, createServer, type AddressInfo } from "node:net";
import { expect, it } from "vitest";
import { WebSocketServer } from "ws";
import { FrameDecoder, lengthPrefixed } from "../../client/src/encrypted-channel.ts";
import { tcpWire, webSocketWire } from "./peer-wire.ts";

it("turns split TCP packets into frames and prefixes each written frame", async () => {
  const received: Uint8Array[] = [];
  const server = createServer((socket) => {
    const wire = tcpWire(socket);
    wire.onFrame((frame) => { received.push(frame); wire.write(frame); });
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const port = (server.address() as AddressInfo).port;
  const client = createConnection({ host: "127.0.0.1", port });
  try {
    await once(client, "connect");
    const decoder = new FrameDecoder(), echoed: Uint8Array[] = [];
    const both = new Promise<void>((resolve) => client.on("data", (chunk) => {
      for (const frame of decoder.feed(chunk)) echoed.push(frame);
      if (echoed.length === 2) resolve();
    }));
    const packet = Buffer.concat([lengthPrefixed(new Uint8Array([1, 2])), lengthPrefixed(new Uint8Array([3]))]);
    client.write(packet.subarray(0, 2));
    client.write(packet.subarray(2));
    await both;
    expect(received).toEqual([new Uint8Array([1, 2]), new Uint8Array([3])]);
    expect(echoed).toEqual(received);
  } finally {
    client.destroy();
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
});

it("propagates close when a malformed frame destroys the socket", async () => {
  let closed = 0;
  const server = createServer((socket) => tcpWire(socket).onClose(() => { closed++; }));
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const client = createConnection({ host: "127.0.0.1", port: (server.address() as AddressInfo).port });
  try {
    await once(client, "connect");
    const disconnected = once(client, "close");
    client.write(Buffer.from([0, 0, 0, 0]));
    await disconnected;
    expect(closed).toBe(1);
  } finally {
    client.destroy();
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
});

it("does not deliver later frames from a chunk after destruction", async () => {
  const received: number[] = [];
  const server = createServer((socket) => {
    const wire = tcpWire(socket);
    wire.onFrame((frame) => { received.push(frame[0]); wire.destroy(); });
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const client = createConnection({ host: "127.0.0.1", port: (server.address() as AddressInfo).port });
  try {
    await once(client, "connect");
    const disconnected = once(client, "close");
    client.write(Buffer.concat([lengthPrefixed(new Uint8Array([1])), lengthPrefixed(new Uint8Array([2]))]));
    await disconnected;
    expect(received).toEqual([1]);
  } finally {
    client.destroy();
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
});

it("maps one binary WebSocket message to one frame and rejects text", async () => {
  const server = new WebSocketServer({ port: 0 });
  await once(server, "listening");
  const port = (server.address() as AddressInfo).port;
  const accepted = new Promise<import("ws").WebSocket>((resolve) => server.once("connection", resolve));
  const socket = new WebSocket(`ws://127.0.0.1:${port}`);
  try {
    await new Promise<void>((resolve, reject) => { socket.addEventListener("open", () => resolve(), { once: true }); socket.addEventListener("error", () => reject(new Error("open failed")), { once: true }); });
    const peer = await accepted;
    const wire = webSocketWire(socket);
    const frames: Uint8Array[] = [];
    const received = new Promise<void>((resolve) => wire.onFrame((frame) => { frames.push(frame); resolve(); }));
    const sent = once(peer, "message");
    wire.write(new Uint8Array([1, 2, 3]));
    const [data] = await sent;
    expect(new Uint8Array(data as Buffer)).toEqual(new Uint8Array([1, 2, 3]));
    peer.send(new Uint8Array([4, 5]));
    await received;
    expect(frames).toEqual([new Uint8Array([4, 5])]);
    const closed = new Promise<void>((resolve) => wire.onClose(resolve));
    peer.send("text frame");
    await closed;
  } finally {
    socket.close();
    for (const peer of server.clients) peer.terminate();
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
});

it.each([0, 65_536])("closes a WebSocket wire on a %i-byte binary frame", async (size) => {
  const server = new WebSocketServer({ port: 0 });
  await once(server, "listening");
  const accepted = new Promise<import("ws").WebSocket>((resolve) => server.once("connection", resolve));
  const socket = new WebSocket(`ws://127.0.0.1:${(server.address() as AddressInfo).port}`);
  try {
    await new Promise<void>((resolve) => socket.addEventListener("open", () => resolve(), { once: true }));
    const peer = await accepted;
    const wire = webSocketWire(socket);
    const frames: Uint8Array[] = [];
    wire.onFrame((frame) => frames.push(frame));
    const closed = new Promise<void>((resolve) => wire.onClose(resolve));
    peer.send(new Uint8Array(size));
    await closed;
    expect(frames).toEqual([]);
  } finally {
    socket.close();
    for (const peer of server.clients) peer.terminate();
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
});

it.each(["oversized", "closing"] as const)("rejects a %s WebSocket wire write", async (caseName) => {
  const server = new WebSocketServer({ port: 0 });
  await once(server, "listening");
  const accepted = new Promise<import("ws").WebSocket>((resolve) => server.once("connection", resolve));
  const socket = new WebSocket(`ws://127.0.0.1:${(server.address() as AddressInfo).port}`);
  try {
    await new Promise<void>((resolve) => socket.addEventListener("open", () => resolve(), { once: true }));
    await accepted;
    const wire = webSocketWire(socket);
    const closed = new Promise<void>((resolve) => wire.onClose(resolve));
    if (caseName === "closing") socket.close();
    expect(() => wire.write(new Uint8Array(caseName === "oversized" ? 65_536 : 1))).toThrow("Connection closed.");
    await closed;
  } finally {
    socket.close();
    for (const peer of server.clients) peer.terminate();
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
});
