import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { connect } from "@tjommie/client";
import { afterEach, describe, expect, it } from "vitest";
import { daemonEnv, listeningPort, newToken, startDaemonProcess, type DaemonLaunch, type RunningDaemon } from "./daemon-process.ts";

const entry = fileURLToPath(new URL("../../daemon/src/main.ts", import.meta.url));
const roots: string[] = [];
const running: RunningDaemon[] = [];

afterEach(async () => {
  for (const daemon of running.splice(0)) await daemon.stop();
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
});

// The daemon from source under this test's Node, on a disposable data root and a free port. TJOMMIE_DEV is set
// on purpose: daemonEnv must remove it, or the daemon refuses to mix development and production modes. PATH is
// emptied, whatever its case, so the daemon finds no CLIs and starts fast (as in the daemon's main.test.ts).
function launch(extraEnv: Record<string, string> = {}): DaemonLaunch {
  const root = mkdtempSync(join(tmpdir(), "tjommie desktop ñ-"));
  roots.push(root);
  const inherited = Object.fromEntries(Object.entries(process.env).filter(([key]) => key.toUpperCase() !== "PATH"));
  return {
    execPath: process.execPath,
    args: ["--disable-warning=ExperimentalWarning", entry],
    env: { ...inherited, PATH: "", TJOMMIE_HOME: root, TJOMMIE_PORT: "0", TJOMMIE_DEV: "1", ...extraEnv },
  };
}

describe("daemon process", () => {
  it("makes fresh 43-character tokens and finds the port in the daemon's output", () => {
    expect(newToken()).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(newToken()).not.toBe(newToken());
    expect(listeningPort("Removed a leftover development daemon-token file.\nTjommie daemon listening on http://127.0.0.1:52311\n")).toBe(52311);
    expect(listeningPort("starting")).toBeNull();
  });

  it("runs the daemon in production mode as plain Node", () => {
    expect(daemonEnv({ TJOMMIE_DEV: "1", TJOMMIE_HOME: "C:\\x" })).toEqual({ TJOMMIE_HOME: "C:\\x", ELECTRON_RUN_AS_NODE: "1" });
  });

  it("does not forward an inherited accounts-folder hook", () => {
    const parentEnv = { ...process.env, TJOMMIE_ACCOUNTS_DIR: "C:\\private-accounts" };
    expect(daemonEnv(parentEnv).TJOMMIE_ACCOUNTS_DIR).toBeUndefined();
    expect(daemonEnv(parentEnv, "C:\\disposable-accounts").TJOMMIE_ACCOUNTS_DIR).toBe("C:\\disposable-accounts");
  });

  it("pipes the token in, keeps it out of output, and stops the daemon by closing stdin (token stays in memory)", async () => {
    const options = launch();
    let output = "";
    const daemon = await startDaemonProcess(options, (text) => { output += text; });
    running.push(daemon);
    const { port, token } = daemon.connection;
    const client = await connect(port, token);
    expect(await client.call("bots.list", {})).toEqual([]);
    expect(output).toContain(`listening on http://127.0.0.1:${port}`);
    expect(output).not.toContain(token);
    await daemon.stop();
    expect((await daemon.exited).code).toBe(0);
    await client.disconnected;
  }, 60_000);

  it("reports the daemon's own reason when it can't start", async () => {
    const first = await startDaemonProcess(launch());
    running.push(first);
    const port = first.connection.port;
    await expect(startDaemonProcess(launch({ TJOMMIE_PORT: String(port) }))).rejects.toThrow(`Tjommie is already running: port ${port} is in use.`);
  }, 60_000);
});
