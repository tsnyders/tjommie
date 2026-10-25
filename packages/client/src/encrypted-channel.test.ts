import { expect, it } from "vitest";
import { EncryptedMessages, FrameDecoder, lengthPrefixed } from "./encrypted-channel.ts";

// A bounded test double isolates record framing. Actual authentication/tamper/replay tests use Snow.
function cipher() {
  let closed = false;
  return { encrypt: (bytes: Uint8Array) => { if (closed) throw new Error(); const frame = new Uint8Array(bytes.length + 16); frame.set(bytes, 16); return frame; }, decrypt: (bytes: Uint8Array) => { if (closed) throw new Error(); return bytes.slice(16); }, close: () => { closed = true; } };
}

it("round trips fragmented Unicode messages and arbitrary TCP packet boundaries", () => {
  const sender = new EncryptedMessages(cipher()), receiver = new EncryptedMessages(cipher());
  const text = "🙂".repeat(50_000);
  const frames = sender.encode(text);
  expect(frames.length).toBeGreaterThan(1);
  const decoder = new FrameDecoder();
  const messages: string[] = [];
  for (const frame of frames) {
    const packet = lengthPrefixed(frame);
    for (let at = 0; at < packet.length; at += 37) {
      for (const decoded of decoder.feed(packet.subarray(at, at + 37))) {
        const message = receiver.receive(decoded);
        if (message !== null) messages.push(message);
      }
    }
  }
  expect(messages).toEqual([text]);
});

it("rejects oversized lengths before allocation and overlapping message starts", () => {
  const decoder = new FrameDecoder();
  expect(() => decoder.feed(new Uint8Array([127, 255, 255, 255]))).toThrow();
  const sender = new EncryptedMessages(cipher()), receiver = new EncryptedMessages(cipher());
  const frames = sender.encode("x".repeat(70_000));
  expect(receiver.receive(frames[0])).toBeNull();
  expect(() => receiver.receive(frames[0])).toThrow();
  expect(() => receiver.receive(frames[1])).toThrow();
});

it("does not accept continuation records out of order", () => {
  const frames = new EncryptedMessages(cipher()).encode("x".repeat(70_000));
  expect(() => new EncryptedMessages(cipher()).receive(frames[1])).toThrow();
});
