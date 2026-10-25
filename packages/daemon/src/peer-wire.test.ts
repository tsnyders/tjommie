import { once } from "node:events";
import { createConnection, createServer, type AddressInfo } from "node:net";
import { expect, it } from "vitest";
import { FrameDecoder, lengthPrefixed } from "../../client/src/encrypted-channel.ts";
import { tcpWire } from "./peer-wire.ts";

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
