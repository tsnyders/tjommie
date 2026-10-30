import { createPublicKey, generateKeyPairSync, sign, verify } from "node:crypto";
import { afterEach, expect, it, vi } from "vitest";
import { LEGACY_PUSH_KINDS, loginMessage } from "@tjommie/protocol";
import type { PeerWire } from "./peer-wire.ts";
import { RelayClient } from "./relay-client.ts";

const hostId = "13653af3-8ca6-4e95-bc95-0949edc28715";
const nonce = Buffer.alloc(32, 7).toString("base64url");
const channel = Buffer.alloc(32, 8).toString("base64url");

class FakeSocket extends EventTarget {
  readyState = 0;
  bufferedAmount = 0;
  sent: string[] = [];
  readonly url: string;
  constructor(url: string) { super(); this.url = url; }
  open(): void { this.readyState = 1; this.dispatchEvent(new Event("open")); }
  receive(value: object | string): void {
    this.dispatchEvent(Object.assign(new Event("message"), { data: typeof value === "string" ? value : JSON.stringify(value) }));
  }
  receiveBinary(): void { this.dispatchEvent(Object.assign(new Event("message"), { data: new Uint8Array([1]).buffer })); }
  send(value: string): void { this.sent.push(value); }
  close(code = 1000): void { if (this.readyState === 3) return; this.readyState = 3; this.dispatchEvent(Object.assign(new Event("close"), { code })); }
}

function setup(onState = () => {}, useDefaultWire = false) {
  const keyPair = generateKeyPairSync("ed25519");
  const publicKey = new Uint8Array(keyPair.publicKey.export({ format: "der", type: "spki" }).subarray(-32));
  const sockets: FakeSocket[] = [];
  const wires: PeerWire[] = [];
  let url: string | null = "https://relay.example/old-base?unused=1";
  let failChannel = false;
  const client = new RelayClient({
    url: () => url, hostId, identity: { publicKey, sign: (message) => new Uint8Array(sign(null, message, keyPair.privateKey)) },
    onChannel: (wire) => wires.push(wire), onState,
    socket: (address) => { if (failChannel && address.includes("/channels/")) throw new Error(channel); const fake = new FakeSocket(address); sockets.push(fake); return fake as unknown as WebSocket; },
    ...(useDefaultWire ? {} : { wire: (_socket: WebSocket): PeerWire => ({ kind: "relay", address: "relay", buffered: 0, write: () => {}, destroy: () => {}, onFrame: () => {}, onClose: () => {} }) }),
  });
  return { client, sockets, wires, publicKey, setUrl: (value: string | null) => { url = value; }, setFailChannel: () => { failChannel = true; } };
}

afterEach(() => vi.useRealTimers());

it("signs the domain-separated challenge and handles a channel leg", () => {
  const { client, sockets, wires, publicKey } = setup();
  client.start();
  expect(sockets[0].url).toBe(`wss://relay.example/v1/hosts/${hostId}/control`);
  sockets[0].open();
  sockets[0].receive({ t: "challenge", nonce });
  const login = JSON.parse(sockets[0].sent[0]) as { t: string; publicKey: string; signature: string };
  expect(login.t).toBe("login");
  expect(login.publicKey).toBe(Buffer.from(publicKey).toString("base64url"));
  const key = createPublicKey({ key: { kty: "OKP", crv: "Ed25519", x: login.publicKey }, format: "jwk" });
  expect(verify(null, loginMessage(hostId, nonce, "relay.example"), key, Buffer.from(login.signature, "base64url"))).toBe(true);
  sockets[0].receive({ t: "ready", push: true });
  expect(client.state()).toBe("online");
  sockets[0].receive({ t: "open", channel });
  expect(sockets[1].url).toBe(`wss://relay.example/v1/hosts/${hostId}/channels/${channel}`);
  sockets[1].open();
  expect(wires).toHaveLength(1);
  expect(wires[0].kind).toBe("relay");
  client.stop();
});

it("drift triggers relogin", async () => {
  vi.useFakeTimers();
  vi.setSystemTime(0);
  const { client, sockets, publicKey } = setup();
  client.start(); sockets[0].open(); sockets[0].receive({ t: "challenge", nonce }); sockets[0].receive({ t: "ready", push: true });
  vi.setSystemTime(60_000);
  await vi.advanceTimersByTimeAsync(5_000);
  expect(sockets).toHaveLength(2);
  expect(sockets[0].readyState).toBe(3);
  const nextNonce = Buffer.alloc(32, 6).toString("base64url");
  sockets[1].open(); sockets[1].receive({ t: "challenge", nonce: nextNonce });
  const login = JSON.parse(sockets[1].sent[0]) as { signature: string };
  const key = createPublicKey({ key: { kty: "OKP", crv: "Ed25519", x: Buffer.from(publicKey).toString("base64url") }, format: "jwk" });
  expect(verify(null, loginMessage(hostId, nextNonce, "relay.example"), key, Buffer.from(login.signature, "base64url"))).toBe(true);
  sockets[1].receive({ t: "ready", push: true });
  expect(client.state()).toBe("online");
  client.stop();
});

