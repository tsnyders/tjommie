import { expect, it } from "vitest";
import { connectTransport, type RpcTransport } from "./client.ts";
import { PROTOCOL_VERSION } from "@tjommie/protocol";

function channel() {
  let message: ((text: string) => void) | undefined;
  let closed: ((event: { code: number; reason: string }) => void) | undefined;
  const sent: unknown[] = [];
  const transport: RpcTransport = {
    send: (text) => {
      const frame: unknown = JSON.parse(text);
      sent.push(frame);
      if (typeof frame === "object" && frame !== null && "type" in frame && frame.type === "hello") {
        queueMicrotask(() => message?.(JSON.stringify({ type: "welcome", protocol: PROTOCOL_VERSION })));
      }
    },
    close: () => closed?.({ code: 1000, reason: "Closed" }),
    onMessage: (listener) => { message = listener; return () => { message = undefined; }; },
    onClose: (listener) => { closed = listener; return () => { closed = undefined; }; },
  };
  return { transport, sent, receive: (frame: unknown) => message?.(JSON.stringify(frame)), disconnect: () => closed?.({ code: 1006, reason: "Host offline" }) };
}

it("uses typed RPC over an authenticated channel without the local daemon token", async () => {
  const wire = channel();
  const client = await connectTransport(wire.transport, { recordEvents: false });
  expect(wire.sent).toEqual([{ type: "hello", protocol: PROTOCOL_VERSION, token: "device" }]);
  const request = client.call("bots.list", {});
  wire.receive({ type: "res", id: 1, ok: true, result: [] });
  expect(await request).toEqual([]);
  await client.close();
});

it("rejects pending calls on disconnect and never replays the mutation", async () => {
  const wire = channel();
  const client = await connectTransport(wire.transport);
  const pending = client.call("chat.stop", { botId: "synthetic" });
  const rejected = expect(pending).rejects.toThrow("Host offline");
  wire.disconnect();
  await rejected;
  expect(wire.sent).toHaveLength(2);
  expect((await client.disconnected).reason).toBe("Host offline");
});

it("validates RPC results and events on remote channels too", async () => {
  const wire = channel();
  const client = await connectTransport(wire.transport);
  const pending = client.call("bots.list", {});
  const rejected = expect(pending).rejects.toThrow();
  wire.receive({ type: "res", id: 1, ok: true, result: [{ id: "invalid" }] });
  await rejected;
  await client.close();
});
