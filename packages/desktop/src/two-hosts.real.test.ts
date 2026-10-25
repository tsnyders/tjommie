import { execFileSync, spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { get } from "node:https";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { getCACertificates, setDefaultCACertificates } from "node:tls";
import { expect, it, vi } from "vitest";
import { parsePairingCode } from "@tjommie/protocol";
import { connect, type Client } from "@tjommie/client";
import type { CredentialTransport } from "../../daemon/src/account-credentials.ts";
import { startDaemon, type Daemon } from "../../daemon/src/daemon.ts";
import { createDeviceKeys } from "./device-keys.ts";
import { newToken } from "./daemon-process.ts";
import { closeRealApp, eventually, launchRealApp, openChat, send, type RealApp } from "./real-app.ts";

const repo = resolve(import.meta.dirname, "../../..");

function certificate(root: string): string {
  const gitCore = execFileSync("git", ["--exec-path"], { encoding: "utf8", windowsHide: true }).trim();
  const openssl = resolve(gitCore, "../../bin/openssl.exe");
  if (!existsSync(openssl)) throw new Error("Git's OpenSSL is required for the local relay test.");
  const cert = join(root, "relay.crt");
  execFileSync(openssl, ["req", "-x509", "-newkey", "rsa:2048", "-sha256", "-nodes", "-days", "1",
    "-keyout", join(root, "relay.key"), "-out", cert, "-subj", "/CN=localhost",
    "-addext", "subjectAltName=IP:127.0.0.1,DNS:localhost"], { windowsHide: true, stdio: "ignore" });
  return cert;
}

function healthy(base: string, cert: string): Promise<boolean> {
  return new Promise((done) => {
    const request = get(`${base}/v1/health`, { ca: readFileSync(cert) }, (response) => {
      response.resume();
      done(response.statusCode === 200);
    });
    request.setTimeout(2_000, () => request.destroy());
    request.once("error", () => done(false));
  });
}

async function startRelay(root: string, cert: string): Promise<{ child: ChildProcessWithoutNullStreams; base: string }> {
  const child = spawn(process.execPath, [join(repo, "node_modules/wrangler/bin/wrangler.js"), "dev", "--local",
    "--port", "0", "--local-protocol", "https", "--https-key-path", join(root, "relay.key"),
    "--https-cert-path", cert, "--persist-to", root, "--config", "packages/relay/wrangler.toml"], {
    cwd: repo, windowsHide: true, stdio: ["pipe", "pipe", "pipe"],
    env: { PATH: process.env.PATH ?? "", SystemRoot: process.env.SystemRoot ?? "C:\\Windows", WINDIR: process.env.SystemRoot ?? "C:\\Windows",
      TEMP: tmpdir(), TMP: tmpdir(), USERPROFILE: root, APPDATA: root, LOCALAPPDATA: root, HOME: root,
      CI: "true", WRANGLER_SEND_METRICS: "false", XDG_CONFIG_HOME: root },
  });
  let output = "";
  const collect = (chunk: Buffer): void => { output = (output + chunk.toString()).slice(-4_000); };
  child.stdout.on("data", collect);
  child.stderr.on("data", collect);
  try {
    const base = await eventually(async () => {
      const port = /https:\/\/(?:127\.0\.0\.1|localhost):(\d+)/.exec(output)?.[1];
      if (!port || port === "0") return undefined;
      const url = `https://127.0.0.1:${port}`;
      return await healthy(url, cert) ? url : undefined;
    }, 30_000);
    return { child, base };
  } catch {
    await stopRelay(child).catch(() => {});
    throw new Error(`Local HTTPS relay did not start: ${output.slice(-1_000)}`);
  }
}

async function stopRelay(child: ChildProcessWithoutNullStreams): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return;
  child.stdin.end("q\n");
  const exited = await Promise.race([
    new Promise<boolean>((done) => child.once("exit", () => done(true))),
    new Promise<false>((done) => setTimeout(() => done(false), 3_000)),
  ]);
  if (!exited && child.pid) {
    try { execFileSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { windowsHide: true, stdio: "ignore" }); }
    catch { if (child.exitCode === null && child.signalCode === null) throw new Error("The local relay did not stop."); }
  }
}