it("rereads a changed relay URL on restart", () => {
  const { client, sockets, setUrl } = setup();
  client.start();
  setUrl("https://new-relay.example/subpath");
  client.stop(); client.start();
  expect(sockets[1].url).toBe(`wss://new-relay.example/v1/hosts/${hostId}/control`);
  client.stop();
});

it("uses 1, 2, 5, 10 and 30 second retry delays", async () => {
  vi.useFakeTimers();
  const { client, sockets } = setup();
  client.start();
  for (const delay of [1_000, 2_000, 5_000, 10_000, 30_000, 30_000]) {
    sockets.at(-1)!.close();
    await vi.advanceTimersByTimeAsync(delay - 1);
    const count = sockets.length;
    await vi.advanceTimersByTimeAsync(1);
    expect(sockets).toHaveLength(count + 1);
  }
  client.stop();
});

it("matches push replies to requests", async () => {
  vi.useFakeTimers();
    const { client, sockets } = setup();
    client.start(); sockets[0].open(); sockets[0].receive({ t: "challenge", nonce }); sockets[0].receive({ t: "ready", push: true });
    const pending = client.push(["synthetic-device-token"], "done", "bot-1", "team");
    const sent = JSON.parse(sockets[0].sent[1]) as { t: string; id: number; tokens: string[]; kind: string; botId: string; roomId: string };
    expect(sent.tokens).toEqual(["synthetic-device-token"]);
    expect(sent).toEqual({ t: "push", id: sent.id, tokens: ["synthetic-device-token"], kind: "done", botId: "bot-1", roomId: "team" });
    sockets[0].receive({ t: "pushed", id: sent.id, results: [{ ok: true, unregistered: false }] });
    await expect(pending).resolves.toEqual([{ ok: true, unregistered: false }]);
    sockets[0].receive({ t: "open", channel });
    sockets[1].close();
    client.stop();
});

it("assumes legacy kinds until a ready advertises more, then resets on disconnect", () => {
  const { client, sockets } = setup();
  expect(client.kinds()).toEqual(LEGACY_PUSH_KINDS);
  client.start(); sockets[0].open(); sockets[0].receive({ t: "challenge", nonce });
  sockets[0].receive({ t: "ready", push: true, kinds: ["approval", "done"] });
  expect(client.kinds()).toEqual(["approval", "done"]);
  client.stop();
  expect(client.kinds()).toEqual(LEGACY_PUSH_KINDS);
});

it("keeps a newer relay online while advertising only known push kinds", () => {
  const { client, sockets } = setup();
  client.start(); sockets[0].open(); sockets[0].receive({ t: "challenge", nonce });
  sockets[0].receive({ t: "ready", push: true, kinds: ["approval", "limit", "routine_failed", "done", "failed", "mention"] });
  expect(client.state()).toBe("online");
  expect(client.kinds()).toEqual(["approval", "limit", "routine_failed", "done", "failed"]);
  client.stop();
});

it("keeps secrets out of all console methods and errors on failure paths", async () => {
  vi.useFakeTimers();
  const logs = (["log", "info", "warn", "error"] as const).map((method) => vi.spyOn(console, method).mockImplementation(() => {}));
  try {
    const { client, sockets, publicKey, setFailChannel } = setup();
    client.start(); sockets[0].open(); sockets[0].receive({ t: "challenge", nonce }); sockets[0].receive({ t: "ready", push: true });
    const pending = client.push(["synthetic-device-token"], "approval", "bot-1");
    client.stop();
    await expect(pending).rejects.not.toThrow("synthetic-device-token");
    client.start(); sockets[1].open(); sockets[1].receive({ t: "challenge", nonce }); sockets[1].receive({ t: "ready", push: true });
    const timedOut = client.push(["synthetic-device-token"], "approval", "bot-1");
    const timedOutCheck = expect(timedOut).rejects.not.toThrow("synthetic-device-token");
    await vi.advanceTimersByTimeAsync(10_000);
    await timedOutCheck;
    setFailChannel(); sockets[1].receive({ t: "open", channel });
    client.stop(); client.start(); sockets[2].open(); sockets[2].receive({ t: "open", channel });
    expect(client.state()).toBe("error");
    client.stop();
    const output = JSON.stringify(logs.flatMap((spy) => spy.mock.calls));
    for (const secret of ["synthetic-device-token", nonce, channel, Buffer.from(publicKey).toString("base64url")]) expect(output).not.toContain(secret);
  } finally { logs.forEach((spy) => spy.mockRestore()); }
});

