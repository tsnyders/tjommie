import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { isAbsolute, join, relative, resolve } from "node:path";
import { tmpdir } from "node:os";
import { expect, it } from "vitest";
import { parsePairingCode } from "@tjommie/protocol";
import { connectTransport, type Client } from "../../client/src/client.ts";
import { relayConnectUrl, webSocketFrames } from "../../client/src/frame-socket.ts";
import { authenticateRemote } from "../../client/src/remote-peer.ts";
import { generateKeypair } from "../../transport/src/index.ts";
import type { CredentialTransport } from "./account-credentials.ts";
import { openDb, type Db } from "./db.ts";
import { RelayClient } from "./relay-client.ts";
import { loadRelayIdentity } from "./relay-identity.ts";
import { RemoteHost } from "./remote-host.ts";

const root = resolve(import.meta.dirname, "../../..");

function startLocalRelay(configDir: string): { child: ChildProcessWithoutNullStreams; ready: Promise<string> } {
  const child = spawn(process.execPath, [resolve(root, "node_modules/wrangler/bin/wrangler.js"), "dev", "--local", "--port", "0",
    "--persist-to", configDir, "--config", "packages/relay/wrangler.toml"], {
    cwd: root, windowsHide: true, stdio: ["pipe", "pipe", "pipe"],
    env: { PATH: process.env.PATH ?? "", SystemRoot: process.env.SystemRoot ?? "C:\\Windows", WINDIR: process.env.SystemRoot ?? "C:\\Windows",
      TEMP: tmpdir(), TMP: tmpdir(), USERPROFILE: configDir, APPDATA: configDir, LOCALAPPDATA: configDir, HOME: configDir,
      CI: "true", WRANGLER_SEND_METRICS: "false", XDG_CONFIG_HOME: configDir },
  });
  const ready = new Promise<string>((resolveBase, reject) => {
    let output = "", finished = false;
    const ports = new Set<string>();
    const finish = (error?: Error, port?: string): void => {
      if (finished) return; finished = true; clearTimeout(timeout); clearInterval(probe);
      if (error) reject(error); else resolveBase(`https://127.0.0.1:${port}`);
    };
    const timeout = setTimeout(() => finish(new Error("Local relay did not start.")), 30_000);
    const probe = setInterval(() => {
      for (const port of ports) void fetch(`http://127.0.0.1:${port}/v1/health`).then(async (response) => {
        if (response.ok && (await response.json() as { relay?: string }).relay === "tjommie-relay/1") finish(undefined, port);
      }).catch(() => {});
    }, 250);
    const collect = (chunk: Buffer): void => {
      output = (output + chunk.toString()).slice(-4_000);
      for (const match of output.matchAll(/http:\/\/(?:127\.0\.0\.1|localhost):(\d+)/g)) if (match[1] !== "0") ports.add(match[1]);
    };
    child.stdout.on("data", collect); child.stderr.on("data", collect);
    child.once("error", () => finish(new Error("Local relay could not start.")));
    child.once("exit", () => finish(new Error("Local relay exited before it was ready.")));
  });
  return { child, ready };
}

async function stopLocalRelay(child: ChildProcessWithoutNullStreams): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return;
  await new Promise<void>((resolveDone) => {
    const timeout = setTimeout(() => { child.kill(); resolveDone(); }, 3_000);
    child.once("exit", () => { clearTimeout(timeout); resolveDone(); });
    child.stdin.end("q\n");
  });
}

it("pairs a real RemoteHost and Node device through the local relay with no reachable LAN endpoint", async () => {
  const configDir = await mkdtemp(join(tmpdir(), "tjommie-t5-relay-"));
  let child: ChildProcessWithoutNullStreams | null = null;
  let db: Db | null = null, host: RemoteHost | null = null, relay: RelayClient | null = null, client: Client | null = null;
  try {
    const started = startLocalRelay(configDir);
    child = started.child;
    const base = await started.ready;
    const values = new Map<string, string>();
    const credentials: CredentialTransport = async ({ operation, target, key }) => {
      if (operation === "write" && key) values.set(target, key);
      return operation === "read" ? values.get(target) ?? null : null;
    };
    db = openDb(":memory:");
    host = new RemoteHost({ db, root: join(configDir, "host"), port: 0, bind: "127.0.0.1", addresses: () => ["192.0.2.1"], credentials,
      relayUrl: () => relay?.state() === "online" ? base : null,
      dispatch: async (method) => { if (method === "bots.list") return []; throw new Error("Unexpected RPC"); } });
    await host.ensureListening();
    const hostId = host.status().hostId;
    if (!hostId) throw new Error("Missing host identity");
    const identity = await loadRelayIdentity({ root: join(configDir, "host"), credentials });
    relay = new RelayClient({ url: () => base, hostId, identity, onChannel: (wire) => host?.acceptWire(wire), onState: () => {},
      socket: (address) => new WebSocket(address.replace(/^wss:/, "ws:")) });
    relay.start();
    await relay.waitUntilOnline();
    const code = parsePairingCode((await host.createPairing()).code);
    expect(code.relay).toBe(base);
    expect(code.lan[0].host).toBe("192.0.2.1");
    const wire = await webSocketFrames(relayConnectUrl(base, hostId).replace(/^wss:/, "ws:"), 10_000);
    const deviceKey = generateKeypair();
    const paired = await authenticateRemote({ wire, privateKey: deviceKey.privateKey, hostPublicKey: Buffer.from(code.hostKey, "base64url"),
      psk: Buffer.from(code.secret, "base64url"), name: "Synthetic device", preface: { v: 1, mode: "pair", hostId, pairingId: code.pairingId } });
    expect(paired.relay).toBe(base);
    client = await connectTransport(paired.transport);
    expect(await client.call("remote.ping", {})).toEqual({});
    expect(await client.call("bots.list", {})).toEqual([]);
    await client.close(); client = null;
    const resumeWire = await webSocketFrames(relayConnectUrl(base, hostId).replace(/^wss:/, "ws:"), 10_000);
    const resumed = await authenticateRemote({ wire: resumeWire, privateKey: deviceKey.privateKey,
      hostPublicKey: Buffer.from(code.hostKey, "base64url"), name: "Synthetic device",
      preface: { v: 1, mode: "resume", hostId } });
    expect(resumed.deviceId).toBe(paired.deviceId);
    expect(resumed.relay).toBe(base);
    client = await connectTransport(resumed.transport);
    expect(await client.call("remote.ping", {})).toEqual({});
    await client.close(); client = null;
    host.revoke(paired.deviceId);
    const refusedWire = await webSocketFrames(relayConnectUrl(base, hostId).replace(/^wss:/, "ws:"), 10_000);
    await expect(authenticateRemote({ wire: refusedWire, privateKey: deviceKey.privateKey,
      hostPublicKey: Buffer.from(code.hostKey, "base64url"), name: "Synthetic device",
      preface: { v: 1, mode: "resume", hostId } })).rejects.toMatchObject({ name: "RemoteRefusedError", reason: "revoked" });
  } finally {
    await client?.close(); relay?.stop(); await host?.close(); db?.close();
    if (child) await stopLocalRelay(child);
    const insideTemp = relative(tmpdir(), configDir);
    if (insideTemp && !insideTemp.startsWith("..") && !isAbsolute(insideTemp)) await rm(configDir, { recursive: true, force: true, maxRetries: 10, retryDelay: 250 });
  }
});