it("pairs two isolated desktop profiles through the relay, chats without a provider, switches hosts and removes the device", async () => {
  const relayRoot = mkdtempSync(join(tmpdir(), "tjommie-two-hosts-relay-"));
  const aRoot = mkdtempSync(join(tmpdir(), "tjommie-two-hosts-a-"));
  let relay: ChildProcessWithoutNullStreams | undefined;
  let aDaemon: Daemon | undefined, aClient: Client | undefined, b: RealApp | undefined, hostId: string | undefined;
  const oldCa = process.env.NODE_EXTRA_CA_CERTS;
  const oldCertificates = getCACertificates("default");
  const failures: unknown[] = [];
  try {
    const cert = certificate(relayRoot);
    const started = await startRelay(relayRoot, cert);
    relay = started.child;
    process.env.NODE_EXTRA_CA_CERTS = cert;
    setDefaultCACertificates([...oldCertificates, readFileSync(cert, "utf8")]);
    const records = new Map<string, string>();
    const credentials: CredentialTransport = async ({ operation, target, key }) => {
      if (operation === "write" && key !== undefined) records.set(target, key);
      if (operation === "delete") records.delete(target);
      return operation === "read" ? records.get(target) ?? null : null;
    };
    aDaemon = await startDaemon({ root: aRoot, port: 0, accountsDir: join(aRoot, "accounts"),
      auth: { mode: "production", token: newToken() },
      remote: { port: 0, bind: "127.0.0.1", addresses: () => ["192.0.2.1"], credentials },
      createSession: () => { throw new Error("A paused synthetic bot must never start a provider session."); } });
    aClient = await connect(aDaemon.port, aDaemon.token);
    b = await launchRealApp(undefined, { isolatedAccounts: true });

    await aClient.call("settings.set", { relayUrl: started.base });
    const bot = await aClient.call("bots.create", { name: "Remote fixture", provider: "claude", model: "haiku",
      color: "blue", shape: "circle", instructions: "Synthetic paused fixture; never run a provider turn.", autonomy: "ask", browser: true });
    await aClient.call("bots.pause", { id: bot.id, paused: true });
    const invite = await aClient.call("pairing.create", {});
    const code = parsePairingCode(invite.code);
    hostId = code.hostId;
    expect(code.relay).toBe(started.base);
    expect(code.lan).toHaveLength(1);
    expect(code.lan[0]?.host).toBe("192.0.2.1");

    await b.page.getByRole("button", { name: /Switch PC, now on This PC/ }).click();
    await b.page.getByRole("menuitem", { name: /Connect to another computer/ }).click();
    await b.page.getByRole("textbox", { name: "Pairing code" }).fill(invite.code);
    await b.page.getByRole("textbox", { name: "Name for this PC" }).fill("Second PC");
    await b.page.getByRole("button", { name: "Connect", exact: true }).click();
    await b.page.getByRole("button", { name: `Switch PC, now on ${code.hostName}` }).waitFor({ timeout: 30_000 });
    await openChat(b.page, bot.name);
    await send(b.page, bot.name, "Message from second PC");
    await vi.waitFor(async () => expect((await aClient!.call("chat.history", { botId: bot.id, limit: 50 })).messages)
      .toEqual(expect.arrayContaining([expect.objectContaining({ body: "Message from second PC", author: "you" })])), { timeout: 10_000 });
    await b.page.getByText(`Runs on ${code.hostName}.`, { exact: false }).waitFor();

    await b.page.getByRole("button", { name: `Switch PC, now on ${code.hostName}` }).click();
    await b.page.getByRole("menuitemradio", { name: "This PC" }).click();
    await b.page.getByRole("button", { name: /Switch PC, now on This PC/ }).waitFor();
    await b.page.getByRole("heading", { name: "Set up Tjommie" }).waitFor();
    await b.page.getByRole("button", { name: /Switch PC, now on This PC/ }).click();
    await b.page.getByRole("menuitemradio", { name: new RegExp(code.hostName) }).click();
    await openChat(b.page, bot.name);

    const device = (await aClient.call("devices.list", {})).find((entry) => entry.name === "Second PC");
    if (!device) throw new Error("The second PC was not registered on the host.");
    const revokedAt = Date.now();
    await aClient.call("devices.revoke", { id: device.id });
    await b.page.getByRole("heading", { name: "This PC removed this device" }).waitFor({ timeout: 2_000 });
    expect(Date.now() - revokedAt).toBeLessThan(2_000);
  } catch (error) {
    failures.push(error);
  } finally {
    if (oldCa === undefined) delete process.env.NODE_EXTRA_CA_CERTS;
    else process.env.NODE_EXTRA_CA_CERTS = oldCa;
    setDefaultCACertificates(oldCertificates);
    if (b) {
      const keys = createDeviceKeys(b.root);
      await closeRealApp(b, false).catch((error: unknown) => failures.push(error));
      if (hostId) {
        await keys.delete(`identity-${hostId}`).catch((error: unknown) => failures.push(error));
        await keys.delete(`identity-${hostId}.pending`).catch((error: unknown) => failures.push(error));
      }
    }
    if (aClient) await aClient.close().catch((error: unknown) => failures.push(error));
    if (aDaemon) await aDaemon.close().catch((error: unknown) => failures.push(error));
    if (relay) await stopRelay(relay).catch((error: unknown) => failures.push(error));
    for (const root of [relayRoot, aRoot]) {
      if (dirname(resolve(root)) !== resolve(tmpdir())) failures.push(new Error("Refusing to remove a root outside the temp directory."));
      else await eventually(async () => {
        try { rmSync(root, { recursive: true, force: true }); return true; }
        catch { return undefined; }
      }, 10_000).catch((error: unknown) => failures.push(error));
    }
  }
  if (failures.length) throw new AggregateError(failures, "Two-host integration or cleanup failed.");
});
