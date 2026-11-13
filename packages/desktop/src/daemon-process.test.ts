import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { connect } from "@tjommie/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { credentialNamespace } from "./credential-transport.ts";
import { BUILD_RUST_DAEMON, DEV_LAN_PORT, DEV_PORT, daemonCommand, daemonEnv, daemonExecutable, daemonLaunchEnv, dataRootFor, developmentDaemonMessage, listeningPort, newToken, packagedDaemonPath, sourceDaemonPath, startDaemonProcess, stopWithin, userDataFor, type DaemonLaunch, type RunningDaemon } from "./daemon-process.ts";

const repoRoot = resolve(import.meta.dirname, "..", "..", "..");
// The Rust daemon the process tests start: TJOMMIE_DAEMON_EXE, else the debug build. Build it with
// `cargo build -p daemon --bin tjommie-daemon` inside tjommie-rs. Without it those tests are skipped, not passed.
const exe = sourceDaemonPath(process.env, repoRoot);
const built = existsSync(exe);
const roots: string[] = [];
const running: RunningDaemon[] = [];

afterEach(async () => {
  for (const daemon of running.splice(0)) await daemon.stop();
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
});

// The Rust daemon on a disposable data root and a free port, with the same environment filter the app applies.
// TJOMMIE_DEV is set on purpose: daemonEnv must remove it, or the daemon refuses to mix development and production
// modes. The phone listener stays on loopback with a free port, so no Windows Firewall prompt and no clash with an
// installed Tjommie. PATH is emptied, whatever its case, so the daemon finds no provider CLIs and starts fast.
function launch(extraEnv: Record<string, string> = {}): DaemonLaunch {
  const root = mkdtempSync(join(tmpdir(), "tjommie desktop ñ-"));
  roots.push(root);
  const inherited = Object.fromEntries(Object.entries(process.env).filter(([key]) => key.toUpperCase() !== "PATH"));
  return {
    ...daemonCommand(process.env, null, repoRoot),
    env: { ...inherited, PATH: "", TJOMMIE_HOME: root, TJOMMIE_PORT: "0", TJOMMIE_LAN_BIND: "127.0.0.1", TJOMMIE_LAN_PORT: "0", TJOMMIE_DEV: "1", ...extraEnv },
  };
}

