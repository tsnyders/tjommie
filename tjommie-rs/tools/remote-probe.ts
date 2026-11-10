// Cross-implementation probe for the Rust remote host (M7). It drives the real TypeScript client, packages/client's
// authenticateRemote and connectTransport, so Rust tests can check that a TypeScript device pairs with and talks to
// the Rust host over LAN TCP and through the local relay.
// Usage: node remote-probe.ts. One JSON command per stdin line, one JSON reply per stdout line. Codes and keys travel
// on stdin and stdout only, never in argv. Nothing here listens on a network address except the local relay helper,
// which wrangler binds to 127.0.0.1.
import { createConnection } from "node:net";
import { mkdtemp, rm } from "node:fs/promises";
import { readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { createInterface } from "node:readline";
import type { ChildProcessWithoutNullStreams } from "node:child_process";
import { parsePairingCode, type PairingCode, type RemotePreface } from "@tjommie/protocol";
import { connectTransport, type Client, type ClientEvent } from "../../packages/client/src/client.ts";
import { FrameDecoder, lengthPrefixed } from "../../packages/client/src/encrypted-channel.ts";
import { relayConnectUrl, webSocketFrames, type FrameTransport } from "../../packages/client/src/frame-socket.ts";
import { authenticateRemote, RemoteRefusedError } from "../../packages/client/src/remote-peer.ts";
import { generateKeypair, initNoise } from "../../packages/transport/src/index.ts";
import { startLocalRelay, stopLocalRelay } from "../../packages/relay/src/local-relay.test-helper.ts";
import { loadRelayIdentity } from "../../packages/daemon/src/relay-identity.ts";

initNoise(readFileSync(new URL("../../packages/transport/wasm/tjommie_noise_bg.wasm", import.meta.url)));

type Via = "lan" | "relay";
type Command =
  | { op: "pair"; code: string; via: Via; psk?: "wrong" }
  | { op: "resume"; via: Via }
  | { op: "call"; method: string; params: unknown }
  | { op: "event"; name: string }
  | { op: "close" }
  | { op: "relay-start" }
  | { op: "relay-stop" }
  // 0.4.2's relay login key in a one-entry in-memory vault (never Credential Manager): what it stores, and its signature.
  | { op: "relay-identity"; saved: string | null; message: string };

const device = generateKeypair();
let code: PairingCode | null = null;
let client: Client | null = null;
let relay: { child: ChildProcessWithoutNullStreams; base: string; dir: string } | null = null;
const events: ClientEvent[] = [];
const reply = (value: unknown): void => { process.stdout.write(`${JSON.stringify(value)}\n`); };

// The LAN transport the TypeScript tests use: one TCP connection, 4-byte length-prefixed Noise frames.
function lanWire(host: string, port: number): Promise<FrameTransport> {
  return new Promise((resolve, reject) => {
    const socket = createConnection({ host, port });
    const decoder = new FrameDecoder();
    const frames = new Set<(frame: Uint8Array) => void>(), closed = new Set<(code?: number) => void>();
    socket.once("error", reject);
    socket.once("connect", () => {
      socket.off("error", reject); socket.on("error", () => socket.destroy());
      socket.on("data", (chunk) => { try { for (const frame of decoder.feed(chunk)) for (const listener of frames) listener(frame); } catch { socket.destroy(); } });
      socket.on("close", () => { for (const listener of closed) listener(); });
      resolve({
        send: (frame) => { socket.write(lengthPrefixed(frame)); }, close: () => { socket.destroy(); },
        onFrame: (listener) => { frames.add(listener); return () => { frames.delete(listener); }; },
        onClose: (listener) => { closed.add(listener); return () => { closed.delete(listener); }; },
      });
    });
  });
}

// The local relay serves plain HTTP; the code carries its https spelling, so swap the scheme back for the socket.
function wire(via: Via, pairing: PairingCode): Promise<FrameTransport> {
  if (via === "lan") return lanWire(pairing.lan[0].host, pairing.lan[0].port);
  if (!pairing.relay) throw new Error("The pairing code has no relay.");
  return webSocketFrames(relayConnectUrl(pairing.relay, pairing.hostId).replace(/^wss:/, "ws:"), 10_000);
}

async function authenticate(via: Via, preface: RemotePreface, psk?: Uint8Array) {
  if (!code) throw new Error("No pairing code yet.");
  const paired = await authenticateRemote({ wire: await wire(via, code), privateKey: device.privateKey,
    hostPublicKey: Buffer.from(code.hostKey, "base64url"), psk, name: "Synthetic TypeScript device", preface });
  client = await connectTransport(paired.transport);
  client.subscribe((event) => { events.push(event); });
  return { ok: true, deviceId: paired.deviceId, hostId: paired.hostId, hostName: paired.hostName, relay: paired.relay };
}

async function run(command: Command): Promise<unknown> {
  switch (command.op) {
    case "pair": {
      code = parsePairingCode(command.code);
      const psk = command.psk === "wrong" ? new Uint8Array(32).fill(99) : Buffer.from(code.secret, "base64url");
      return authenticate(command.via, { v: 1, mode: "pair", hostId: code.hostId, pairingId: code.pairingId }, psk);
    }
    case "resume":
      if (!code) throw new Error("No pairing code yet.");
      return authenticate(command.via, { v: 1, mode: "resume", hostId: code.hostId });
    case "call": {
      if (!client) throw new Error("Not connected.");
      // The method name comes from the Rust test; the client's typed API is bypassed on purpose.
      const call = client.call as (method: string, params: unknown) => Promise<unknown>;
      try { return { ok: true, result: await call(command.method, command.params) }; }
      catch (error) {
        const failure = error as { code?: unknown; message?: unknown };
        return { ok: false, code: typeof failure.code === "string" ? failure.code : "closed", message: String(failure.message) };
      }
    }
    case "event": {
      const deadline = Date.now() + 10_000;
      while (Date.now() < deadline) {
        const index = events.findIndex((event) => event.event === command.name);
        if (index >= 0) return { ok: true, ...events.splice(index, 1)[0] };
        await new Promise((resolve) => setTimeout(resolve, 20));
      }
      return { ok: false, error: "no event" };
    }
    case "close": {
      const closing = client; client = null;
      if (closing) { await closing.close(); }
      return { ok: true };
    }
    case "relay-start": {
      const dir = await mkdtemp(join(tmpdir(), "tjommie-rs-relay-"));
      const started = await startLocalRelay(dir);
      relay = { ...started, dir };
      return { ok: true, base: started.base };
    }
    case "relay-identity": {
      let saved = command.saved;
      const identity = await loadRelayIdentity({ root: "remote-probe-root", credentials: async ({ operation, key }) => {
        if (operation === "write" && key) saved = key;
        return operation === "read" ? saved : null;
      } });
      return { ok: true, saved, publicKey: Buffer.from(identity.publicKey).toString("base64url"),
        signature: Buffer.from(identity.sign(Buffer.from(command.message, "base64url"))).toString("base64url") };
    }
    case "relay-stop": {
      if (relay) { await stopLocalRelay(relay.child); await rm(relay.dir, { recursive: true, force: true, maxRetries: 10, retryDelay: 250 }); relay = null; }
      return { ok: true };
    }
  }
}

const lines = createInterface({ input: process.stdin });
for await (const line of lines) {
  if (!line.trim()) continue;
  const command = JSON.parse(line) as Command;
  try { reply(await run(command)); }
  catch (error) {
    if (error instanceof RemoteRefusedError) reply({ ok: false, refused: error.reason });
    else reply({ ok: false, error: error instanceof Error ? error.message : String(error) });
  }
}
// stdin closed: the Rust test is done. Leave nothing running.
await run({ op: "close" }).catch(() => {});
await run({ op: "relay-stop" }).catch(() => {});
