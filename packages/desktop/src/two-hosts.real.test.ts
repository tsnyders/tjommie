import { execFileSync, type ChildProcessWithoutNullStreams } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
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
import { startLocalRelay, stopLocalRelay } from "../../relay/src/local-relay.test-helper.ts";

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
    const started = await startLocalRelay(relayRoot, { keyPath: join(relayRoot, "relay.key"), certPath: cert });
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
    if (relay) await stopLocalRelay(relay).catch((error: unknown) => failures.push(error));
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
