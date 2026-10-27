import { existsSync, mkdirSync, mkdtempSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { createConnection } from "node:net";
import { fileURLToPath } from "node:url";
import { parsePairingCode, RELAY, type BotInput, type BotPatch, type ProviderSetup } from "@tjommie/protocol";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import WebSocket from "ws";
import { createBot, getStoredBot, setLimitedUntil } from "./bots.ts";
import { showBrowser } from "./browser.ts";
import { browsers } from "./browser-manager.ts";
import { BrowserMcp } from "./browser-mcp.ts";
import { history, updateMessage } from "./chat.ts";
import { connect, connectTransport, type Client } from "@tjommie/client";
import { FrameDecoder, lengthPrefixed } from "../../client/src/encrypted-channel.ts";
import { authenticateRemote, type FrameTransport } from "../../client/src/remote-peer.ts";
import { generateKeypair } from "../../transport/src/index.ts";
import { startDaemon, type Daemon, type DaemonAuth } from "./daemon.ts";
import { startNativeBrowserHost } from "../../desktop/src/native-browser-host.ts";
import { newId, openDb, type Db } from "./db.ts";
import { addDevice, setPushToken } from "./devices-store.ts";
import * as dbModule from "./db.ts";
import { dataPaths } from "./paths.ts";
import { GoogleAuth } from "./google-auth.ts";
import type { GoogleVault, GoogleVaultRecord } from "./google-vault.ts";
import type { ProviderEvent, ProviderSession, StartOptions } from "./providers/types.ts";
import { enqueue, getWorkItem, newChain, setStatus } from "./queue.ts";
import { addRun, createRoutine, getRun, listRuns } from "./routines.ts";
import { Scheduler } from "./scheduler.ts";
import * as serverModule from "./server.ts";
import { checkSetup, ensureAccounts, prepareCodexSandbox, signIn } from "./setup.ts";
import { protectCodexData, protectCodexDataAsync } from "./windows-acl.ts";
import { Team } from "./team.ts";
import { tempRoot, waitFor } from "./testing.ts";
import type { CredentialTransport } from "./account-credentials.ts";
import { RemoteHost } from "./remote-host.ts";
import { RelayClient } from "./relay-client.ts";
import { setSettings } from "./settings.ts";

const removal = vi.hoisted(() => ({
  failPath: "",
  beforeRemove: () => {},
  failTokenPath: "",
  tokenOptions: undefined as Parameters<typeof import("node:fs").rmSync>[1],
}));
vi.mock("node:fs", async (original) => {
  const fs = await original<typeof import("node:fs")>();
  return { ...fs, rmSync: (...args: Parameters<typeof fs.rmSync>) => {
    if (String(args[0]) === removal.failTokenPath) {
      removal.tokenOptions = args[1];
      throw Object.assign(new Error("Forced locked token file"), { code: "EBUSY" });
    }
    return fs.rmSync(...args);
  }, renameSync: (...args: Parameters<typeof fs.renameSync>) => {
    if (String(args[0]) === removal.failPath) {
      removal.beforeRemove();
      throw new Error("Forced locked folder");
    }
    return fs.renameSync(...args);
  } };
});
vi.mock("./setup.ts", async (original) => ({
  ...await original<typeof import("./setup.ts")>(), checkSetup: vi.fn(), signIn: vi.fn(), prepareCodexSandbox: vi.fn(),
}));
vi.mock("./windows-acl.ts", async (original) => ({
  ...await original<typeof import("./windows-acl.ts")>(), protectCodexData: vi.fn(), protectCodexDataAsync: vi.fn(),
}));
vi.mock("./browser.ts", async (original) => ({
  ...await original<typeof import("./browser.ts")>(), showBrowser: vi.fn(),
}));

class Session implements ProviderSession {
  listener: (event: ProviderEvent) => void = () => {};
  sent: string[] = [];
  closed = false;
  interruptions = 0;
  answers: string[] = [];
  async start(): Promise<void> {}
  async send(text: string): Promise<void> { this.sent.push(text); }
  async steer(): Promise<boolean> { return false; }
  async interrupt(): Promise<void> { this.interruptions++; this.emit({ type: "turn_done", ok: false, error: null }); }
  answer(id: string): void { this.answers.push(id); }
  close(): void { this.closed = true; }
  pid(): undefined { return undefined; }
  onEvent(listener: (event: ProviderEvent) => void): void { this.listener = listener; }
  emit(event: ProviderEvent): void { this.listener(event); }
  finish(text: string): void {
    this.emit({ type: "text", delta: text });
    this.emit({ type: "text_end" });
    this.emit({ type: "turn_done", ok: true, error: null });
  }
}

const setup = (signedIn = true, sandbox: "ready" | "notConfigured" | "updateRequired" | "unavailable" = "ready"): ProviderSetup[] => [
  { provider: "claude", installed: true, version: "test", signedIn, account: null, plan: null, error: null },
  { provider: "codex", installed: true, version: "test", signedIn, account: null, plan: null, error: null, sandbox },
];
const input = (name: string, extra: Partial<BotInput> = {}): BotInput => ({
  name, color: "green", shape: "pill", instructions: "Be terse", provider: "claude", model: "haiku", ...extra,
});
const daemons: Daemon[] = [];
const clients: Client[] = [];
const databases: Db[] = [];
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(checkSetup).mockResolvedValue(setup());
  vi.mocked(protectCodexData).mockReturnValue({ status: "ready", ready: true, reason: null });
  vi.mocked(protectCodexDataAsync).mockResolvedValue({ status: "ready", ready: true, reason: null });
  vi.mocked(prepareCodexSandbox).mockResolvedValue(undefined);
  vi.mocked(showBrowser).mockResolvedValue(undefined);
  removal.failPath = "";
  removal.beforeRemove = () => {};
  removal.failTokenPath = "";
  removal.tokenOptions = undefined;
});
afterEach(async () => {
  for (const client of clients.splice(0)) await client.close();
  for (const daemon of daemons.splice(0)) await daemon.close();
  for (const db of databases.splice(0)) db.close();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

async function harness(root = tempRoot(), auth: DaemonAuth = { mode: "development" }, googleAuth?: GoogleAuth,
  remote?: { credentials?: CredentialTransport; port?: number; bind?: string; addresses?: () => string[] }, relayUrl: string | null = null) {
  const paths = dataPaths(root);
  const settingsDb = openDb(existsSync(join(root, "tjommie.db")) ? join(root, "tjommie.db") : paths.db);
  setSettings(settingsDb, { relayUrl });
  settingsDb.close();
  const sessions: { botId: string; options: StartOptions; session: Session }[] = [];
  const daemon = await startDaemon({ root, port: 0, auth, googleAuth, remote, createSession: (bot, options) => {
    const session = new Session();
    sessions.push({ botId: bot.id, options, session });
    return session;
  } });
  daemons.push(daemon);
  const client = await connect(daemon.port, daemon.token);
  clients.push(client);
  const db = openDb(dataPaths(root).db);
  databases.push(db);
  return { daemon, client, db, sessions, root };
}

async function lanWire(port: number): Promise<FrameTransport> {
  const socket = createConnection({ host: "127.0.0.1", port });
  await new Promise<void>((resolve, reject) => { socket.once("connect", resolve); socket.once("error", reject); });
  const frames = new Set<(frame: Uint8Array) => void>(), closed = new Set<() => void>();
  const decoder = new FrameDecoder();
  socket.on("data", (chunk) => { try { for (const frame of decoder.feed(chunk)) for (const listener of frames) listener(frame); } catch { socket.destroy(); } });
  socket.on("close", () => { for (const listener of closed) listener(); });
  socket.on("error", () => socket.destroy());
  return { send: (frame) => { socket.write(lengthPrefixed(frame)); }, close: () => { socket.destroy(); },
    onFrame: (listener) => { frames.add(listener); return () => { frames.delete(listener); }; },
    onClose: (listener) => { closed.add(listener); return () => { closed.delete(listener); } } };
}

class SilentRelaySocket extends EventTarget {
  static OPEN = 1;
  static created = 0;
  static closed = 0;
  readyState = 0;
  bufferedAmount = 0;
  binaryType = "blob";
  constructor() { super(); SilentRelaySocket.created++; }
  send(_value: string | Uint8Array): void {}
  close(): void { if (this.readyState === 3) return; SilentRelaySocket.closed++; this.readyState = 3; this.dispatchEvent(Object.assign(new Event("close"), { code: 1000 })); }
}

function appServer(options: StartOptions) {
  const server = options.mcpServers?.find((entry) => entry.name === "tjommie");
  if (!server || !("url" in server)) throw new Error("Missing Tjommie MCP server");
  return server;
}

it("routes production browser RPC and scoped MCP tools to the private desktop host", async () => {
  const desktopHostToken = "h".repeat(43);
  const { daemon, client, sessions } = await harness(tempRoot(), { mode: "production", token: "t".repeat(43), desktopHostToken });
  const seen: string[] = [];
  const host = startNativeBrowserHost({ port: daemon.port, token: desktopHostToken, execute: async (bot, command) => {
    seen.push(command.kind);
    if (command.kind === "close" || command.kind === "delete_profile" || command.kind === "cancel_tools") return {};
    if (command.kind === "tool") return { content: [{ type: "text", text: "Synthetic native snapshot" }] };
    return { botId: bot.id, generation: "native-g", control: "bot", revision: 1, activePageId: "native-p", tabs: [{ id: "native-p", title: "Native", url: "https://example.test/" }], dialog: null };
  } });
  const opening = vi.spyOn(browsers, "open");
  try {
    const bot = await client.call("bots.create", input("Native bridge", { browser: true }));
    expect(await client.call("browser.open", { botId: bot.id })).toMatchObject({ generation: "native-g" });
    expect(await client.call("browser.frame", { botId: bot.id })).toMatchObject({ frame: null });
    await client.call("bots.showBrowser", { id: bot.id });
    expect(opening).not.toHaveBeenCalled();
    await client.call("chat.send", { botId: bot.id, text: "Synthetic native tool turn" });
    await waitFor(() => sessions[0]?.session.sent.length);
    const descriptor = sessions[0].options.mcpServers?.find((entry) => entry.name === "browser");
    if (!descriptor || !("url" in descriptor)) throw new Error("Native browser MCP descriptor missing.");
    const response = await fetch(descriptor.url, { method: "POST", headers: descriptor.headers, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "browser_snapshot", arguments: {} } }) });
    expect(await response.json()).toMatchObject({ result: { content: [{ type: "text", text: "Synthetic native snapshot" }] } });
    expect(seen).toContain("tool");
    sessions[0].session.finish("Done");
    await client.call("bots.update", { id: bot.id, patch: { browser: false } });
    expect(seen).toContain("close");
    expect((await fetch(`http://127.0.0.1:${daemon.port}/browser-host`, { headers: { Authorization: `Bearer ${daemon.token}` } })).status).toBe(401);
  } finally { await host.close(); }
});

