import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { connect } from "@tjommie/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { daemonCommand, daemonEnv, daemonExecutable, daemonLaunchEnv, listeningPort, newToken, packagedDaemonPath, startDaemonProcess, stopWithin, type DaemonLaunch, type RunningDaemon } from "./daemon-process.ts";

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

  it("launches the desktop daemon on the explicit desktop data root", () => {
    expect(daemonLaunchEnv({ TJOMMIE_HOME: "C:\\owner", PATH: "test" }, "C:\\disposable", true)).toEqual({
      TJOMMIE_HOME: "C:\\disposable", PATH: "test", TJOMMIE_PORT: "0",
    });
  });

  it("rejects a daemon stop that never settles so a later Quit can proceed", async () => {
    vi.useFakeTimers();
    try {
      const stopped = stopWithin(new Promise<void>(() => {}), 20_000);
      const failed = expect(stopped).rejects.toThrow("did not stop in time");
      await vi.advanceTimersByTimeAsync(20_000);
      await failed;
    } finally {
      vi.useRealTimers();
    }
  });

  it("starts the exe named by TJOMMIE_DAEMON_EXE in unpackaged runs, with no Node arguments", () => {
    const exe = "C:\\tjommie-rs\\tjommie-daemon.exe";
    const node = { execPath: "C:\\node.exe", args: ["--disable-warning=ExperimentalWarning", "C:\\daemon\\main.ts"], runAsNode: true };
    const command = (env: NodeJS.ProcessEnv) => daemonCommand(env, null, node.execPath, "C:\\daemon\\main.ts");
    expect(command({})).toEqual(node);
    expect(command({ TJOMMIE_DAEMON_EXE: exe })).toEqual({ execPath: exe, args: [], runAsNode: false });
    expect(command({ TJOMMIE_DAEMON_EXE: "" })).toEqual(node);
  });

  it("runs the bundled Rust daemon in an installed app, with no Node arguments and whatever the environment says", () => {
    const resources = "C:\\Users\\owner\\AppData\\Local\\Programs\\Tjommie\\resources";
    const bundled = packagedDaemonPath(resources);
    expect(bundled).toBe(join(resources, "daemon", "tjommie-daemon.exe"));
    const command = (env: NodeJS.ProcessEnv) => daemonCommand(env, bundled, "C:\\Tjommie.exe", "C:\\daemon\\main.ts");
    expect(command({})).toEqual({ execPath: bundled, args: [], runAsNode: false });
    // An installed app never runs an override, and never the Node daemon.
    expect(command({ TJOMMIE_DAEMON_EXE: "C:\\elsewhere\\daemon.exe" })).toEqual({ execPath: bundled, args: [], runAsNode: false });
  });

  it("sets ELECTRON_RUN_AS_NODE only for the Node daemon, and never lets the Rust exe inherit it", () => {
    const inherited = { TJOMMIE_HOME: "C:\\x", ELECTRON_RUN_AS_NODE: "1", TJOMMIE_DEV: "1" };
    expect(daemonEnv(inherited, undefined, true)).toEqual({ TJOMMIE_HOME: "C:\\x", ELECTRON_RUN_AS_NODE: "1" });
    expect(daemonEnv(inherited, undefined, false)).toEqual({ TJOMMIE_HOME: "C:\\x" });
    expect(daemonEnv({ electron_run_as_node: "1" }, undefined, false)).toEqual({}); // Whatever its case.
    expect(daemonEnv(inherited, "C:\\disposable", false)).toEqual({ TJOMMIE_HOME: "C:\\x", TJOMMIE_ACCOUNTS_DIR: "C:\\disposable" });
  });

  it("names the process that owns the daemon's port for an installed app, an exe-hosted source run and the Node daemon", () => {
    const app = "C:\\Tjommie\\Tjommie.exe";
    expect(daemonExecutable(app, true, {})).toBe(join("C:\\Tjommie", "resources", "daemon", "tjommie-daemon.exe"));
    // The override is for source runs only; an installed app ignores it, as daemonCommand does.
    expect(daemonExecutable(app, true, { TJOMMIE_DAEMON_EXE: "C:\\elsewhere\\daemon.exe" })).toBe(join("C:\\Tjommie", "resources", "daemon", "tjommie-daemon.exe"));
    expect(daemonExecutable("C:\\electron.exe", false, { TJOMMIE_DAEMON_EXE: "C:\\rs\\tjommie-daemon.exe" })).toBe(resolve("C:\\rs\\tjommie-daemon.exe"));
    expect(daemonExecutable("C:\\electron.exe", false, {})).toBe("C:\\electron.exe");
    expect(daemonExecutable("C:\\electron.exe", false, { TJOMMIE_DAEMON_EXE: "" })).toBe("C:\\electron.exe");
  });

  it("keeps an inherited TJOMMIE_PORT next to TJOMMIE_HOME, so a second install sets both and never contends for 47100", () => {
    expect(daemonLaunchEnv({ TJOMMIE_PORT: "47200", TJOMMIE_LAN_PORT: "47201" }, "C:\\Users\\owner\\AppData\\Local\\Tjommie-rs")).toEqual({
      TJOMMIE_PORT: "47200", TJOMMIE_LAN_PORT: "47201", TJOMMIE_HOME: "C:\\Users\\owner\\AppData\\Local\\Tjommie-rs",
    });
    // A disposable test root always gets a free port, whatever was inherited.
    expect(daemonLaunchEnv({ TJOMMIE_PORT: "47200" }, "C:\\disposable", true).TJOMMIE_PORT).toBe("0");
  });

  // Build the Rust daemon and set TJOMMIE_DAEMON_EXE to run this against it.
  it.runIf(process.env.TJOMMIE_DAEMON_EXE)("runs the TJOMMIE_DAEMON_EXE daemon with the same token-stdin contract", async () => {
    let output = "";
    const daemon = await startDaemonProcess({ ...launch(), ...daemonCommand(process.env, null, process.execPath, entry) }, (text) => { output += text; });
    running.push(daemon);
    const { port, token } = daemon.connection;
    const client = await connect(port, token);
    expect(output).toContain(`listening on http://127.0.0.1:${port}`);
    expect(output).not.toContain(token);
    expect(output).not.toContain(daemon.desktopHostToken);
    await daemon.stop();
    expect((await daemon.exited).code).toBe(0);
    await client.disconnected;
  }, 60_000);

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
    expect(daemon.desktopHostToken).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(daemon.desktopHostToken).not.toBe(token);
    expect(daemon.connection).not.toHaveProperty("desktopHostToken");
    const client = await connect(port, token);
    expect(await client.call("bots.list", {})).toEqual([]);
    expect(output).toContain(`listening on http://127.0.0.1:${port}`);
    expect(output).not.toContain(token);
    expect(output).not.toContain(daemon.desktopHostToken);
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
