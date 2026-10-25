import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { generateKeyPairSync, sign } from "node:crypto";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { resolve } from "node:path";
import { afterAll, expect, it } from "vitest";
import { RelayClose, RelayToHost, loginMessage } from "@tjommie/protocol";

const root = resolve(import.meta.dirname, "../../..");
const hostId = "99cf98ba-2638-4b16-8a6e-1691c1673b45";
let child: ChildProcessWithoutNullStreams | null = null;
let configDir: string | null = null;

function message(socket: WebSocket, name: string): Promise<string | ArrayBuffer> {
  return new Promise((resolveMessage, reject) => {
    const cleanup = () => { clearTimeout(timer); socket.removeEventListener("message", onMessage); socket.removeEventListener("close", onClose); socket.removeEventListener("error", onError); };
    const onMessage = (event: MessageEvent) => { cleanup(); resolveMessage(event.data as string | ArrayBuffer); };
    const onClose = (event: CloseEvent) => { cleanup(); reject(new Error(`${name} closed ${event.code}`)); };
    const onError = () => { cleanup(); reject(new Error(`${name} WebSocket error`)); };
    const timer = setTimeout(() => { cleanup(); reject(new Error(`${name} message timed out`)); }, 10_000);
    socket.addEventListener("message", onMessage);
    socket.addEventListener("close", onClose);
    socket.addEventListener("error", onError);
  });
}

function closed(socket: WebSocket, name: string): Promise<number> {
  return new Promise((resolveClose, reject) => {
    const timer = setTimeout(() => reject(new Error(`${name} close timed out, state ${socket.readyState}`)), 10_000);
    socket.addEventListener("close", (event) => { clearTimeout(timer); resolveClose(event.code); }, { once: true });
  });
}

function socket(url: string): WebSocket {
  const value = new WebSocket(url);
  value.binaryType = "arraybuffer";
  return value;
}

async function waitOpen(socket: WebSocket): Promise<void> {
  await new Promise<void>((resolveOpen, reject) => {
    socket.addEventListener("open", () => resolveOpen(), { once: true });
    socket.addEventListener("error", () => reject(new Error("WebSocket upgrade failed")), { once: true });
  });
}

async function startRelay(): Promise<string> {
  configDir = await mkdtemp(resolve(tmpdir(), "tjommie-relay-"));
  child = spawn(process.execPath, [resolve(root, "node_modules/wrangler/bin/wrangler.js"), "dev", "--local", "--port", "0", "--persist-to", configDir, "--config", "packages/relay/wrangler.toml"], {
    cwd: root, windowsHide: true, stdio: ["pipe", "pipe", "pipe"],
    env: { ...process.env, CI: "true", WRANGLER_SEND_METRICS: "false", XDG_CONFIG_HOME: configDir },
  });
  return await new Promise((resolveUrl, reject) => {
    let output = "";
    const ports = new Set<string>();
    const timer = setTimeout(() => { clearInterval(probe); reject(new Error(`Wrangler did not start: ${output.slice(-1000)}`)); }, 30_000);
    const probe = setInterval(() => {
      for (const port of ports) void fetch(`http://127.0.0.1:${port}/v1/health`).then(async (response) => {
        if (response.ok && (await response.json() as { relay?: string }).relay === "tjommie-relay/1") {
          clearTimeout(timer); clearInterval(probe);
          child?.stdout.off("data", collect); child?.stderr.off("data", collect);
          resolveUrl(`ws://127.0.0.1:${port}`);
        }
      }).catch(() => {});
    }, 250);
    const collect = (chunk: Buffer) => {
      output += chunk.toString();
      for (const match of output.matchAll(/http:\/\/(?:127\.0\.0\.1|localhost):(\d+)/g)) if (match[1] !== "0") ports.add(match[1]);
    };
    child?.stdout.on("data", collect);
    child?.stderr.on("data", collect);
    child?.once("exit", (code) => { clearTimeout(timer); clearInterval(probe); reject(new Error(`Wrangler exited ${code}: ${output.slice(-1000)}`)); });
  });
}

afterAll(async () => {
  if (child?.pid) {
    const running = child;
    await new Promise<void>((resolveDone) => {
      if (running.exitCode !== null) { resolveDone(); return; }
      const timer = setTimeout(() => { running.kill(); running.stdout.destroy(); running.stderr.destroy(); resolveDone(); }, 3_000);
      running.once("exit", () => { clearTimeout(timer); resolveDone(); });
      running.stdin.end("q\n");
    });
  }
  if (configDir && configDir.startsWith(tmpdir())) {
    for (let attempt = 0; attempt < 10; attempt++) {
      try { await rm(configDir, { recursive: true, force: true }); break; }
      catch (error) {
        if (attempt === 9) throw error;
        await new Promise((resolveDelay) => setTimeout(resolveDelay, 250));
      }
    }
  }
});

it("pairs a real host and device through the local Worker and reports offline after host loss", async () => {
  const base = await startRelay();
  const path = `/v1/hosts/${hostId}`;
  const initiallyOffline = socket(`${base}/v1/hosts/264f5f6d-8c7f-4e62-9d65-7ac60a7884a3/connect`);
  const initialClose = closed(initiallyOffline, "initial offline");
  await waitOpen(initiallyOffline);
  initiallyOffline.send(Uint8Array.of(1));
  expect(await initialClose).toBe(RelayClose.hostOffline);
  const control = socket(`${base}${path}/control`);
  const challengeMessage = message(control, "challenge");
  await waitOpen(control);
  const challenge = RelayToHost.parse(JSON.parse(await challengeMessage as string));
  if (challenge.t !== "challenge") throw new Error("Expected challenge");
  const keys = generateKeyPairSync("ed25519");
  const publicKey = keys.publicKey.export({ type: "spki", format: "der" }).subarray(-32).toString("base64url");
  const ready = message(control, "ready");
  control.send(JSON.stringify({ t: "login", publicKey, signature: sign(null, loginMessage(hostId, challenge.nonce, new URL(base).host), keys.privateKey).toString("base64url") }));
  expect(RelayToHost.parse(JSON.parse(await ready as string))).toMatchObject({ t: "ready", push: false });
  const opened = message(control, "open");
  const device = socket(`${base}${path}/connect`);
  await waitOpen(device);
  const early = Uint8Array.of(1, 2, 3);
  device.send(early);
  const channel = RelayToHost.parse(JSON.parse(await opened as string));
  if (channel.t !== "open") throw new Error("Expected channel");
  const host = socket(`${base}${path}/channels/${channel.channel}`);
  const received = message(host, "early frame");
  await waitOpen(host);
  expect(new Uint8Array(await received as ArrayBuffer)).toEqual(early);
  const reply = message(device, "reply frame");
  host.send(Uint8Array.of(4, 5));
  expect(new Uint8Array(await reply as ArrayBuffer)).toEqual(Uint8Array.of(4, 5));
  const deviceClosed = closed(device, "device");
  control.close();
  expect(await deviceClosed).toBe(RelayClose.hostOffline);
  const offline = socket(`${base}${path}/connect`);
  const offlineClosed = closed(offline, "offline");
  await waitOpen(offline);
  offline.send(Uint8Array.of(1));
  expect(await offlineClosed).toBe(RelayClose.hostOffline);
  host.close();
});