async function tool(options: StartOptions, name: string, args: Record<string, unknown>) {
  const server = appServer(options);
  const response = await fetch(server.url, {
    method: "POST", headers: { ...server.headers, "Content-Type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name, arguments: args } }),
  });
  expect(response.status).toBe(200);
  const result: unknown = await response.json();
  expect(result).not.toHaveProperty("error");
  expect(result).not.toHaveProperty("result.isError", true);
  return result;
}

it("wires settings, routine CRUD and the Team room through the WebSocket methods", async () => {
  const { client } = await harness();
  const bot = await client.call("bots.create", input("Roomy"));
  expect(await client.call("settings.get", {})).toEqual({ chainLimit: 5, relayUrl: null });
  expect(await client.call("settings.set", { chainLimit: 7 })).toEqual({ chainLimit: 7, relayUrl: null });
  await client.call("room.post", { text: "Hello team" });
  expect(await client.call("room.history", {})).toMatchObject({ messages: [{ body: "Hello team" }], hasMore: false });
  const routine = await client.call("routines.create", { botId: bot.id, name: "Check", prompt: "Check", days: [1], times: ["08:00"] });
  expect(await client.call("routines.list", { botId: bot.id })).toEqual([routine]);
  expect(await client.call("routines.runs", { routineId: routine.id })).toEqual([]);
  expect(await client.call("routines.update", { id: routine.id, patch: { enabled: false } })).toMatchObject({ enabled: false });
  await client.call("routines.delete", { id: routine.id });
  expect(await client.call("routines.list", { botId: bot.id })).toEqual([]);
  await expect(client.call("routines.runs", { routineId: routine.id })).rejects.toThrow(/not_found/);
  await expect(client.call("routines.list", { botId: "missing" })).rejects.toThrow(/not_found/);
  expect(client.events.some((event) => event.event === "settings.changed")).toBe(true);
  expect(client.events.some((event) => event.event === "routine.deleted")).toBe(true);
});

it("starts the relay for an invite after LAN listening, restarts on settings, and sends content-free pushes", async () => {
  const values = new Map<string, string>();
  let relayWrites = 0, listeningResolved = false, createdBeforeListening = false;
  const credentials: CredentialTransport = async ({ operation, target, key }) => {
    if (operation === "write" && key) { values.set(target, key); if (target.endsWith("/relay-login")) relayWrites++; }
    return operation === "read" ? values.get(target) ?? null : null;
  };
  const { client, db, sessions } = await harness(tempRoot(), { mode: "development" }, undefined,
    { credentials, port: 0, bind: "127.0.0.1", addresses: () => ["127.0.0.1"] });
  const ensureListening = RemoteHost.prototype.ensureListening;
  vi.spyOn(RemoteHost.prototype, "ensureListening").mockImplementation(async function (this: RemoteHost) {
    await ensureListening.call(this); listeningResolved = true;
  });
  const sockets: RelaySocket[] = [];
  class RelaySocket extends EventTarget {
    static OPEN = 1;
    readonly url: string;
    readyState = 0;
    bufferedAmount = 0;
    binaryType = "blob";
    sent: string[] = [];
    constructor(url: string) {
      super(); if (!listeningResolved) createdBeforeListening = true; this.url = url; sockets.push(this);
      queueMicrotask(() => { this.readyState = 1; this.dispatchEvent(new Event("open"));
        this.receive({ t: "challenge", nonce: Buffer.alloc(32, 5).toString("base64url") }); });
    }
    receive(value: object): void { this.dispatchEvent(Object.assign(new Event("message"), { data: JSON.stringify(value) })); }
    send(value: string | Uint8Array): void {
      if (typeof value !== "string") return;
      this.sent.push(value);
      const message = JSON.parse(value) as { t?: string; id?: number; tokens?: string[] };
      if (message.t === "login") queueMicrotask(() => this.receive({ t: "ready", push: true, kinds: ["approval", "limit", "routine_failed", "done", "failed"] }));
      if (message.t === "push") queueMicrotask(() => this.receive({ t: "pushed", id: message.id, results: message.tokens?.map(() => ({ ok: true, unregistered: false })) }));
    }
    close(): void { if (this.readyState === 3) return; this.readyState = 3; this.dispatchEvent(Object.assign(new Event("close"), { code: 1000 })); }
  }
  vi.stubGlobal("WebSocket", RelaySocket);
  try {
    expect(await client.call("remote.ping", {})).toEqual({});
    await expect(client.call("device.registerPush", { token: "synthetic-token" })).rejects.toMatchObject({ code: "bad_state" });
    await client.call("settings.set", { relayUrl: "https://relay.example/old-base" });
    expect(sockets).toHaveLength(0);
    expect((await client.call("remote.status", {})).relay).toEqual({ url: "https://relay.example/old-base", state: "off", push: false });
    const invite = await client.call("pairing.create", {});
    expect(createdBeforeListening).toBe(false);
    expect(parsePairingCode(invite.code).relay).toBe("https://relay.example/old-base");
    expect(sockets).toHaveLength(1);
    expect(sockets[0].url).toContain("/control");
    const status = await client.call("remote.status", {});
    expect(status.listening).toBe(true);
    expect(status.relay).toEqual({ url: "https://relay.example/old-base", state: "online", push: true });
    expect([...values.keys()].filter((target) => target.endsWith("/relay-login"))).toHaveLength(1);
    await client.call("settings.set", { relayUrl: "https://new-relay.example" });
    await waitFor(() => sockets.length === 2);
    expect(sockets[0].readyState).toBe(3);
    expect(sockets[1].url).toContain("new-relay.example");
    expect(relayWrites).toBe(1);
    expect((await client.call("remote.status", {})).port).toBe(status.port);
    const device = addDevice(db, { name: "Synthetic phone", publicKey: Buffer.alloc(32, 9).toString("base64url") });
    setPushToken(db, device.id, "synthetic-device-token");
    const bot = await client.call("bots.create", input("Relay bot"));
    await client.call("chat.send", { botId: bot.id, text: "Start" });
    await waitFor(() => sessions[0]?.session.sent.length);
    sessions[0].session.emit({ type: "approval", id: "relay-approval", tool: "Write", summary: "Private summary", detail: "Private body", rule: null });
    await waitFor(() => sockets[1].sent.some((value) => value.includes('"t":"push"')));
    const push = sockets[1].sent.find((value) => value.includes('"t":"push"'))!;
    expect(push).toContain('"kind":"approval"');
    expect(push).toContain('"botId":"' + bot.id + '"');
    expect(push).not.toContain("Private summary");
    expect(push).not.toContain("Private body");
    sessions[0].session.emit({ type: "turn_done", ok: false, error: "Private first error" });
    await client.call("room.post", { text: "@everyone Report" });
    await waitFor(() => sessions.some(({ session }) => session.sent.some((value) => value.includes("Report"))));
    const roomSession = sessions.find(({ session }) => session.sent.some((value) => value.includes("Report")))!;
    roomSession.session.finish("Private room reply");
    await waitFor(() => sockets[1].sent.some((value) => value.includes('"roomId":"team"')));
    const roomPush = JSON.parse(sockets[1].sent.find((value) => value.includes('"roomId":"team"'))!) as Record<string, unknown>;
    expect(roomPush).toEqual({ t: "push", id: expect.any(Number), tokens: ["synthetic-device-token"], kind: "done", botId: bot.id, roomId: "team" });
    expect(JSON.stringify(roomPush)).not.toContain("Private room reply");
    expect(sockets[1].sent.join("")).not.toContain("Private first error");
    await client.call("pairing.cancel", { id: invite.id });
    expect((await client.call("remote.status", {})).relay.state).toBe("online");
    await client.call("devices.revoke", { id: device.id });
    expect((await client.call("remote.status", {})).relay.state).toBe("online");
    expect(client.events.some((event) => event.event === "devices.changed")).toBe(true);
  } finally { vi.unstubAllGlobals(); }
});

it("keeps the relay available for the last revoked device after restarting the daemon", async () => {
  const values = new Map<string, string>();
  const credentials: CredentialTransport = async ({ operation, target, key }) => {
    if (operation === "write" && key) values.set(target, key);
    return operation === "read" ? values.get(target) ?? null : null;
  };
  try {
    const root = tempRoot();
    const remote = { credentials, port: 0, bind: "127.0.0.1", addresses: () => ["127.0.0.1"] };
    const first = await harness(root, { mode: "development" }, undefined, remote);
    const invite = await first.client.call("pairing.create", {});
    await first.client.call("pairing.cancel", { id: invite.id });
    const device = addDevice(first.db, { name: "Only phone", publicKey: Buffer.alloc(32, 7).toString("base64url") });
    expect((await first.client.call("devices.list", {})).map((value) => value.id)).toContain(device.id);
    SilentRelaySocket.created = SilentRelaySocket.closed = 0;
    vi.stubGlobal("WebSocket", SilentRelaySocket);
    await first.client.call("settings.set", { relayUrl: "https://relay.example" });
    expect((await first.client.call("remote.status", {})).relay.state).toBe("connecting");
    expect(SilentRelaySocket.created).toBe(1);
    await first.client.call("devices.revoke", { id: device.id });
    await first.client.close(); clients.splice(clients.indexOf(first.client), 1);
    await first.daemon.close(); daemons.splice(daemons.indexOf(first.daemon), 1);
    first.db.close(); databases.splice(databases.indexOf(first.db), 1);

    const second = await startDaemon({ root, port: 0, auth: { mode: "development" }, remote });
    daemons.push(second);
    await waitFor(() => SilentRelaySocket.created === 2, 5_000);
    vi.unstubAllGlobals();
    const client = await connect(second.port, second.token);
    clients.push(client);
    expect((await client.call("devices.list", {}))[0].revokedAt).not.toBeNull();
    expect((await client.call("remote.status", {})).relay.state).toBe("connecting");
  } finally { vi.unstubAllGlobals(); }
});

it("announces the configured relay in ready when a device resumes over LAN during relay login", async () => {
  const values = new Map<string, string>();
  const credentials: CredentialTransport = async ({ operation, target, key }) => {
    if (operation === "write" && key) values.set(target, key);
    return operation === "read" ? values.get(target) ?? null : null;
  };
  const { client } = await harness(tempRoot(), { mode: "development" }, undefined,
    { credentials, port: 0, bind: "127.0.0.1", addresses: () => ["127.0.0.1"] });
  const code = parsePairingCode((await client.call("pairing.create", {})).code);
  const deviceKey = generateKeypair();
  const paired = await authenticateRemote({ wire: await lanWire(code.lan[0].port), privateKey: deviceKey.privateKey,
    hostPublicKey: Buffer.from(code.hostKey, "base64url"), psk: Buffer.from(code.secret, "base64url"),
    name: "Synthetic device", preface: { v: 1, mode: "pair", hostId: code.hostId, pairingId: code.pairingId } });
  const phone = await connectTransport(paired.transport);
  clients.push(phone);
  await phone.close();
  SilentRelaySocket.created = SilentRelaySocket.closed = 0;
  vi.stubGlobal("WebSocket", SilentRelaySocket);
  try {
    await client.call("settings.set", { relayUrl: "https://relay.example" });
    expect(SilentRelaySocket.created).toBe(1);
    expect((await client.call("remote.status", {})).relay.state).toBe("connecting");
    const resumed = await authenticateRemote({ wire: await lanWire(code.lan[0].port), privateKey: deviceKey.privateKey,
      hostPublicKey: Buffer.from(code.hostKey, "base64url"), name: "Synthetic device",
      preface: { v: 1, mode: "resume", hostId: code.hostId } });
    expect(resumed.deviceId).toBe(paired.deviceId);
    expect(resumed.relay).toBe("https://relay.example");
    const resumedPhone = await connectTransport(resumed.transport);
    clients.push(resumedPhone);
    expect(await resumedPhone.call("remote.ping", {})).toEqual({});
  } finally { vi.unstubAllGlobals(); }
});

it("keeps ready relay metadata while a separate pairing code is being built", async () => {
  const values = new Map<string, string>();
  const credentials: CredentialTransport = async ({ operation, target, key }) => {
    if (operation === "write" && key) values.set(target, key);
    return operation === "read" ? values.get(target) ?? null : null;
  };
  const { client } = await harness(tempRoot(), { mode: "development" }, undefined,
    { credentials, port: 0, bind: "127.0.0.1", addresses: () => ["127.0.0.1"] });
  const code = parsePairingCode((await client.call("pairing.create", {})).code);
  const key = generateKeypair();
  const paired = await authenticateRemote({ wire: await lanWire(code.lan[0].port), privateKey: key.privateKey,
    hostPublicKey: Buffer.from(code.hostKey, "base64url"), psk: Buffer.from(code.secret, "base64url"),
    name: "Synthetic device", preface: { v: 1, mode: "pair", hostId: code.hostId, pairingId: code.pairingId } });
  await paired.transport.close();
  SilentRelaySocket.created = SilentRelaySocket.closed = 0;
  vi.stubGlobal("WebSocket", SilentRelaySocket);
  const entered = Promise.withResolvers<void>(), release = Promise.withResolvers<void>();
  const createPairing = RemoteHost.prototype.createPairing;
  vi.spyOn(RemoteHost.prototype, "createPairing").mockImplementation(async function (this: RemoteHost) {
    entered.resolve();
    await release.promise;
    return createPairing.call(this);
  });
  vi.spyOn(RelayClient.prototype, "waitUntilOnline").mockResolvedValue();
  let waiting: Promise<unknown> | undefined;
  try {
    await client.call("settings.set", { relayUrl: "https://relay.example" });
    waiting = client.call("pairing.create", {});
    await entered.promise;
    const resumed = await authenticateRemote({ wire: await lanWire(code.lan[0].port), privateKey: key.privateKey,
      hostPublicKey: Buffer.from(code.hostKey, "base64url"), name: "Synthetic device",
      preface: { v: 1, mode: "resume", hostId: code.hostId } });
    expect(resumed.relay).toBe("https://relay.example");
    resumed.transport.close();
  } finally {
    release.resolve();
    await waiting?.catch(() => {});
    vi.unstubAllGlobals();
  }
});

it("returns a LAN-only pairing code when the relay socket never answers", async () => {
  const credentials: CredentialTransport = async () => null;
  const { client } = await harness(tempRoot(), { mode: "development" }, undefined,
    { credentials, port: 0, bind: "127.0.0.1", addresses: () => ["127.0.0.1"] });
  await client.call("settings.set", { relayUrl: "https://relay.example" });
  SilentRelaySocket.created = SilentRelaySocket.closed = 0;
  vi.stubGlobal("WebSocket", SilentRelaySocket);
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  try {
    const pairing = client.call("pairing.create", {});
    await vi.waitFor(() => expect(SilentRelaySocket.created).toBe(1));
    await vi.advanceTimersByTimeAsync(RELAY.acceptTimeoutMs);
    const invite = await pairing;
    expect(parsePairingCode(invite.code).relay).toBeNull();
    expect((await client.call("remote.status", {})).relay.state).toBe("error");
  } finally { vi.useRealTimers(); vi.unstubAllGlobals(); }
});

it("keeps the relay running while another pairing request still waits", async () => {
  const credentials: CredentialTransport = async () => null;
  const { client } = await harness(tempRoot(), { mode: "development" }, undefined,
    { credentials, port: 0, bind: "127.0.0.1", addresses: () => ["127.0.0.1"] });
  await client.call("settings.set", { relayUrl: "https://relay.example" });
  SilentRelaySocket.created = SilentRelaySocket.closed = 0;
  vi.stubGlobal("WebSocket", SilentRelaySocket);
  const waiting: Array<() => void> = [];
  vi.spyOn(RelayClient.prototype, "waitUntilOnline").mockImplementation(() => new Promise<void>((resolve) => { waiting.push(resolve); }));
  try {
    const first = client.call("pairing.create", {});
    const second = client.call("pairing.create", {});
    await vi.waitFor(() => expect(waiting).toHaveLength(2));
    waiting[0]();
    await first;
    expect(SilentRelaySocket.closed).toBe(0);
    waiting[1]();
    await second;
  } finally { vi.unstubAllGlobals(); }
});

it("reports a startup LAN listen failure in relay status", async () => {
  const root = tempRoot();
  const db = openDb(dataPaths(root).db);
  addDevice(db, { name: "Synthetic phone", publicKey: Buffer.alloc(32, 4).toString("base64url") });
  db.close();
  vi.spyOn(RemoteHost.prototype, "ensureListening").mockRejectedValue(new Error("Synthetic bind failure"));
  vi.spyOn(console, "error").mockImplementation(() => {});
  const { client } = await harness(root, { mode: "development" }, undefined, { credentials: async () => null });
  vi.stubGlobal("WebSocket", SilentRelaySocket);
  await client.call("settings.set", { relayUrl: "https://relay.example" });
  await vi.waitFor(async () => expect((await client.call("remote.status", {})).relay.state).toBe("error"));
});

it("creates a room, posts into it, and broadcasts room and message events", async () => {
  const { client } = await harness();
  const bot = await client.call("bots.create", input("Planner"));
  const room = await client.call("rooms.create", { name: "Planning", botIds: [bot.id] });
  expect(await client.call("rooms.list", {})).toMatchObject([
    { id: "team", name: "Everyone", botIds: [bot.id], everyone: true },
    { id: room.id, name: "Planning", botIds: [bot.id], everyone: false },
  ]);
  const post = await client.call("room.post", { roomId: room.id, text: "Planning note" });
  expect(post.conversationId).toBe(room.id);
  expect((await client.call("room.history", { roomId: room.id })).messages).toMatchObject([{ id: post.id, body: "Planning note" }]);
  expect(client.events).toContainEqual({ type: "event", event: "room.updated", data: room });
  expect(client.events).toContainEqual({ type: "event", event: "message.added", data: post });
  await expect(client.call("room.post", { roomId: bot.id, text: "Wrong kind" })).rejects.toThrow(/not_found/);
  await expect(client.call("room.history", { roomId: bot.id })).rejects.toThrow(/not_found/);
  const renamed = await client.call("rooms.update", { id: room.id, patch: { name: "Plans" } });
  expect(renamed.name).toBe("Plans");
  expect(client.events).toContainEqual({ type: "event", event: "room.updated", data: renamed });
  await client.call("rooms.delete", { id: room.id });
  expect(client.events).toContainEqual({ type: "event", event: "room.deleted", data: { id: room.id } });
  expect((await client.call("rooms.list", {})).map((entry) => entry.id)).toEqual(["team"]);
});

it.each(["folder", "ask"] as const)("attaches session MCP servers and revokes their token when closing (%s)", async (autonomy) => {
  const { client, sessions } = await harness();
  const bot = await client.call("bots.create", input("Browser", { browser: true, autonomy }));
  await client.call("chat.send", { botId: bot.id, text: "Begin" });
  const first = sessions[0];
  expect(first.options.mcpServers?.map((entry) => [entry.name, entry.autoAllow])).toEqual([["tjommie", true], ["browser", autonomy !== "ask"]]);
  const browser = first.options.mcpServers.find((entry) => entry.name === "browser");
  expect(browser).toMatchObject({ url: `${appServer(first.options).url}/browser`, headers: appServer(first.options).headers });
  expect(browser).not.toHaveProperty("command");
  await tool(first.options, "list_bots", {});
  first.session.finish("Done");
  await client.call("bots.update", { id: bot.id, patch: { browser: false } });
  expect(first.session.closed).toBe(true);
  const server = appServer(first.options);
  expect((await fetch(server.url, { method: "POST", headers: server.headers, body: "{}" })).status).toBe(401);
  expect((await fetch(`${server.url}/browser`, { method: "POST", headers: server.headers, body: "{}" })).status).toBe(401);
  await client.call("chat.send", { botId: bot.id, text: "Again" });
  expect(sessions[1].options.mcpServers?.map((entry) => entry.name)).toEqual(["tjommie"]);
  expect(appServer(sessions[1].options).headers).not.toEqual(server.headers);
});

it("routes routine proposals to Team and tool approvals to the runner", async () => {
  const { client, sessions } = await harness();
  const bot = await client.call("bots.create", input("Planner"));
  await client.call("chat.send", { botId: bot.id, text: "Begin" });
  await tool(sessions[0].options, "add_routine", { name: "Morning check", prompt: "Check", days: ["mon"], times: ["08:48"] });
  const card = await waitFor(() => client.events.find((event) => event.event === "approval.added" && event.data.kind === "routine"));
  if (card.event !== "approval.added") throw new Error("Expected approval card");
  await client.call("approvals.respond", { approvalId: card.data.id, decision: "once" });
  expect(await client.call("routines.list", { botId: bot.id })).toMatchObject([{ name: "Morning check", enabled: true }]);
  expect(sessions[0].session.answers).toEqual([]);
  sessions[0].session.emit({ type: "approval", id: "cli-approval", tool: "Write", summary: "Write file", detail: "File", rule: null });
  const approval = await waitFor(() => client.events.find((event) => event.event === "approval.added" && event.data.kind === "tool"));
  if (approval.event !== "approval.added") throw new Error("Expected tool approval");
  await client.call("approvals.respond", { approvalId: approval.data.id, decision: "once" });
  expect(sessions[0].session.answers).toEqual(["cli-approval"]);
});

it.each([false, true])("keeps a pending routine proposal waiting and clears it on answer (restart: %s)", async (restart) => {
  let h = await harness();
  const bot = await h.client.call("bots.create", input("Waiting planner"));
  await h.client.call("chat.send", { botId: bot.id, text: "Begin" });
  h.sessions[0].session.finish("Ready");
  await tool(h.sessions[0].options, "add_routine", { name: "Check", prompt: "Check", days: ["mon"], times: ["08:48"] });
  const card = await waitFor(() => h.client.events.find((event) => event.event === "approval.added"));
  if (card.event !== "approval.added") throw new Error("Expected approval card");
  expect(await h.client.call("bots.list", {})).toMatchObject([{ id: bot.id, state: "waiting" }]);
  expect(h.client.events).toContainEqual({ type: "event", event: "bot.updated", data: expect.objectContaining({ id: bot.id, state: "waiting" }) });
  expect(h.client.events).toContainEqual({ type: "event", event: "notify", data: expect.objectContaining({ kind: "approval", botId: bot.id, approvalId: card.data.id }) });
  if (restart) {
    await h.client.close();
    clients.splice(clients.indexOf(h.client), 1);
    await h.daemon.close();
    daemons.splice(daemons.indexOf(h.daemon), 1);
    h = await harness(h.root);
    expect(await h.client.call("bots.list", {})).toMatchObject([{ id: bot.id, state: "waiting" }]);
    expect((await h.client.call("chat.history", { botId: bot.id })).messages).toContainEqual(expect.objectContaining({ kind: "approval", data: expect.objectContaining({ approvalId: card.data.id, status: "pending" }) }));
  }
  await h.client.call("approvals.respond", { approvalId: card.data.id, decision: "deny" });
  expect(await h.client.call("bots.list", {})).toMatchObject([{ id: bot.id, state: "idle" }]);
  expect(h.client.events).toContainEqual({ type: "event", event: "bot.updated", data: expect.objectContaining({ id: bot.id, state: "idle" }) });
});

it.each(["once", "deny"] as const)("answers a damaged routine card and clears Waiting through the API (%s)", async (decision) => {
  const { client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("Damaged card"));
  await client.call("chat.send", { botId: bot.id, text: "Begin" });
  sessions[0].session.finish("Ready");
  await tool(sessions[0].options, "add_routine", { name: "Check", prompt: "Check", days: ["mon"], times: ["08:48"] });
  const card = await waitFor(() => client.events.find((event) => event.event === "approval.added" && event.data.kind === "routine"));
  if (card.event !== "approval.added") throw new Error("Expected routine approval card");
  updateMessage(db, card.data.messageId, { data: { corrupt: "missing routine card fields" } });
  expect(await client.call("bots.list", {})).toMatchObject([{ id: bot.id, state: "waiting" }]);
  const from = client.events.length;

  const resolved = await client.call("approvals.respond", { approvalId: card.data.id, decision });
  const status = decision === "deny" ? "denied" : "allowed";
  expect(resolved).toMatchObject({ id: card.data.id, status });
  expect((await client.call("chat.history", { botId: bot.id })).messages).toContainEqual(expect.objectContaining({
    id: card.data.messageId, data: { corrupt: "missing routine card fields", status },
  }));
  expect(await client.call("bots.list", {})).toMatchObject([{ id: bot.id, state: "idle" }]);
  expect(client.events.slice(from)).toContainEqual({ type: "event", event: "bot.updated", data: expect.objectContaining({ id: bot.id, state: "idle" }) });
  expect(await client.call("routines.list", { botId: bot.id })).toHaveLength(decision === "deny" ? 0 : 1);
  await expect(client.call("approvals.respond", { approvalId: card.data.id, decision })).rejects.toThrow(/already_answered/);
});

it.each(["once", "deny"] as const)("runs queued room work while a routine proposal stays pending (%s)", async (decision) => {
  const { daemon, client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("Planner"));
  await client.call("chat.send", { botId: bot.id, text: "Begin" });
  await tool(sessions[0].options, "add_routine", { name: "Check", prompt: "Check", days: ["mon"], times: ["08:48"] });
  const card = await waitFor(() => client.events.find((event) => event.event === "approval.added" && event.data.kind === "routine"));
  if (card.event !== "approval.added") throw new Error("Expected routine approval card");
  const post = await client.call("room.post", { text: "@Planner Check the room while the proposal waits" });
  const queued = db.prepare("SELECT id FROM work_items WHERE message_id = ?").get(post.id) as { id: string };
  expect(getWorkItem(db, queued.id).status).toBe("queued");
  expect(sessions[0].session.sent).toHaveLength(1);

  sessions[0].session.finish("Ready");
  expect(getWorkItem(db, queued.id).status).toBe("running");
  expect(daemon.runner.currentWork(bot.id)?.id).toBe(queued.id);
  await waitFor(() => sessions[0].session.sent.length === 2, 2_000);
  expect(sessions[0].session.sent).toHaveLength(2);
  expect(sessions[0].session.sent[1]).toContain("Check the room while the proposal waits");
  expect(db.prepare("SELECT status FROM approvals WHERE id = ?").get(card.data.id)).toMatchObject({ status: "pending" });
  sessions[0].session.finish("Done");
  expect(getWorkItem(db, queued.id).status).toBe("done");
  expect(daemon.runner.currentWork(bot.id)).toBeNull();
  expect(await client.call("bots.list", {})).toMatchObject([{ id: bot.id, state: "waiting" }]);
  expect((await client.call("chat.history", { botId: bot.id })).messages).toContainEqual(expect.objectContaining({ kind: "approval", data: expect.objectContaining({ approvalId: card.data.id, status: "pending" }) }));
  const resolved = await client.call("approvals.respond", { approvalId: card.data.id, decision });
  expect(resolved.status).toBe(decision === "deny" ? "denied" : "allowed");
  expect(await client.call("bots.list", {})).toMatchObject([{ id: bot.id, state: "idle" }]);
});

it("starts recovered queued work while a routine proposal remains pending", async () => {
  let h = await harness();
  const bot = await h.client.call("bots.create", input("Planner"));
  await h.client.call("chat.send", { botId: bot.id, text: "Begin" });
  await tool(h.sessions[0].options, "add_routine", { name: "Check", prompt: "Check", days: ["mon"], times: ["08:48"] });
  const card = await waitFor(() => h.client.events.find((event) => event.event === "approval.added" && event.data.kind === "routine"));
  if (card.event !== "approval.added") throw new Error("Expected routine approval card");
  const post = await h.client.call("room.post", { text: "@Planner Pick this up after restart" });
  const queued = h.db.prepare("SELECT id FROM work_items WHERE message_id = ?").get(post.id) as { id: string };
  expect(getWorkItem(h.db, queued.id).status).toBe("queued");
  await h.client.close();
  clients.splice(clients.indexOf(h.client), 1);
  await h.daemon.close();
  daemons.splice(daemons.indexOf(h.daemon), 1);

  h = await harness(h.root);
  expect(getWorkItem(h.db, queued.id).status).toBe("running");
  await waitFor(() => h.sessions[0]?.session.sent.length, 2_000);
  expect(h.sessions[0].session.sent[0]).toContain("Pick this up after restart");
  expect(h.db.prepare("SELECT status FROM approvals WHERE id = ?").get(card.data.id)).toMatchObject({ status: "pending" });
  h.sessions[0].session.finish("Done after restart");
  expect(getWorkItem(h.db, queued.id).status).toBe("done");
  expect(await h.client.call("bots.list", {})).toMatchObject([{ id: bot.id, state: "waiting" }]);
  await h.client.call("approvals.respond", { approvalId: card.data.id, decision: "deny" });
  expect(await h.client.call("bots.list", {})).toMatchObject([{ id: bot.id, state: "idle" }]);
});

it("lists routines across bots and includes the bot on every run", async () => {
  const { daemon, client, sessions } = await harness();
  const bots = await Promise.all(["First planner", "Second planner"].map((name) => client.call("bots.create", input(name))));
  const routines = await Promise.all(bots.map((bot) => client.call("routines.create", { botId: bot.id, name: "Check", prompt: "Check", days: [0, 1, 2, 3, 4, 5, 6], times: ["08:48"] })));
  expect(await client.call("routines.list", {})).toEqual(expect.arrayContaining(routines));
  expect(await client.call("routines.list", { botId: bots[0].id })).toEqual([routines[0]]);
  daemon.scheduler.tick(Date.parse(routines[0].nextRunAt!));
  await waitFor(() => sessions.length === 2 && sessions.every(({ session }) => session.sent.length));
  for (const routine of routines) {
    expect(await client.call("routines.runs", { routineId: routine.id })).toMatchObject([{ botId: routine.botId }]);
  }
});

it("retries only the failed bot from a shared Team room post", async () => {
  const { daemon, client, db, sessions } = await harness();
  const first = await client.call("bots.create", input("First"));
  const second = await client.call("bots.create", input("Second"));
  const post = await client.call("room.post", { text: "@First @Second Please check" });
  await waitFor(() => sessions.length === 2 && sessions.every(({ session }) => session.sent.length));
  const failed = sessions.find(({ botId }) => botId === first.id)!.session;
  const successful = sessions.find(({ botId }) => botId === second.id)!.session;
  failed.emit({ type: "turn_done", ok: false, error: "First failed" });
  successful.finish("Second done");
  expect(history(db, "team", undefined, 50).find((message) => message.id === post.id)?.status).toBe("done");
  expect(history(db, "team", undefined, 50)).toContainEqual(expect.objectContaining({ kind: "error", data: { retryMessageId: post.id, retryBotId: first.id } }));
  const old = db.prepare("SELECT id FROM work_items WHERE bot_id = ?").get(first.id) as { id: string };
  await client.call("chat.retry", { messageId: post.id });
  await waitFor(() => failed.sent.length === 2);
  const retry = daemon.runner.currentWork(first.id)!;
  expect(retry).toMatchObject({ source: "room", conversationId: "team", messageId: post.id });
  expect(history(db, "team", undefined, 50).find((message) => message.id === post.id)?.status).toBe("done");
  expect(retry.chainId).not.toBe(getWorkItem(db, old.id).chainId);
  expect(successful.sent).toHaveLength(1);
  await expect(client.call("chat.retry", { messageId: post.id, botId: first.id })).rejects.toThrow(/bad_state/);
  failed.finish("First done");
});

it("requires a bot target when more than one room turn failed", async () => {
  const { client, sessions } = await harness();
  const first = await client.call("bots.create", input("First"));
  await client.call("bots.create", input("Second"));
  const post = await client.call("room.post", { text: "@First @Second Please check" });
  await waitFor(() => sessions.length === 2 && sessions.every(({ session }) => session.sent.length));
  for (const { session } of sessions) session.emit({ type: "turn_done", ok: false, error: "Failed" });
  await expect(client.call("chat.retry", { messageId: post.id })).rejects.toThrow(/bad_params/);
  await client.call("chat.retry", { messageId: post.id, botId: first.id });
  await waitFor(() => sessions.find(({ botId }) => botId === first.id)!.session.sent.length === 2);
  expect(sessions.find(({ botId }) => botId !== first.id)!.session.sent).toHaveLength(1);
});

it("retries a failed routine as a new run with a new label and chain", async () => {
  const { daemon, client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("Routine retry"));
  const routine = await client.call("routines.create", { botId: bot.id, name: "Check", prompt: "Check", days: [0, 1, 2, 3, 4, 5, 6], times: ["08:48"] });
  daemon.scheduler.tick(Date.parse(routine.nextRunAt!));
  await waitFor(() => sessions[0]?.session.sent.length);
  const old = daemon.runner.currentWork(bot.id)!;
  sessions[0].session.emit({ type: "turn_done", ok: false, error: "Routine failed" });
  const label = history(db, bot.id, undefined, 50).find((message) => message.kind === "routine")!;
  expect(label.status).toBe("done");
  expect(history(db, bot.id, undefined, 50)).toContainEqual(expect.objectContaining({ kind: "error", data: { retryMessageId: label.id } }));
  const nextLabel = await client.call("chat.retry", { messageId: label.id });
  await waitFor(() => sessions[0].session.sent.length === 2);
  const next = daemon.runner.currentWork(bot.id)!;
  expect(next.routineRunId).toBeTruthy();
  expect(next.routineRunId).not.toBe(old.routineRunId);
  expect(next.chainId).not.toBe(old.chainId);
  expect(next.messageId).toBe(nextLabel.id);
  expect(nextLabel.id).not.toBe(label.id);
  expect(history(db, bot.id, undefined, 50).find((message) => message.id === nextLabel.id)?.status).toBe("done");
  expect(listRuns(db, routine.id)).toEqual(expect.arrayContaining([expect.objectContaining({ id: old.routineRunId, status: "failed" }), expect.objectContaining({ id: next.routineRunId, status: "running" })]));
  await expect(client.call("chat.retry", { messageId: label.id })).rejects.toThrow(/busy/);
  sessions[0].session.finish("Recovered");
  expect(history(db, bot.id, undefined, 50).find((message) => message.id === nextLabel.id)?.data?.status).toBe("done");
});

it("settles a routine and re-arms its lifecycle after a provider limit requeue", async () => {
  const requeued = vi.spyOn(Scheduler.prototype, "runRequeued");
  const started = vi.spyOn(Scheduler.prototype, "runStarted");
  const ended = vi.spyOn(Scheduler.prototype, "runEnded");
  const teamEnded = vi.spyOn(Team.prototype, "turnEnded");
  const { daemon, client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("Clockwork"));
  const routine = await client.call("routines.create", { botId: bot.id, name: "Tick", prompt: "Tick", days: [0, 1, 2, 3, 4, 5, 6], times: ["03:00"], timeLimitMinutes: 1 });
  if (!routine.nextRunAt) throw new Error("Missing next run");
  daemon.scheduler.tick(Date.parse(routine.nextRunAt));
  await waitFor(() => sessions[0]?.session.sent.length);
  sessions[0].session.emit({ type: "limit", state: "reached", usedPercent: 100, resetsAt: new Date(Date.now() + 60_000).toISOString() });
  sessions[0].session.emit({ type: "turn_done", ok: false, error: "Limited" });
  expect(requeued).toHaveBeenCalledOnce();
  expect(ended).not.toHaveBeenCalled();
  setLimitedUntil(db, bot.id, null);
  daemon.runner.pump(bot.id);
  await waitFor(() => sessions[0].session.sent.length === 2);
  expect(started).toHaveBeenCalledTimes(2);
  sessions[0].session.finish("TICK");
  expect(listRuns(db, routine.id)).toMatchObject([{ status: "done" }]);
  expect(ended).toHaveBeenCalledOnce();
  expect(teamEnded).toHaveBeenCalledOnce();
});

it("keeps the bot, queue and exchanges intact when folder removal fails, then deletes successfully", async () => {
  const { daemon, client, db, sessions } = await harness();
  const writer = await client.call("bots.create", input("Writer"));
  const asker = await client.call("bots.create", input("Asker", { canMessage: [writer.id] }));
  await client.call("chat.send", { botId: writer.id, text: "Warm up" });
  sessions[0].session.finish("Ready");
  await client.call("bots.pause", { id: writer.id, paused: true });
  await client.call("chat.send", { botId: asker.id, text: "Ask Writer" });
  await tool(sessions[1].options, "message_bot", { bot: "Writer", message: "Help" });
  const queue = db.prepare("SELECT * FROM work_items WHERE bot_id = ?").all(writer.id);
  const exchanges = db.prepare("SELECT * FROM exchanges").all();
  const bot = getStoredBot(db, writer.id);
  removal.failPath = writer.homeFolder;
  removal.beforeRemove = () => expect(sessions[0].session.closed).toBe(true);
  const pump = vi.spyOn(daemon.runner, "pump");
  await expect(client.call("bots.delete", { id: writer.id })).rejects.toThrow(/busy/);
  expect(pump).toHaveBeenCalledWith(writer.id);
  expect(getStoredBot(db, writer.id)).toEqual(bot);
  expect(db.prepare("SELECT * FROM work_items WHERE bot_id = ?").all(writer.id)).toEqual(queue);
  expect(db.prepare("SELECT * FROM exchanges").all()).toEqual(exchanges);
  expect(client.events.some((event) => event.event === "bot.deleted")).toBe(false);
  const row = exchanges[0];
  if (typeof row.id !== "string") throw new Error("Missing exchange");
  expect(await client.call("exchanges.get", { id: row.id })).toMatchObject({ exchange: { status: "waiting" } });
  removal.failPath = "";
  await client.call("bots.delete", { id: writer.id });
  expect(existsSync(writer.homeFolder)).toBe(false);
  expect(await client.call("exchanges.get", { id: row.id })).toMatchObject({ exchange: { status: "failed" } });
  expect(client.events.filter((event) => event.event === "bot.deleted")).toHaveLength(1);
});

it("reports queued routine work dropped by routine deletion through both end hooks", async () => {
  const ended = vi.spyOn(Scheduler.prototype, "runEnded");
  const teamEnded = vi.spyOn(Team.prototype, "turnEnded");
  const { daemon, client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("Queued routine"));
  await client.call("chat.send", { botId: bot.id, text: "Keep working" });
  const routine = await client.call("routines.create", { botId: bot.id, name: "Waiting", prompt: "Check", days: [0, 1, 2, 3, 4, 5, 6], times: ["03:00"] });
  if (!routine.nextRunAt) throw new Error("Missing next run");
  daemon.scheduler.tick(Date.parse(routine.nextRunAt));
  const run = listRuns(db, routine.id)[0];
  expect(run.status).toBe("queued");
  await client.call("routines.delete", { id: routine.id });
  expect(ended).toHaveBeenCalledOnce();
  expect(ended.mock.calls[0][0].routineRunId).toBe(run.id);
  expect(teamEnded).toHaveBeenCalledOnce();
  sessions[0].session.finish("Done");
  expect(sessions[0].session.sent).toHaveLength(1);
});

it.each(["orphan", "running-work"] as const)("retains %s startup failure notifications through reconnect and restart until ack", async (source) => {
  const root = tempRoot();
  const paths = dataPaths(root);
  const seed = openDb(paths.db);
  ensureAccounts(seed, paths.accounts);
  const bot = createBot(seed, paths.bots, input("Recovery"));
  const routine = createRoutine(seed, { botId: bot.id, name: "Check", prompt: "Check", days: [1], times: ["08:00"], enabled: false });
  const run = addRun(seed, routine.id, new Date().toISOString(), "running");
  if (source === "running-work") {
    const work = enqueue(seed, { botId: bot.id, source: "routine", text: "Check", routineRunId: run.id, chainId: newChain(seed) });
    setStatus(seed, work.id, "running");
  }
  seed.close();
  const first = await harness(root);
  expect(first.client.events.filter((event) => event.event === "notify")).toEqual([]); // emitted before connection
  const notices = await first.client.call("notifications.pending", {});
  expect(notices).toEqual([expect.objectContaining({ kind: "routine_failed", botId: bot.id, routineId: routine.id, runId: run.id })]);
  expect(notices[0].body).toContain("restarted");
  expect(await first.client.call("notifications.pending", {})).toEqual(notices);
  const reconnected = await connect(first.daemon.port, first.daemon.token);
  clients.push(reconnected);
  expect(await reconnected.call("notifications.pending", {})).toEqual(notices);
  expect(history(first.db, bot.id, undefined, 20).filter((message) => message.kind === "routine")).toHaveLength(1);
  await first.daemon.close();
  daemons.splice(daemons.indexOf(first.daemon), 1);
  const second = await harness(root);
  expect(await second.client.call("notifications.pending", {})).toEqual(notices);
  expect(await second.client.call("notifications.ack", { runId: run.id })).toEqual({});
  expect(await second.client.call("notifications.ack", { runId: run.id })).toEqual({});
  expect(await second.client.call("notifications.ack", { runId: "unknown" })).toEqual({});
  expect(await second.client.call("notifications.pending", {})).toEqual([]);
  await second.daemon.close();
  daemons.splice(daemons.indexOf(second.daemon), 1);
  const third = await harness(root);
  expect(await third.client.call("notifications.pending", {})).toEqual([]);
  expect(getRun(third.db, run.id)?.status).toBe("failed");
  expect(history(third.db, bot.id, undefined, 20).filter((message) => message.kind === "routine")).toHaveLength(1);
});

it("keeps live routine failures pending until the notification consumer acknowledges them", async () => {
  const { daemon, client, sessions, db } = await harness();
  const bot = await client.call("bots.create", input("Live failure"));
  const routine = await client.call("routines.create", { botId: bot.id, name: "Check", prompt: "Check", days: [0, 1, 2, 3, 4, 5, 6], times: ["03:00"] });
  if (!routine.nextRunAt) throw new Error("Missing next run");
  daemon.scheduler.tick(Date.parse(routine.nextRunAt));
  await waitFor(() => sessions[0]?.session.sent.length);
  sessions[0].session.emit({ type: "turn_done", ok: false, error: "Provider failed" });
  const notices = await client.call("notifications.pending", {});
  expect(notices).toHaveLength(1);
  expect(client.events.filter((event) => event.event === "notify").map((event) => event.data)).toContainEqual(notices[0]);
  expect(listRuns(db, routine.id)).toMatchObject([{ status: "failed" }]);
  await client.call("notifications.ack", { runId: notices[0].runId });
  expect(await client.call("notifications.pending", {})).toEqual([]);
});

it("recovers an interrupted routine and sweeps stale shares before starting queued work", async () => {
  const root = tempRoot();
  const paths = dataPaths(root);
  const db = openDb(paths.db);
  databases.push(db);
  ensureAccounts(db, paths.accounts);
  const bot = createBot(db, paths.bots, input("Restart"));
  const routine = createRoutine(db, { botId: bot.id, name: "Interrupted", prompt: "Check", days: [1], times: ["08:00"], enabled: false });
  const run = addRun(db, routine.id, new Date().toISOString(), "running");
  const item = enqueue(db, { botId: bot.id, source: "routine", text: "Check", routineRunId: run.id, chainId: newChain(db) });
  setStatus(db, item.id, "running");
  const queued = enqueue(db, { botId: bot.id, source: "you", text: "Queued", chainId: newChain(db) });
  const stale = join(root, "shared-files", "share-old");
  mkdirSync(stale, { recursive: true });
  const deleted = join(paths.bots, `${newId()}.deleting-${newId()}`);
  mkdirSync(deleted);
  const old = new Date(Date.now() - 25 * 60 * 60_000);
  utimesSync(stale, old, old);
  const listen = serverModule.startServer;
  const server = vi.spyOn(serverModule, "startServer").mockImplementation((options) => {
    expect(options.allowedOrigins).toEqual(["app://tjommie", "http://localhost:3000"]);
    expect(getWorkItem(db, item.id).status).toBe("running");
    expect(getRun(db, run.id)?.status).toBe("running");
    expect(getWorkItem(db, queued.id).status).toBe("queued");
    expect(existsSync(stale)).toBe(true);
    expect(existsSync(deleted)).toBe(true);
    return listen(options);
  });
  const { sessions } = await harness(root);
  expect(server).toHaveBeenCalledOnce();
  await waitFor(() => sessions[0]?.session.sent.length);
  expect(getWorkItem(db, item.id).status).toBe("interrupted");
  expect(getRun(db, run.id)?.status).toBe("failed");
  expect(getWorkItem(db, queued.id).status).toBe("running");
  expect(appServer(sessions[0].options).url).toMatch(/^http:\/\/127\.0\.0\.1:\d+\/mcp$/);
  expect(existsSync(stale)).toBe(false);
  expect(existsSync(deleted)).toBe(false);
});

it("leaves the live daemon's work, approvals and streaming messages untouched on a second start", async () => {
  const { daemon, client, db, sessions, root } = await harness();
  const bot = await client.call("bots.create", input("Still working"));
  await client.call("chat.send", { botId: bot.id, text: "Begin" });
  sessions[0].session.emit({ type: "approval", id: "waiting", tool: "Write", summary: "Write", detail: "file", rule: null });
  sessions[0].session.emit({ type: "text", delta: "In progress" });
  const before = {
    work: db.prepare("SELECT * FROM work_items").all(),
    approvals: db.prepare("SELECT * FROM approvals").all(),
    messages: db.prepare("SELECT * FROM messages").all(),
  };
  expect(before.work).toMatchObject([{ status: "running" }]);
  expect(before.approvals).toMatchObject([{ status: "pending" }]);
  expect(before.messages.some((row) => row.status === "streaming")).toBe(true);
  const deleted = join(dataPaths(root).bots, `${newId()}.deleting-${newId()}`);
  mkdirSync(deleted);
  vi.mocked(checkSetup).mockClear();
  await expect(startDaemon({ root, port: daemon.port, auth: { mode: "development" } })).rejects.toMatchObject({ code: "EADDRINUSE" });
  expect(db.prepare("SELECT * FROM work_items").all()).toEqual(before.work);
  expect(db.prepare("SELECT * FROM approvals").all()).toEqual(before.approvals);
  expect(db.prepare("SELECT * FROM messages").all()).toEqual(before.messages);
  expect(checkSetup).not.toHaveBeenCalled();
  expect(existsSync(deleted)).toBe(true);
});

it.each(["development", "production"] as const)("allows the dev origin only in development mode (%s)", async (mode) => {
  const { daemon } = await harness(tempRoot(), mode === "development" ? { mode } : { mode, token: "p".repeat(43) });
  const upgrade = (origin: string) => new Promise<boolean>((resolve) => {
    const socket = new WebSocket(`ws://127.0.0.1:${daemon.port}`, { origin });
    socket.once("open", () => { socket.close(); resolve(true); });
    socket.once("error", () => resolve(false));
  });
  expect(await upgrade("app://tjommie")).toBe(true);
  expect(await upgrade("http://localhost:3000")).toBe(mode === "development");
});

it("uses a supplied production token without creating or keeping a token file", async () => {
  const root = tempRoot();
  const tokenFile = dataPaths(root).token;
  writeFileSync(tokenFile, "old-development-token");
  const { daemon } = await harness(root, { mode: "production", token: "p".repeat(43) });
  expect(daemon.token).toBe("p".repeat(43));
  expect(existsSync(tokenFile)).toBe(false);
});

it("reports a plain error when the old development token file cannot be removed", async () => {
  const cache = join(fileURLToPath(new URL("../../../", import.meta.url)), "node_modules", ".cache");
  mkdirSync(cache, { recursive: true });
  const root = mkdtempSync(join(cache, "tjommie-token-removal-"));
  const target = resolve(root);
  if (dirname(target) !== resolve(cache) || !target.startsWith(join(resolve(cache), "tjommie-token-removal-"))) {
    throw new Error("Refusing to remove a folder outside the test root.");
  }
  const tokenFile = join(root, "daemon-token");
  const settingsDb = openDb(dataPaths(root).db);
  setSettings(settingsDb, { relayUrl: null });
  settingsDb.close();
  try {
    writeFileSync(tokenFile, "old-development-token");
    removal.failTokenPath = tokenFile;
    await expect(startDaemon({ root, port: 0, auth: { mode: "production", token: "p".repeat(43) } }))
      .rejects.toThrow("Could not remove the development daemon-token file. Close programs using it and start Tjommie again.");
    expect(removal.tokenOptions).toMatchObject({ force: true, recursive: true, maxRetries: 3 });
    expect(existsSync(tokenFile)).toBe(true);
  } finally {
    removal.failTokenPath = "";
    rmSync(target, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

it("waits for a delayed session close before capturing exchanges and deleting the bot", async () => {
  const { client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("Delayed close"));
  await client.call("chat.send", { botId: bot.id, text: "Begin" });
  sessions[0].session.finish("Done");
  const exited = Promise.withResolvers<void>();
  const closing = Promise.withResolvers<void>();
  vi.spyOn(sessions[0].session, "close").mockImplementation(() => {
    closing.resolve();
  });
  Object.assign(sessions[0].session, { waitForExit: () => exited.promise });
  const captured = vi.spyOn(Team.prototype, "botDeleting");
  const deletion = client.call("bots.delete", { id: bot.id });
  try {
    await closing.promise;
    expect(captured).not.toHaveBeenCalled();
    expect(getStoredBot(db, bot.id).id).toBe(bot.id);
    expect(existsSync(bot.homeFolder)).toBe(true);
  } finally {
    exited.resolve();
    await deletion;
  }
  expect(captured).toHaveBeenCalledExactlyOnceWith(bot.id);
  expect(existsSync(bot.homeFolder)).toBe(false);
  expect(db.prepare("SELECT * FROM work_items WHERE bot_id = ?").all(bot.id)).toEqual([]);
});

it("returns busy after fifteen seconds and restores work only when the delayed exit arrives", async () => {
  const { daemon, client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("Slow close"));
  const message = await client.call("chat.send", { botId: bot.id, text: "Still running" });
  const exited = Promise.withResolvers<void>();
  const closing = Promise.withResolvers<void>();
  vi.spyOn(sessions[0].session, "close").mockImplementation(() => { closing.resolve(); });
  Object.assign(sessions[0].session, { waitForExit: () => exited.promise });
  const captured = vi.spyOn(Team.prototype, "botDeleting");
  let outcome: unknown;
  vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  const deletion = client.call("bots.delete", { id: bot.id }).then(
    (result) => { outcome = result; },
    (error: unknown) => { outcome = error; },
  );
  try {
    await closing.promise;
    await vi.advanceTimersByTimeAsync(14_999);
    expect(outcome).toBeUndefined();
    await vi.advanceTimersByTimeAsync(1);
    await vi.waitFor(() => expect(outcome).toBeInstanceOf(Error), { timeout: 200, interval: 10 });
    expect((outcome as Error).message).toMatch(/^busy: The bot is taking too long to close\./);
    expect(captured).not.toHaveBeenCalled();
    expect(getStoredBot(db, bot.id).id).toBe(bot.id);
    expect(existsSync(bot.homeFolder)).toBe(true);
    expect(db.prepare("SELECT status FROM work_items WHERE message_id = ?").get(message.id)).toEqual({ status: "running" });
    daemon.runner.pump(bot.id);
    expect(sessions).toHaveLength(1);
    exited.resolve();
    await vi.waitFor(() => expect(sessions).toHaveLength(2), { timeout: 200, interval: 10 });
    expect(sessions[1].session.sent).toContain("Still running");
    expect(client.events.some((event) => event.event === "bot.deleted" && event.data.id === bot.id)).toBe(false);
    expect(captured).not.toHaveBeenCalled();
  } finally {
    exited.resolve();
    vi.useRealTimers();
    await deletion;
  }
});

it("rejects a second Delete while the first request waits for the same process", async () => {
  const { daemon, client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("One deletion"));
  await client.call("chat.send", { botId: bot.id, text: "Running" });
  const exited = Promise.withResolvers<void>();
  const closing = Promise.withResolvers<void>();
  vi.spyOn(sessions[0].session, "close").mockImplementation(() => { closing.resolve(); });
  Object.assign(sessions[0].session, { waitForExit: () => exited.promise });
  const cancelled = vi.spyOn(daemon.runner, "cancelDeletion");
  const deletion = client.call("bots.delete", { id: bot.id });
  await closing.promise;
  let outcome: unknown;
  const duplicate = client.call("bots.delete", { id: bot.id }).then(
    (result) => { outcome = result; },
    (error: unknown) => { outcome = error; },
  );
  try {
    await vi.waitFor(() => expect(outcome).toBeInstanceOf(Error), { timeout: 200, interval: 10 });
    expect((outcome as Error).message).toMatch(/^busy:/);
    expect(cancelled).not.toHaveBeenCalled();
    expect(getStoredBot(db, bot.id).id).toBe(bot.id);
    expect(sessions).toHaveLength(1);
  } finally {
    exited.resolve();
    await deletion;
    await duplicate;
  }
  expect(existsSync(bot.homeFolder)).toBe(false);
  expect(sessions).toHaveLength(1);
});

it("pumps waiting work after unpause and a successful setup refresh", async () => {
  const { client, sessions } = await harness();
  const bot = await client.call("bots.create", input("Paused"));
  await client.call("bots.pause", { id: bot.id, paused: true });
  await client.call("chat.send", { botId: bot.id, text: "Queued" });
  expect(sessions).toHaveLength(0);
  await client.call("bots.pause", { id: bot.id, paused: false });
  await waitFor(() => sessions[0]?.session.sent.length);
  sessions[0].session.finish("Done");
  vi.mocked(checkSetup).mockResolvedValue(setup(false));
  await client.call("setup.status", {});
  await client.call("chat.send", { botId: bot.id, text: "Wait for sign-in" });
  expect(sessions[0].session.sent).toHaveLength(1);
  vi.mocked(checkSetup).mockResolvedValue(setup());
  await client.call("setup.status", {});
  await waitFor(() => sessions[0].session.sent.length === 2);
});

it("moves an existing SQLite database before opening the daemon database", async () => {
  const root = tempRoot();
  const legacy = join(root, "tjommie.db");
  const old = openDb(legacy);
  old.prepare("INSERT INTO settings (key, value) VALUES ('migration-probe', 'kept')").run();
  old.close();
  const { db } = await harness(root);
  expect(existsSync(legacy)).toBe(false);
  expect(existsSync(dataPaths(root).db)).toBe(true);
  expect(db.prepare("SELECT value FROM settings WHERE key = 'migration-probe'").get()).toEqual({ value: "kept" });
});

it("keeps Codex Own folder queued until sandbox setup succeeds and broadcasts the change", async () => {
  vi.mocked(checkSetup).mockResolvedValue(setup(true, "notConfigured"));
  const { client, sessions } = await harness();
  const bot = await client.call("bots.create", input("Codex protected", { provider: "codex", model: "test", autonomy: "folder" }));
  expect(bot).toMatchObject({ state: "needs_setup", setupReason: "Set up Codex's sandbox" });
  await client.call("chat.send", { botId: bot.id, text: "Wait" });
  expect(sessions).toHaveLength(0);
  vi.mocked(checkSetup).mockResolvedValue(setup());
  const status = await client.call("setup.prepareCodexSandbox", {});
  expect(prepareCodexSandbox).toHaveBeenCalledOnce();
  expect(status).toMatchObject({ provider: "codex", sandbox: "ready" });
  expect(client.events.some((event) => event.event === "setup.changed" && event.data.some((entry) => entry.provider === "codex" && entry.sandbox === "ready"))).toBe(true);
  expect(client.events.some((event) => event.event === "bot.updated" && event.data.id === bot.id && event.data.state === "idle")).toBe(true);
  await waitFor(() => sessions.length === 1);
});

it("fails Codex Own folder closed when a required ACL cannot be verified", async () => {
  vi.mocked(protectCodexDataAsync).mockResolvedValue({ status: "check_failed", ready: false, reason: "Tjommie could not protect its data" });
  const { client, sessions } = await harness();
  const bot = await client.call("bots.create", input("Codex ACL", { provider: "codex", model: "test", autonomy: "folder" }));
  expect(bot).toMatchObject({ state: "needs_setup", setupReason: "Tjommie could not protect its data" });
  await client.call("chat.send", { botId: bot.id, text: "Wait" });
  expect(sessions).toHaveLength(0);
});

it("protects each new Claude bot and reconciles both provider switches before returning", async () => {
  const passes: { id: string; provider: string }[][] = [];
  vi.mocked(protectCodexDataAsync).mockImplementation(async (_paths, homes) => {
    passes.push((homes ?? []).map((home) => ({ id: home.id, provider: home.provider })));
    return { status: "ready", ready: true, reason: null };
  });
  const { client } = await harness();
  const bot = await client.call("bots.create", input("Changing provider"));
  expect(passes.at(-1)).toContainEqual({ id: bot.id, provider: "claude" });
  await client.call("bots.update", { id: bot.id, patch: { provider: "codex", model: "test" } });
  expect(passes.at(-1)).toContainEqual({ id: bot.id, provider: "codex" });
  await client.call("bots.update", { id: bot.id, patch: { provider: "claude", model: "haiku" } });
  expect(passes.at(-1)).toContainEqual({ id: bot.id, provider: "claude" });
  const count = passes.length;
  await client.call("bots.delete", { id: bot.id });
  expect(passes).toHaveLength(count);
});

it.each(["create", "switch"] as const)("keeps an unrelated busy Codex Own folder turn running during a Claude bot %s", async (action) => {
  const { client, db, sessions } = await harness();
  const codex = await client.call("bots.create", input("Busy Codex", { provider: "codex", model: "test", autonomy: "folder" }));
  await client.call("chat.send", { botId: codex.id, text: "Keep working" });
  await waitFor(() => sessions[0]?.session.sent.length);
  const other = action === "switch" ? await client.call("bots.create", input("Other Codex", { provider: "codex", model: "test" })) : null;
  let release: (() => void) | undefined;
  const pending = new Promise<void>((resolve) => { release = resolve; });
  let markEntered: (() => void) | undefined;
  const entered = new Promise<void>((resolve) => { markEntered = resolve; });
  vi.mocked(protectCodexDataAsync).mockImplementationOnce(async () => {
    markEntered?.();
    await pending;
    return { status: "ready", ready: true, reason: null };
  });
  const change = other
    ? client.call("bots.update", { id: other.id, patch: { provider: "claude", model: "haiku" } })
    : client.call("bots.create", input("New Claude"));
  await entered;
  expect(sessions[0].session.interruptions).toBe(0);
  const item = db.prepare("SELECT id FROM work_items WHERE bot_id = ?").get(codex.id) as { id: string };
  expect(getWorkItem(db, item.id).status).toBe("running");
  release?.();
  await change;
  expect(sessions[0].session.interruptions).toBe(0);
  expect(sessions[0].session.closed).toBe(false);
  expect((await client.call("bots.list", {})).find((bot) => bot.id === codex.id)?.state).toBe("working");
});

it("holds a switching bot before its stopped turn can pump queued work on the old provider", async () => {
  const { client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("Switching"));
  await client.call("chat.send", { botId: bot.id, text: "First" });
  await waitFor(() => sessions[0]?.session.sent.length);
  await client.call("chat.send", { botId: bot.id, text: "Queued" });
  await waitFor(() => db.prepare("SELECT 1 FROM work_items WHERE bot_id = ? AND status = 'queued'").get(bot.id));
  let release: (() => void) | undefined;
  const pending = new Promise<void>((resolve) => { release = resolve; });
  let markEntered: (() => void) | undefined;
  const entered = new Promise<void>((resolve) => { markEntered = resolve; });
  vi.mocked(protectCodexDataAsync).mockImplementationOnce(async () => {
    markEntered?.();
    await pending;
    return { status: "ready", ready: true, reason: null };
  });
  const change = client.call("bots.update", { id: bot.id, patch: { provider: "codex", model: "test" } });
  await entered;
  expect(getStoredBot(db, bot.id).provider).toBe("claude");
  expect(sessions).toHaveLength(1);
  release?.();
  await change;
  await waitFor(() => sessions.length === 2);
  expect(sessions[1].botId).toBe(bot.id);
  expect(getStoredBot(db, bot.id).provider).toBe("codex");
});

it("keeps a provider switch held when an earlier ACL pass finishes", async () => {
  const { client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("Switching"));
  await client.call("chat.send", { botId: bot.id, text: "First" });
  await waitFor(() => sessions[0]?.session.sent.length);
  await client.call("chat.send", { botId: bot.id, text: "Queued" });
  await waitFor(() => db.prepare("SELECT 1 FROM work_items WHERE bot_id = ? AND status = 'queued'").get(bot.id));
  const releases: Array<() => void> = [];
  const entered: Array<() => void> = [];
  const passes = [0, 1].map(() => new Promise<void>((resolve) => { releases.push(resolve); }));
  const arrivals = [0, 1].map(() => new Promise<void>((resolve) => { entered.push(resolve); }));
  let pass = 0;
  vi.mocked(protectCodexDataAsync).mockImplementation(async () => {
    const index = pass++;
    entered[index]?.();
    await passes[index];
    return { status: "ready", ready: true, reason: null };
  });
  vi.spyOn(Date, "now").mockReturnValue(Date.now() + 61_000);
  const status = client.call("setup.status", {});
  await arrivals[0];
  const change = client.call("bots.update", { id: bot.id, patch: { provider: "codex", model: "test" } });
  await waitFor(() => sessions[0].session.interruptions === 1);
  releases[0]();
  await status;
  await arrivals[1];
  expect(getStoredBot(db, bot.id).provider).toBe("claude");
  expect(sessions).toHaveLength(1);
  releases[1]();
  await change;
  await waitFor(() => sessions.length === 2);
  expect(sessions[1].botId).toBe(bot.id);
  expect(getStoredBot(db, bot.id).provider).toBe("codex");
});

it("waits for Claude deny removal before the first Codex turn", async () => {
  const { client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("Switch guard"));
  let release: (() => void) | undefined;
  const pending = new Promise<void>((resolve) => { release = resolve; });
  let markEntered: (() => void) | undefined;
  const entered = new Promise<void>((resolve) => { markEntered = resolve; });
  vi.mocked(protectCodexDataAsync).mockImplementationOnce(async () => {
    markEntered?.();
    await pending;
    return { status: "ready", ready: true, reason: null };
  });
  const change = client.call("bots.update", { id: bot.id, patch: { provider: "codex", model: "test" } });
  await entered;
  expect(getStoredBot(db, bot.id).provider).toBe("claude");
  expect(sessions).toHaveLength(0);
  release?.();
  const changed = await change;
  expect(changed.state).toBe("idle");
  await client.call("chat.send", { botId: bot.id, text: "Now run" });
  await waitFor(() => sessions.length === 1);
  expect(sessions[0].options.autonomy).toBe("folder");
});

it("allows both provider switch directions while Codex sandbox setup is absent", async () => {
  vi.mocked(checkSetup).mockResolvedValue(setup(true, "notConfigured"));
  const { client, db } = await harness();
  const claude = await client.call("bots.create", input("Claude ask", { autonomy: "ask" }));
  const codex = await client.call("bots.create", input("Codex ask", { provider: "codex", model: "test", autonomy: "ask" }));
  vi.mocked(protectCodexDataAsync).mockResolvedValue({ ready: false, status: "not_configured", reason: "Set up Codex's sandbox" });
  expect(await client.call("bots.update", { id: claude.id, patch: { provider: "codex", model: "test" } })).toMatchObject({ provider: "codex", state: "idle" });
  expect(await client.call("bots.update", { id: codex.id, patch: { provider: "claude", model: "haiku" } })).toMatchObject({ provider: "claude", state: "idle" });
  expect(getStoredBot(db, claude.id).provider).toBe("codex");
  expect(getStoredBot(db, codex.id).provider).toBe("claude");
});

it("rejects invalid provider switches before stopping or dropping the running session", async () => {
  const { client, db, root, sessions } = await harness();
  const bot = await client.call("bots.create", input("Switch validation"));
  const taken = await client.call("bots.create", input("Taken name"));
  await client.call("chat.send", { botId: bot.id, text: "Keep working" });
  await waitFor(() => sessions[0]?.session.sent.length);
  sessions[0].session.emit({ type: "session", cliSessionId: "fixture-session" });
  const before = getStoredBot(db, bot.id);
  const item = db.prepare("SELECT id FROM work_items WHERE bot_id = ?").get(bot.id) as { id: string };
  const passes = vi.mocked(protectCodexDataAsync).mock.calls.length;
  const invalid: BotPatch[] = [
    { provider: "codex" },
    { provider: "codex", model: "test", name: taken.name },
    { provider: "codex", model: "test", projectFolder: join(root, "missing-project") },
    { provider: "codex", model: "test", canMessage: ["missing-bot"] },
  ];
  for (const patch of invalid) {
    await expect(client.call("bots.update", { id: bot.id, patch })).rejects.toThrow();
    expect(getStoredBot(db, bot.id)).toEqual(before);
    expect(getWorkItem(db, item.id).status).toBe("running");
    expect(sessions[0].session.interruptions).toBe(0);
    expect(sessions[0].session.closed).toBe(false);
    expect(db.prepare("SELECT COUNT(*) AS count FROM sessions WHERE bot_id = ?").get(bot.id)).toEqual({ count: 1 });
    expect(vi.mocked(protectCodexDataAsync)).toHaveBeenCalledTimes(passes);
  }
});

it("keeps a held bot's stop notice when setup changes during its stop", async () => {
  const { client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("Held Claude"));
  await client.call("chat.send", { botId: bot.id, text: "Working" });
  await waitFor(() => sessions[0]?.session.sent.length);
  const session = sessions[0].session;
  let release: (() => void) | undefined;
  const pending = new Promise<void>((resolve) => { release = resolve; });
  vi.spyOn(session, "interrupt").mockImplementation(async () => {
    session.interruptions++;
    await pending;
    session.emit({ type: "turn_done", ok: false, error: null });
  });
  const change = client.call("bots.update", { id: bot.id, patch: { provider: "codex", model: "test" } });
  await waitFor(() => session.interruptions === 1);
  const now = Date.now();
  vi.spyOn(Date, "now").mockReturnValue(now + 61_000);
  await client.call("setup.status", {});
  release?.();
  await change;
  expect(session.interruptions).toBe(1);
  expect(history(db, bot.id, undefined, 50)).toContainEqual(expect.objectContaining({ kind: "notice", body: "Stopped." }));
  expect(history(db, bot.id, undefined, 50).some((message) => message.body.includes("sandbox is no longer ready"))).toBe(false);
});

it("refuses a failed provider switch and releases its hold for old-provider work", async () => {
  const { client, db, sessions } = await harness();
  const bot = await client.call("bots.create", input("Failed switch"));
  let release: (() => void) | undefined;
  const pending = new Promise<void>((resolve) => { release = resolve; });
  let markEntered: (() => void) | undefined;
  const entered = new Promise<void>((resolve) => { markEntered = resolve; });
  vi.mocked(protectCodexDataAsync).mockImplementationOnce(async () => {
    markEntered?.();
    await pending;
    return { ready: false, status: "check_failed", reason: "Could not verify the ACL" };
  });
  const change = client.call("bots.update", { id: bot.id, patch: { provider: "codex", model: "test" } });
  await entered;
  await client.call("chat.send", { botId: bot.id, text: "Run on Claude after rollback" });
  expect(sessions).toHaveLength(0);
  release?.();
  await expect(change).rejects.toThrow(/bad_state/);
  expect(getStoredBot(db, bot.id)).toMatchObject({ provider: "claude", model: "haiku" });
  await waitFor(() => sessions[0]?.session.sent.length);
  expect(sessions[0].botId).toBe(bot.id);
  expect(vi.mocked(protectCodexDataAsync)).toHaveBeenCalledTimes(4);
});

it("caches setup.status ACL results for 60 seconds", async () => {
  const { client } = await harness();
  expect(vi.mocked(protectCodexData)).toHaveBeenCalledOnce();
  const initial = vi.mocked(protectCodexDataAsync).mock.calls.length;
  expect(initial).toBe(1);
  await client.call("setup.status", {});
  await client.call("setup.status", {});
  expect(vi.mocked(protectCodexDataAsync)).toHaveBeenCalledTimes(initial);
  const now = Date.now();
  vi.spyOn(Date, "now").mockReturnValue(now + 61_000);
  await client.call("setup.status", {});
  expect(vi.mocked(protectCodexDataAsync)).toHaveBeenCalledTimes(initial + 1);
});

it("logs Codex CLI drift from the measured version once", async () => {
  const different = setup().map((entry) => entry.provider === "codex" ? { ...entry, version: "0.157.1" } : entry);
  vi.mocked(checkSetup).mockResolvedValue(different as ProviderSetup[]);
  const info = vi.spyOn(console, "info").mockImplementation(() => {});
  const { client } = await harness();
  await client.call("setup.status", {});
  await client.call("setup.status", {});
  expect(info.mock.calls.filter((call) => String(call[0]).includes("differs from the measured ACL recipe"))).toHaveLength(1);
});

it("logs a rejected background setup refresh from the sign-in callback", async () => {
  const { client } = await harness();
  const failure = new Error("Setup unavailable");
  const log = vi.spyOn(console, "error").mockImplementation(() => {});
  vi.mocked(checkSetup).mockRejectedValueOnce(failure);
  vi.mocked(signIn).mockImplementation(async (_db, _provider, onDone) => {
    onDone(true, "a1");
    return { url: null, attempt: "a1" };
  });
  await client.call("setup.signIn", { provider: "claude" });
  await waitFor(() => log.mock.calls.length);
  expect(log).toHaveBeenCalledWith("Failed to refresh provider setup.", failure);
  // The sign-in still ended, so a card waiting on it stops waiting.
  await waitFor(() => client.events.some((event) => event.event === "setup.signInEnded" && event.data.provider === "claude"));
});

it("says a sign-in ended once, after the fresh setup, even when it changed nothing", async () => {
  const { client } = await harness();
  let done: ((ok: boolean, attempt: string) => void) | undefined;
  vi.mocked(signIn).mockImplementation(async (_db, _provider, onDone) => {
    done = onDone;
    return { url: "https://auth.openai.com/sign-in", attempt: "a2" };
  });
  await client.call("setup.signIn", { provider: "codex" });
  const from = client.events.length;

  done?.(false, "a2"); // the login failed: the fresh setup reads as it did before
  await waitFor(() => client.events.slice(from).some((event) => event.event === "setup.signInEnded"));
  const setupEvents = client.events.slice(from).filter((event) => event.event === "setup.changed" || event.event === "setup.signInEnded");
  expect(setupEvents.map((event) => event.event)).toEqual(["setup.changed", "setup.signInEnded"]);
  expect(setupEvents[1]).toEqual({ type: "event", event: "setup.signInEnded", data: { provider: "codex", attempt: "a2" } });
});

it("names the sign-in that ended, so a late end of an old attempt can't end a newer one", async () => {
  const { client } = await harness();
  const done: ((ok: boolean, attempt: string) => void)[] = [];
  vi.mocked(signIn).mockImplementation(async (_db, _provider, onDone) => {
    done.push(onDone);
    return { url: null, attempt: `attempt-${done.length}` };
  });
  const first = await client.call("setup.signIn", { provider: "claude" });
  const second = await client.call("setup.signIn", { provider: "claude" });
  expect([first.attempt, second.attempt]).toEqual(["attempt-1", "attempt-2"]);
  const from = client.events.length;
  done[0]?.(false, first.attempt); // the old sign-in ends while the new one is open
  await waitFor(() => client.events.slice(from).some((event) => event.event === "setup.signInEnded"));
  const ended = client.events.slice(from).filter((event) => event.event === "setup.signInEnded");
  expect(ended.map((event) => event.data)).toEqual([{ provider: "claude", attempt: "attempt-1" }]);
});

it("checks setup once and broadcasts the fresh result when sign-in ends", async () => {
  const { client } = await harness();
  let done: ((ok: boolean, attempt: string) => void) | undefined;
  vi.mocked(signIn).mockImplementation(async (_db, _provider, onDone) => {
    done = onDone;
    return { url: null, attempt: "a3" };
  });
  await client.call("setup.signIn", { provider: "claude" });
  const checksBefore = vi.mocked(checkSetup).mock.calls.length;
  const eventsBefore = client.events.filter((event) => event.event === "setup.changed").length;
  const fresh = setup(true).map((entry) => entry.provider === "claude" ? { ...entry, account: "new@example.com" } : entry);
  vi.mocked(checkSetup).mockResolvedValue(fresh);

  done?.(false, "a3");
  await waitFor(() => client.events.filter((event) => event.event === "setup.changed").length === eventsBefore + 1);
  expect(checkSetup).toHaveBeenCalledTimes(checksBefore + 1);
  expect(client.events.filter((event) => event.event === "setup.changed").at(-1)?.data).toEqual(fresh);
});

it("awaits browser launch errors and rejects showing a disabled browser", async () => {
  const { client } = await harness();
  const off = await client.call("bots.create", input("Off"));
  await expect(client.call("bots.showBrowser", { id: off.id })).rejects.toThrow(/bad_params/);
  const on = await client.call("bots.create", input("On", { browser: true }));
  vi.mocked(showBrowser).mockRejectedValueOnce(new Error("Browser could not start"));
  await expect(client.call("bots.showBrowser", { id: on.id })).rejects.toThrow(/Browser could not start/);
  await expect(client.call("bots.showBrowser", { id: on.id })).resolves.toEqual({});
});

it("still closes the browser and publishes the disabled setting if MCP teardown fails", async () => {
  const { client } = await harness();
  const bot = await client.call("bots.create", input("Disable browser", { browser: true }));
  vi.spyOn(BrowserMcp.prototype, "close").mockRejectedValueOnce(new Error("MCP close failed"));
  const closeBrowser = vi.spyOn(browsers, "close").mockResolvedValueOnce(undefined);
  await expect(client.call("bots.update", { id: bot.id, patch: { browser: false } })).rejects.toThrow(/browser was disabled/);
  expect(closeBrowser).toHaveBeenCalledWith(bot.id);
  expect((await client.call("bots.list", {})).find((entry) => entry.id === bot.id)?.browser).toBe(false);
  expect(client.events.some((event) => event.event === "bot.updated" && event.data.id === bot.id && !event.data.browser)).toBe(true);
});

it("closes the server and database after browser cleanup failures and reports all errors", async () => {
  const originalOpen = dbModule.openDb;
  const opened: Db[] = [];
  vi.spyOn(dbModule, "openDb").mockImplementation((...args) => {
    const db = originalOpen(...args);
    opened.push(db);
    return db;
  });
  const h = await harness();
  const daemonDb = opened[1]; // The harness opens its settings DB before startDaemon opens the daemon DB.
  if (!daemonDb) throw new Error("The daemon database was not tracked.");
  await h.client.close();
  clients.splice(clients.indexOf(h.client), 1);
  const dbClose = vi.spyOn(daemonDb, "close");
  const stop = vi.spyOn(h.daemon.scheduler, "stop");
  const shutdown = vi.spyOn(h.daemon.runner, "shutdown");
  const mcpError = new Error("MCP cleanup failed");
  const browserError = new Error("Browser cleanup failed");
  const mcpClose = vi.spyOn(BrowserMcp.prototype, "closeAll").mockRejectedValueOnce(mcpError);
  const browserClose = vi.spyOn(browsers, "closeAll").mockRejectedValueOnce(browserError);
  try {
    await expect(h.daemon.close()).rejects.toMatchObject({ name: "AggregateError", errors: [mcpError, browserError] });
  } finally { daemons.splice(daemons.indexOf(h.daemon), 1); }
  expect(stop).toHaveBeenCalledOnce();
  expect(shutdown).toHaveBeenCalledOnce();
  expect(mcpClose).toHaveBeenCalledOnce();
  expect(browserClose).toHaveBeenCalledOnce();
  expect(dbClose).toHaveBeenCalledOnce();
  expect(() => daemonDb.prepare("SELECT 1")).toThrow();
  const rebound = await serverModule.startServer({ port: h.daemon.port, token: "", handlers: {}, allowedOrigins: [] });
  await rebound.close();
});

it("reads the start of a bot's notes.md for Bot settings", async () => {
  const { client } = await harness();
  const bot = await client.call("bots.create", input("Noter"));
  expect(await client.call("bots.notes", { id: bot.id })).toEqual({ text: "# Noter's notes\n" });
  writeFileSync(join(bot.homeFolder, "notes.md"), "x".repeat(5000));
  expect((await client.call("bots.notes", { id: bot.id })).text).toHaveLength(4000);
  rmSync(join(bot.homeFolder, "notes.md"));
  expect(await client.call("bots.notes", { id: bot.id })).toEqual({ text: "" });
  await expect(client.call("bots.notes", { id: "missing" })).rejects.toThrow(/not_found/);
});


it("keeps Google credentials private and gates read-only MCP access per existing bot", async () => {
  let saved: GoogleVaultRecord | null = {
    version: 1, client: { clientId: "fixture.apps.googleusercontent.com", clientSecret: "synthetic-client-secret" },
    connection: { refreshToken: "synthetic-refresh-token", subject: "fixture-account", email: "fixture@example.test", scopes: ["openid", "email", "https://www.googleapis.com/auth/gmail.readonly"], services: ["gmail"] },
    grantedBotIds: [],
  };
  const vault: GoogleVault = { load: async () => structuredClone(saved), save: async (next) => { saved = structuredClone(next); }, clear: async () => { saved = null; } };
  const transport = vi.fn(async (req: { url: string }) => req.url.endsWith("/token")
    ? { status: 200, body: { access_token: "synthetic-access-token", token_type: "Bearer", expires_in: 3600 } }
    : { status: 200, body: { messages: [{ id: "abc123" }] } });
  const googleAuth = await GoogleAuth.open({ vault, transport });
  const { client, sessions } = await harness(tempRoot(), { mode: "development" }, googleAuth);
  const status = await client.call("google.status", {});
  expect(status).toMatchObject({ connected: true, grantedBotIds: [] });
  expect(JSON.stringify(status)).not.toContain("synthetic-");
  await expect(client.call("google.grant", { botId: "missing", enabled: true })).rejects.toThrow();
  const bot = await client.call("bots.create", input("Google fixture"));
  await client.call("chat.send", { botId: bot.id, text: "Synthetic message", files: [] });
  await waitFor(() => sessions.length === 1);
  const descriptor = appServer(sessions[0].options);
  const denied = await fetch(descriptor.url, { method: "POST", headers: { ...descriptor.headers, "Content-Type": "application/json" }, body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "tools/call", params: { name: "google_gmail_search", arguments: { query: "in:inbox" } } }) });
  expect(await denied.json()).toMatchObject({ result: { isError: true } });
  expect(transport).not.toHaveBeenCalled();
  await client.call("google.grant", { botId: bot.id, enabled: true });
  const allowed = await tool(sessions[0].options, "google_gmail_search", { query: "in:inbox" });
  expect(JSON.stringify(allowed)).toContain("abc123");
  expect(JSON.stringify(allowed)).not.toContain("synthetic-access-token");
  await client.call("bots.delete", { id: bot.id });
  expect((await client.call("google.status", {})).grantedBotIds).toEqual([]);
  expect(saved?.grantedBotIds).toEqual([]);
});