it("pings every 30 seconds and reconnects when pong stops", async () => {
  vi.useFakeTimers(); vi.setSystemTime(0);
  const { client, sockets } = setup();
  client.start(); sockets[0].open(); sockets[0].receive({ t: "challenge", nonce }); sockets[0].receive({ t: "ready", push: true });
  await vi.advanceTimersByTimeAsync(29_999);
  expect(sockets[0].sent).toHaveLength(1);
  await vi.advanceTimersByTimeAsync(1);
  expect(sockets[0].sent[1]).toBe("ping");
  sockets[0].receive("pong");
  await vi.advanceTimersByTimeAsync(30_000);
  expect(sockets[0].sent[2]).toBe("ping");
  await vi.advanceTimersByTimeAsync(50_000);
  expect(client.state()).toBe("error");
  client.stop();
});

it("times out login and rejects invalid control messages", async () => {
  vi.useFakeTimers();
  const { client, sockets } = setup();
  client.start(); sockets[0].open();
  await vi.advanceTimersByTimeAsync(10_000);
  expect(client.state()).toBe("error");
  client.stop();
  for (const invalid of [{ t: "ready", push: true }, { t: "open", channel }, { t: "pushed", id: 1, results: [] }, "{bad-json", { t: "challenge", nonce: "bad" }]) {
    client.start(); const socket = sockets.at(-1)!; socket.open(); socket.receive(invalid);
    expect(socket.readyState).toBe(3);
    client.stop();
  }
  client.start(); const socket = sockets.at(-1)!; socket.open(); socket.receiveBinary();
  expect(socket.readyState).toBe(3);
  client.stop();
  client.start(); const repeated = sockets.at(-1)!; repeated.open();
  repeated.receive({ t: "challenge", nonce }); repeated.receive({ t: "challenge", nonce });
  expect(repeated.readyState).toBe(3);
  client.stop();
});

it("resets backoff after ready and uses a 30 second cap after close 4401", async () => {
  vi.useFakeTimers();
  const { client, sockets } = setup();
  client.start(); sockets[0].close();
  await vi.advanceTimersByTimeAsync(1_000);
  sockets[1].open(); sockets[1].receive({ t: "challenge", nonce }); sockets[1].receive({ t: "ready", push: true });
  sockets[1].close();
  await vi.advanceTimersByTimeAsync(1_000);
  expect(sockets).toHaveLength(3);
  sockets[2].close(4401);
  expect(client.state()).toBe("error");
  await vi.advanceTimersByTimeAsync(29_999);
  expect(sockets).toHaveLength(3);
  await vi.advanceTimersByTimeAsync(1);
  expect(sockets).toHaveLength(4);
  sockets[3].close(4404);
  await vi.advanceTimersByTimeAsync(30_000);
  expect(sockets).toHaveLength(5);
  client.stop();
});

it("rejects pending pushes on stop or bad result counts and caps channels", async () => {
  const { client, sockets } = setup();
  client.start(); sockets[0].open(); sockets[0].receive({ t: "challenge", nonce }); sockets[0].receive({ t: "ready", push: true });
  const mismatched = client.push(["synthetic-token"], "limit", "bot-1");
  const id = (JSON.parse(sockets[0].sent[1]) as { id: number }).id;
  sockets[0].receive({ t: "pushed", id, results: [] });
  await expect(mismatched).rejects.toThrow("unavailable");
  for (let index = 0; index < 33; index++) sockets[0].receive({ t: "open", channel });
  expect(sockets).toHaveLength(33);
  const pending = client.push(["synthetic-token"], "limit", "bot-1");
  client.stop();
  await expect(pending).rejects.toThrow("unavailable");
});

it("uses the real WebSocket wire by default and contains a throwing state callback", () => {
  const { client, sockets, wires } = setup(() => { throw new Error("synthetic UI failure"); }, true);
  expect(() => client.start()).not.toThrow();
  sockets[0].open(); sockets[0].receive({ t: "challenge", nonce });
  expect(() => sockets[0].receive({ t: "ready", push: true })).not.toThrow();
  sockets[0].receive({ t: "open", channel }); sockets[1].open();
  expect(wires[0].kind).toBe("relay");
  client.stop();
});
