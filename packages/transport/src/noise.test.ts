import { readFileSync } from "node:fs";
import { afterEach, describe, expect, it } from "vitest";
import { generateKeypair, initNoise, initiator, responder, MAX_NOISE_FRAME, MAX_NOISE_PLAINTEXT, type NoiseSession } from "./index.ts";

initNoise(readFileSync(new URL("../wasm/tjommie_noise_bg.wasm", import.meta.url)));
const encoder = new TextEncoder(), decoder = new TextDecoder();
const live = new Set<NoiseSession>();
afterEach(() => { for (const session of live) session.close(); live.clear(); });
const track = (session: NoiseSession): NoiseSession => { live.add(session); return session; };
function peers(psk?: Uint8Array, prologue = encoder.encode("synthetic-context")) {
  const device = generateKeypair(), host = generateKeypair();
  const client = track(initiator({ privateKey: device.privateKey, remotePublicKey: host.publicKey, psk, prologue }));
  const server = track(responder({ privateKey: host.privateKey, psk, prologue }));
  return { client, server, device, host };
}
function handshake(pair: ReturnType<typeof peers>): void {
  expect(decoder.decode(pair.server.readHandshake(pair.client.writeHandshake(encoder.encode("hello"))))).toBe("hello");
  expect(decoder.decode(pair.client.readHandshake(pair.server.writeHandshake(encoder.encode("reply"))))).toBe("reply");
}