describe("daemon process", () => {
  it("makes fresh 43-character tokens and finds the port in the daemon's output", () => {
    expect(newToken()).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(newToken()).not.toBe(newToken());
    expect(listeningPort("Removed a leftover development daemon-token file.\nTjommie daemon listening on http://127.0.0.1:52311\n")).toBe(52311);
    expect(listeningPort("starting")).toBeNull();
  });

  it("runs the daemon in production mode: no development switch, and no Node environment", () => {
    expect(daemonEnv({ TJOMMIE_DEV: "1", TJOMMIE_HOME: "C:\\x" })).toEqual({ TJOMMIE_HOME: "C:\\x" });
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

  it("starts the exe named by TJOMMIE_DAEMON_EXE in unpackaged runs, else the debug build, with no Node arguments", () => {
    const override = "C:\\tjommie-rs\\tjommie-daemon.exe";
    const debugBuild = join(repoRoot, "tjommie-rs", "target", "debug", "tjommie-daemon.exe");
    const present = () => true;
    expect(daemonCommand({ TJOMMIE_DAEMON_EXE: override }, null, repoRoot, present)).toEqual({ execPath: override, args: [], runAsNode: false });
    expect(daemonCommand({}, null, repoRoot, present)).toEqual({ execPath: debugBuild, args: [], runAsNode: false });
    expect(daemonCommand({ TJOMMIE_DAEMON_EXE: "" }, null, repoRoot, present)).toEqual({ execPath: debugBuild, args: [], runAsNode: false });
    expect(sourceDaemonPath({}, repoRoot)).toBe(debugBuild);
  });

  it("never falls back to Node: a missing Rust daemon fails with the instruction to build it", () => {
    const missing = () => false;
    expect(() => daemonCommand({}, null, repoRoot, missing)).toThrow(BUILD_RUST_DAEMON);
    expect(BUILD_RUST_DAEMON).toBe("Build the Rust daemon first: cargo build -p daemon --bin tjommie-daemon (inside tjommie-rs)");
    expect(() => daemonCommand({ TJOMMIE_DAEMON_EXE: "" }, null, repoRoot, missing)).toThrow(BUILD_RUST_DAEMON);
    // An override that points nowhere is its own mistake: say which file, not how to build the default.
    expect(() => daemonCommand({ TJOMMIE_DAEMON_EXE: "C:\\rs\\tjommie-daemon.exe" }, null, repoRoot, missing)).toThrow(resolve("C:\\rs\\tjommie-daemon.exe"));
  });

  it("runs the bundled Rust daemon in an installed app, with no Node arguments and whatever the environment says", () => {
    const resources = "C:\\Users\\owner\\AppData\\Local\\Programs\\Tjommie\\resources";
    const bundled = packagedDaemonPath(resources);
    expect(bundled).toBe(join(resources, "daemon", "tjommie-daemon.exe"));
    const command = (env: NodeJS.ProcessEnv) => daemonCommand(env, bundled, repoRoot, () => false);
    expect(command({})).toEqual({ execPath: bundled, args: [], runAsNode: false });
    // An installed app never runs an override, and never looks for the debug build.
    expect(command({ TJOMMIE_DAEMON_EXE: "C:\\elsewhere\\daemon.exe" })).toEqual({ execPath: bundled, args: [], runAsNode: false });
  });

  it("never lets the Rust exe inherit ELECTRON_RUN_AS_NODE, and sets it only for 0.4.2's Node daemon when asked", () => {
    const inherited = { TJOMMIE_HOME: "C:\\x", ELECTRON_RUN_AS_NODE: "1", TJOMMIE_DEV: "1" };
    expect(daemonEnv(inherited)).toEqual({ TJOMMIE_HOME: "C:\\x" });
    expect(daemonEnv(inherited, undefined, false)).toEqual({ TJOMMIE_HOME: "C:\\x" });
    expect(daemonEnv({ electron_run_as_node: "1" })).toEqual({}); // Whatever its case.
    expect(daemonEnv(inherited, "C:\\disposable")).toEqual({ TJOMMIE_HOME: "C:\\x", TJOMMIE_ACCOUNTS_DIR: "C:\\disposable" });
    // scripts/r1-acceptance.ts seeds a data root with the 0.4.2 install's own Node daemon, which runs inside Electron.
    expect(daemonEnv(inherited, undefined, true)).toEqual({ TJOMMIE_HOME: "C:\\x", ELECTRON_RUN_AS_NODE: "1" });
  });

  it("names the process that owns the daemon's port for an installed app and a run from source", () => {
    const app = "C:\\Tjommie\\Tjommie.exe";
    const bundled = join("C:\\Tjommie", "resources", "daemon", "tjommie-daemon.exe");
    expect(daemonExecutable(app, true, {}, repoRoot)).toBe(bundled);
    // The override is for source runs only; an installed app ignores it, as daemonCommand does.
    expect(daemonExecutable(app, true, { TJOMMIE_DAEMON_EXE: "C:\\elsewhere\\daemon.exe" }, repoRoot)).toBe(bundled);
    expect(daemonExecutable("C:\\electron.exe", false, { TJOMMIE_DAEMON_EXE: "C:\\rs\\tjommie-daemon.exe" }, repoRoot)).toBe(resolve("C:\\rs\\tjommie-daemon.exe"));
    expect(daemonExecutable("C:\\electron.exe", false, {}, repoRoot)).toBe(join(repoRoot, "tjommie-rs", "target", "debug", "tjommie-daemon.exe"));
    expect(daemonExecutable("C:\\electron.exe", false, { TJOMMIE_DAEMON_EXE: "" }, repoRoot)).toBe(join(repoRoot, "tjommie-rs", "target", "debug", "tjommie-daemon.exe"));
  });

  it("keeps an inherited TJOMMIE_PORT next to TJOMMIE_HOME, so a second install sets both and never contends for 47100", () => {
    expect(daemonLaunchEnv({ TJOMMIE_PORT: "47300", TJOMMIE_LAN_PORT: "47301" }, "C:\\Users\\owner\\AppData\\Local\\Tjommie-two")).toEqual({
      TJOMMIE_PORT: "47300", TJOMMIE_LAN_PORT: "47301", TJOMMIE_HOME: "C:\\Users\\owner\\AppData\\Local\\Tjommie-two",
    });
    // A disposable test root always gets a free port, whatever was inherited.
    expect(daemonLaunchEnv({ TJOMMIE_PORT: "47200" }, "C:\\disposable", true).TJOMMIE_PORT).toBe("0");
  });

  describe("where an unpackaged run keeps its data and which ports it uses", () => {
    const local = "C:\\Users\\owner\\AppData\\Local";
    const installedFolder = join(local, "Tjommie");

    it("never resolves to the installed app's data folder with no environment, and an installed app does", () => {
      const unpackaged = dataRootFor({ LOCALAPPDATA: local }, false, "Tjommie");
      expect(unpackaged).toBe(join(local, "Tjommie-rs"));
      expect(unpackaged).not.toBe(installedFolder);
      // A different folder is a different Credential Manager namespace, so a run from source cannot reach the installed app's secrets either.
      expect(credentialNamespace(unpackaged)).not.toBe(credentialNamespace(installedFolder));
      expect(dataRootFor({ LOCALAPPDATA: local }, true, "Tjommie")).toBe(installedFolder);
      expect(credentialNamespace(dataRootFor({ LOCALAPPDATA: local }, true, "Tjommie"))).toBe(credentialNamespace(installedFolder));
      // Without LOCALAPPDATA the fallback is still the development folder, not the product folder.
      expect(dataRootFor({}, false, "Tjommie", undefined, "C:\\Users\\owner")).toBe(join("C:\\Users\\owner", ".local", "share", "Tjommie-rs"));
      expect(dataRootFor({}, true, "Tjommie", undefined, "C:\\Users\\owner")).toBe(join("C:\\Users\\owner", ".local", "share", "Tjommie"));
    });

    it("lets an explicit TJOMMIE_HOME win in both, and an isolated test build's own root win over the defaults", () => {
      for (const packaged of [false, true]) {
        expect(dataRootFor({ LOCALAPPDATA: local, TJOMMIE_HOME: "C:\\chosen" }, packaged, "Tjommie")).toBe("C:\\chosen");
        expect(dataRootFor({ LOCALAPPDATA: local, TJOMMIE_HOME: "C:\\chosen" }, packaged, "Tjommie", "C:\\test-root")).toBe("C:\\chosen");
        expect(dataRootFor({ LOCALAPPDATA: local }, packaged, "Tjommie Update Test", "C:\\test-root")).toBe("C:\\test-root");
      }
      expect(dataRootFor({ LOCALAPPDATA: local }, true, "Tjommie Update Test")).toBe(join(local, "Tjommie Update Test"));
    });

    it("gives a development run the debug ports when it inherits none, and an installed app nothing of its own", () => {
      expect(DEV_PORT).toBe("47200");
      expect(DEV_LAN_PORT).toBe("47201");
      const dev = "C:\\Users\\owner\\AppData\\Local\\Tjommie-rs";
      expect(daemonLaunchEnv({ PATH: "test" }, dev, false, true)).toEqual({ PATH: "test", TJOMMIE_HOME: dev, TJOMMIE_PORT: "47200", TJOMMIE_LAN_PORT: "47201" });
      // An installed app leaves the ports to the release daemon: 47100 and 47101.
      expect(daemonLaunchEnv({ PATH: "test" }, "C:\\Tjommie", false, false)).toEqual({ PATH: "test", TJOMMIE_HOME: "C:\\Tjommie" });
      // An inherited port wins, whatever its case, and a disposable test root still gets a free one.
      expect(daemonLaunchEnv({ TJOMMIE_PORT: "47300" }, dev, false, true)).toEqual({ TJOMMIE_PORT: "47300", TJOMMIE_LAN_PORT: "47201", TJOMMIE_HOME: dev });
      expect(daemonLaunchEnv({ tjommie_lan_port: "47301" }, dev, false, true)).toEqual({ tjommie_lan_port: "47301", TJOMMIE_PORT: "47200", TJOMMIE_HOME: dev });
      expect(daemonLaunchEnv({ TJOMMIE_PORT: "47300" }, "C:\\disposable", true, true).TJOMMIE_PORT).toBe("0");
    });

    it("asks only for what the daemon cannot guess when development mode finds no daemon", () => {
      const dev = join(local, "Tjommie-rs");
      const start = "Start the Rust daemon in development mode first: cargo run -p daemon --bin tjommie-daemon -- --dev (inside tjommie-rs)";
      expect(developmentDaemonMessage({}, dev, 47200)).toBe(`${start}.`);
      expect(developmentDaemonMessage({ TJOMMIE_PORT: "47300" }, dev, 47300)).toBe(`${start}, with the same TJOMMIE_PORT=47300 as this app.`);
      expect(developmentDaemonMessage({ TJOMMIE_HOME: "C:\\chosen", TJOMMIE_PORT: "47300" }, "C:\\chosen", 47300)).toBe(`${start}, with the same TJOMMIE_HOME=C:\\chosen and TJOMMIE_PORT=47300 as this app.`);
    });

    it("gives an unpackaged run an Electron profile of its own, never the installed app's, and an installed app Electron's default", () => {
      const roaming = "C:\\Users\\owner\\AppData\\Roaming";
      const installedProfile = join(roaming, "Tjommie");
      const devRoot = dataRootFor({ LOCALAPPDATA: local }, false, "Tjommie");
      const unpackaged = userDataFor({}, false, devRoot);
      expect(unpackaged).toBe(join(local, "Tjommie-rs", "electron"));
      expect(unpackaged).not.toBe(installedProfile);
      expect(unpackaged?.toLowerCase().startsWith(roaming.toLowerCase())).toBe(false);
      // null means no override at all: an installed app keeps Electron's default, %APPDATA%\Tjommie, as it always has.
      expect(userDataFor({}, true, installedFolder)).toBeNull();
      // An explicit TJOMMIE_USER_DATA wins in both (an empty one is no setting); an isolated test build keeps its own root's profile.
      for (const packaged of [false, true]) {
        expect(userDataFor({ TJOMMIE_USER_DATA: "C:\\profile" }, packaged, devRoot, "C:\\test-root")).toBe("C:\\profile");
        expect(userDataFor({ TJOMMIE_USER_DATA: "" }, packaged, devRoot)).toBe(packaged ? null : join(devRoot, "electron"));
      }
      expect(userDataFor({}, true, "C:\\test-root", "C:\\test-root")).toBe(join("C:\\test-root", "electron"));
      // A development run on an explicit data folder carries its profile with it.
      expect(userDataFor({}, false, dataRootFor({ TJOMMIE_HOME: "C:\\chosen" }, false, "Tjommie"))).toBe(join("C:\\chosen", "electron"));
    });

    // main.ts is the Electron entry and cannot be imported by a test, so check that it takes its folder, profile, port and launch
    // environment from the functions above instead of deriving its own, which is how a run from source reached the installed
    // app's data, and that the test-only switches stay keyed on the explicit TJOMMIE_USER_DATA, not on a derived profile.
    it("takes the data folder, profile, development port and launch environment from those functions in main.ts", () => {
      const main = readFileSync(resolve(import.meta.dirname, "main.ts"), "utf8").replaceAll("\r\n", "\n");
      expect(main).toContain("const dataRoot = dataRootFor(process.env, packaged, productName, packageIdentity.testDataRoot);");
      expect(main).toContain("const userData = userDataFor(process.env, packaged, dataRoot, packageIdentity.testDataRoot);");
      expect(main).toContain(`if (userData !== null) {\n  mkdirSync(userData, { recursive: true });\n  app.setPath("userData", userData);`);
      expect(main.match(/app\.setPath\("userData"/g)).toHaveLength(1);
      expect(main).toContain("daemonLaunchEnv(process.env, dataRoot, !!packageIdentity.testDataRoot, !packaged)");
      expect(main).toContain("Number(process.env.TJOMMIE_PORT ?? DEV_PORT)");
      expect(main).toContain("developmentDaemonMessage(process.env, dataRoot, port)");
      expect(main).not.toMatch(/LOCALAPPDATA|47100|47101/);
      // The login item, the updater switch and the account-folder hook follow an explicit TJOMMIE_USER_DATA only.
      expect(main).toContain("if (!process.env.TJOMMIE_USER_DATA && !packageIdentity.testDataRoot) {");
      expect(main).toContain(`TJOMMIE_UPDATES: (process.env.TJOMMIE_USER_DATA && APP_ID === "com.tsnyders.tjommie")`);
      expect(main).toContain("process.env.TJOMMIE_USER_DATA ? process.env.TJOMMIE_ACCOUNTS_DIR || undefined : undefined");
    });
  });

  it("does not forward an inherited test hook that turns off a check or a protection", () => {
    const parentEnv = {
      ...process.env,
      TJOMMIE_SKIP_FOLDER_PROTECTION: "1",
      tjommie_skip_account_check: "1", // Whatever its case.
      TJOMMIE_SKIP_SOMETHING_NEW: "1", // Every TJOMMIE_SKIP_* switch, not only the ones that exist today.
      TJOMMIE_HOME: "C:\\x",
    };
    for (const runAsNode of [true, false]) {
      const env = daemonEnv(parentEnv, undefined, runAsNode);
      expect(Object.keys(env).filter((key) => key.toUpperCase().startsWith("TJOMMIE_SKIP_"))).toEqual([]);
      expect(env.TJOMMIE_HOME).toBe("C:\\x"); // Other settings still pass through.
    }
  });

  it("does not forward an inherited accounts-folder hook", () => {
    const parentEnv = { ...process.env, TJOMMIE_ACCOUNTS_DIR: "C:\\private-accounts" };
    expect(daemonEnv(parentEnv).TJOMMIE_ACCOUNTS_DIR).toBeUndefined();
    expect(daemonEnv(parentEnv, "C:\\disposable-accounts").TJOMMIE_ACCOUNTS_DIR).toBe("C:\\disposable-accounts");
  });

  it.skipIf(!built)("pipes the token in, keeps it out of output, and stops the Rust daemon by closing stdin (token stays in memory)", async () => {
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

  it.skipIf(!built)("reports the daemon's own reason when it can't start", async () => {
    const first = await startDaemonProcess(launch());
    running.push(first);
    const port = first.connection.port;
    await expect(startDaemonProcess(launch({ TJOMMIE_PORT: String(port) }))).rejects.toThrow(`Tjommie is already running: port ${port} is in use.`);
  }, 60_000);
});