describe("compiled Snow WASM transport", () => {
  it.each([false, true])("performs the actual IK handshake and authenticated bidirectional encryption (pairing=%s)", (pairing) => {
    const pair = peers(pairing ? crypto.getRandomValues(new Uint8Array(32)) : undefined);
    expect(pair.client.handshakeFinished).toBe(false); expect(pair.server.remotePublicKey).toBeNull();
    handshake(pair);
    expect(pair.client.handshakeFinished).toBe(true); expect(pair.server.handshakeFinished).toBe(true);
    expect(pair.client.remotePublicKey).toEqual(pair.host.publicKey); expect(pair.server.remotePublicKey).toEqual(pair.device.publicKey);
    const text = encoder.encode("Synthetic RPC data"); const encrypted = pair.client.encrypt(text);
    expect(encrypted).not.toEqual(text); expect(pair.server.decrypt(encrypted)).toEqual(text);
    expect(decoder.decode(pair.client.decrypt(pair.server.encrypt(encoder.encode("Synthetic result"))))).toBe("Synthetic result");
  });
  it("generates independent 32-byte key pairs", () => {
    const a = generateKeypair(), b = generateKeypair();
    expect(a.privateKey).toHaveLength(32); expect(a.publicKey).toHaveLength(32);
    expect(a.privateKey).not.toEqual(a.publicKey); expect(a.privateKey).not.toEqual(b.privateKey); expect(a.publicKey).not.toEqual(b.publicKey);
  });
  it("IKpsk2 responder handshake completion alone does not prove the client's PSK", () => {
    const device = generateKeypair(), host = generateKeypair();
    const client = track(initiator({ privateKey: device.privateKey, remotePublicKey: host.publicKey, psk: new Uint8Array(32).fill(1) }));
    const server = track(responder({ privateKey: host.privateKey, psk: new Uint8Array(32).fill(2) }));
    // This succeeds without knowledge of the correct PSK. Application pairing MUST NOT finish here.
    server.readHandshake(client.writeHandshake(new Uint8Array()));
    const response = server.writeHandshake(new Uint8Array()); expect(server.handshakeFinished).toBe(true);
    expect(() => client.readHandshake(response)).toThrow("Secure connection failed.");
    expect(() => client.encrypt(encoder.encode("confirm"))).toThrow("Secure connection failed.");
  });
  it("only an encrypted confirmation authenticates the new pairing before application authorization", () => {
    const pair = peers(new Uint8Array(32).fill(3)); handshake(pair);
    const confirm = pair.client.encrypt(encoder.encode('{"type":"confirm","protocol":1}'));
    expect(JSON.parse(decoder.decode(pair.server.decrypt(confirm)))).toEqual({ type: "confirm", protocol: 1 });
  });
  it("rejects a wrong pinned host key and closes the failed session", () => {
    const pair = peers(), wrong = generateKeypair();
    const attacker = track(initiator({ privateKey: pair.device.privateKey, remotePublicKey: wrong.publicKey }));
    expect(() => pair.server.readHandshake(attacker.writeHandshake(new Uint8Array()))).toThrow("Secure connection failed.");
    expect(pair.server.remotePublicKey).toBeNull(); expect(() => pair.server.writeHandshake(new Uint8Array())).toThrow("Secure connection failed.");
  });
  it("binds routing context and refuses downgrade between pairing and reconnect", () => {
    const pair = peers(new Uint8Array(32).fill(4));
    const changedContext = track(responder({ privateKey: pair.host.privateKey, psk: new Uint8Array(32).fill(4), prologue: encoder.encode("other-host") }));
    const frame = pair.client.writeHandshake(new Uint8Array());
    expect(() => changedContext.readHandshake(frame)).toThrow("Secure connection failed.");
    const downgraded = track(responder({ privateKey: pair.host.privateKey }));
    expect(() => downgraded.readHandshake(frame)).toThrow("Secure connection failed.");
  });
  it.each(["tamper", "replay", "reorder"])("fails closed on %s without allowing nonce retries", (attack) => {
    const pair = peers(); handshake(pair);
    const one = pair.client.encrypt(encoder.encode("one")), two = pair.client.encrypt(encoder.encode("two"));
    if (attack === "tamper") one[0] ^= 1;
    if (attack === "replay") pair.server.decrypt(one);
    expect(() => pair.server.decrypt(attack === "reorder" ? two : one)).toThrow("Secure connection failed.");
    expect(() => pair.server.decrypt(two)).toThrow("Secure connection failed.");
  });
  it("fresh reconnect uses fresh ephemeral keys and rejects prior-session ciphertext", () => {
    const pair = peers(); handshake(pair); const previous = pair.client.encrypt(encoder.encode("private message"));
    const client = track(initiator({ privateKey: pair.device.privateKey, remotePublicKey: pair.host.publicKey }));
    const server = track(responder({ privateKey: pair.host.privateKey }));
    const first = client.writeHandshake(new Uint8Array()); server.readHandshake(first); client.readHandshake(server.writeHandshake(new Uint8Array()));
    expect(client.encrypt(encoder.encode("private message"))).not.toEqual(previous);
    expect(() => server.decrypt(previous)).toThrow("Secure connection failed.");
  });
  it("supports the maximum Noise record and refuses one excess plaintext byte", () => {
    const pair = peers(); handshake(pair); const plain = new Uint8Array(MAX_NOISE_PLAINTEXT).fill(5);
    const encrypted = pair.client.encrypt(plain); expect(encrypted).toHaveLength(MAX_NOISE_FRAME); expect(pair.server.decrypt(encrypted)).toEqual(plain);
    expect(() => pair.client.encrypt(new Uint8Array(MAX_NOISE_PLAINTEXT + 1))).toThrow("Secure connection failed.");
    expect(() => pair.client.encrypt(new Uint8Array())).toThrow("Secure connection failed.");
  });
  it.each([0, 15, MAX_NOISE_FRAME + 1])("rejects malformed ciphertext length %s", (length) => {
    const pair = peers(); handshake(pair); expect(() => pair.server.decrypt(new Uint8Array(length))).toThrow("Secure connection failed.");
  });
  it("rejects oversized handshakes, invalid phase calls, and closed sessions", () => {
    const pair = peers(); expect(() => pair.server.readHandshake(new Uint8Array(MAX_NOISE_FRAME + 1))).toThrow("Secure connection failed.");
    expect(() => pair.client.encrypt(new Uint8Array())).toThrow("Secure connection failed.");
    const ready = peers(); handshake(ready); expect(() => ready.client.writeHandshake(new Uint8Array())).toThrow("Secure connection failed.");
    ready.server.close(); ready.server.close(); expect(ready.server.handshakeFinished).toBe(false); expect(ready.server.remotePublicKey).toBeNull();
    expect(() => ready.server.decrypt(new Uint8Array(16))).toThrow("Secure connection failed.");
  });
  it.each([0, 31, 33, 64])("rejects invalid key and PSK lengths (%s) with fixed diagnostics", (length) => {
    const key = generateKeypair();
    expect(() => initiator({ privateKey: new Uint8Array(length), remotePublicKey: key.publicKey })).toThrow("Secure connection failed.");
    expect(() => initiator({ privateKey: key.privateKey, remotePublicKey: new Uint8Array(length) })).toThrow("Secure connection failed.");
    expect(() => responder({ privateKey: key.privateKey, psk: new Uint8Array(length) })).toThrow("Secure connection failed.");
  });
});
