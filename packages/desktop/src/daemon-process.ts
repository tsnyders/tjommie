import { execFileSync, spawn } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";

export type DaemonConnection = { port: number; token: string };
export type DaemonLaunch = {
  execPath: string;
  args: string[];
  env: NodeJS.ProcessEnv;
  testAccountsDir?: string;
  // False (the default): execPath is the Rust daemon's own exe, which must not inherit ELECTRON_RUN_AS_NODE. True only for
  // scripts/r1-acceptance.ts, which runs 0.4.2's installed Node daemon (Electron as Node) to seed a data root.
  runAsNode?: boolean;
};
export type RunningDaemon = {
  connection: DaemonConnection;
  desktopHostToken: string;
  pid: number | undefined;
  exited: Promise<{ code: number | null }>;
  stop: () => Promise<void>;
  forceStop: () => Promise<void>;
};

const LISTENING = /Tjommie daemon listening on http:\/\/127\.0\.0\.1:(\d+)/;
const STOP_GRACE_MS = 10_000;

// A fresh token for every daemon start: 32 random bytes, 43 base64url characters.
export const newToken = (): string => randomBytes(32).toString("base64url");

export function listeningPort(output: string): number | null {
  const match = LISTENING.exec(output);
  return match ? Number(match[1]) : null;
}

// Production mode cannot inherit development settings or the test-only hooks: the account-folder hook, and every
// TJOMMIE_SKIP_* switch (the Rust daemon's test hooks that turn off its account check and its Codex folder protection;
// a debug build honours them, and none of them may ride in from a PC's environment). The explicit testAccountsDir
// option is the only way a desktop test can send the account-folder hook to its disposable daemon. The Rust daemon gets no
// ELECTRON_RUN_AS_NODE and drops an inherited one; only 0.4.2's Node daemon, which runs as plain Node inside Electron, asks for it.
export function daemonEnv(base: NodeJS.ProcessEnv, testAccountsDir?: string, runAsNode = false): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = Object.fromEntries(
    Object.entries(base).filter(([key]) => {
      const name = key.toUpperCase();
      return !["TJOMMIE_DEV", "TJOMMIE_ACCOUNTS_DIR", "ELECTRON_RUN_AS_NODE"].includes(name) && !name.startsWith("TJOMMIE_SKIP_");
    }),
  );
  if (runAsNode) env.ELECTRON_RUN_AS_NODE = "1";
  if (testAccountsDir !== undefined) env.TJOMMIE_ACCOUNTS_DIR = testAccountsDir;
  return env;
}

// The Rust daemon ships as one exe in the installed app's resources, beside the UI and the icons.
export const packagedDaemonPath = (resourcesPath: string): string => join(resourcesPath, "daemon", "tjommie-daemon.exe");

export const BUILD_RUST_DAEMON = "Build the Rust daemon first: cargo build -p daemon --bin tjommie-daemon (inside tjommie-rs)";

// The Rust daemon an unpackaged run starts: the exe TJOMMIE_DAEMON_EXE names, else the debug build under the repo root.
export const sourceDaemonPath = (env: NodeJS.ProcessEnv, repoRoot: string): string =>
  env.TJOMMIE_DAEMON_EXE ? resolve(env.TJOMMIE_DAEMON_EXE) : join(repoRoot, "tjommie-rs", "target", "debug", "tjommie-daemon.exe");

// What starts the daemon: always the Rust exe, never Node. An installed app (`packagedExe` is its bundled exe) runs that
// exe whatever the environment says. An unpackaged run uses sourceDaemonPath and fails with a clear message when the exe
// is not there. The exe takes --token-stdin and no other arguments.
export function daemonCommand(env: NodeJS.ProcessEnv, packagedExe: string | null, repoRoot: string, exists: (path: string) => boolean = existsSync): Pick<DaemonLaunch, "execPath" | "args" | "runAsNode"> {
  if (packagedExe !== null) return { execPath: packagedExe, args: [], runAsNode: false };
  const exe = sourceDaemonPath(env, repoRoot);
  if (!exists(exe)) throw new Error(env.TJOMMIE_DAEMON_EXE ? `TJOMMIE_DAEMON_EXE names a daemon that is not there: ${exe}` : BUILD_RUST_DAEMON);
  return { execPath: exe, args: [], runAsNode: false };
}

// The executable of the process that listens on the daemon's port, for tests that must recognize their own daemon: the
// Rust daemon bundled beside an installed app, else the one a run from source starts (see sourceDaemonPath).
export function daemonExecutable(appExecutable: string, installed: boolean, env: NodeJS.ProcessEnv, repoRoot: string): string {
  return installed ? packagedDaemonPath(join(dirname(appExecutable), "resources")) : sourceDaemonPath(env, repoRoot);
}

// An unpackaged run is a development run. It keeps its data in the folder a debug Rust daemon uses (`Tjommie-rs`, APP_FOLDER
// in the daemon's paths.rs) and listens on the debug ports (startup.rs: 47200, and 47201 for the phone), so it never touches
// the installed app's `Tjommie` folder or ports 47100 and 47101. An installed app keeps `Tjommie` and the release defaults.
// An explicit TJOMMIE_HOME or TJOMMIE_PORT wins in both.
export const DEV_FOLDER = "Tjommie-rs";
export const DEV_PORT = "47200";
export const DEV_LAN_PORT = "47201";

// Where this run keeps its data: TJOMMIE_HOME, else an isolated test build's own root, else the installed app's product
// folder, or the development folder when unpackaged. A pure function so a test can prove which folder a run resolves to.
export function dataRootFor(env: NodeJS.ProcessEnv, packaged: boolean, productName: string, testDataRoot?: string, home: string = homedir()): string {
  return env.TJOMMIE_HOME ?? testDataRoot ?? join(env.LOCALAPPDATA ?? join(home, ".local", "share"), packaged ? productName : DEV_FOLDER);
}

// The Electron profile of this run (cookies, local storage, the single-instance lock): TJOMMIE_USER_DATA, else an isolated
// test build's, else, unpackaged, an `electron` folder in the development data folder, so a run from source has a profile and
// a lock of its own beside an installed Tjommie. null: an installed app keeps Electron's default, %APPDATA%\<product name>.
export function userDataFor(env: NodeJS.ProcessEnv, packaged: boolean, dataRoot: string, testDataRoot?: string): string | null {
  return env.TJOMMIE_USER_DATA || (testDataRoot || !packaged ? join(dataRoot, "electron") : null);
}

// TJOMMIE_HOME always follows the data root. TJOMMIE_PORT is only forced for a disposable test root; an inherited one
// passes through, so a second install sets both and never contends for port 47100. A development run, when it inherits no
// port, gets the development ports, whichever daemon exe (debug or release) it starts.
export function daemonLaunchEnv(base: NodeJS.ProcessEnv, dataRoot: string, testRoot = false, development = false): NodeJS.ProcessEnv {
  const inherited = (name: string): boolean => Object.keys(base).some((key) => key.toUpperCase() === name);
  return {
    ...(development && !inherited("TJOMMIE_PORT") ? { TJOMMIE_PORT: DEV_PORT } : {}),
    ...(development && !inherited("TJOMMIE_LAN_PORT") ? { TJOMMIE_LAN_PORT: DEV_LAN_PORT } : {}),
    ...base,
    TJOMMIE_HOME: dataRoot,
    ...(testRoot ? { TJOMMIE_PORT: "0" } : {}),
  };
}

// What the app says when development mode finds no running daemon. With no TJOMMIE_HOME or TJOMMIE_PORT of its own, the app
// and a debug daemon already agree on the folder and port; with either set, the daemon needs the same value.
export function developmentDaemonMessage(env: NodeJS.ProcessEnv, dataRoot: string, port: number): string {
  const same = [env.TJOMMIE_HOME === undefined ? null : `TJOMMIE_HOME=${dataRoot}`, env.TJOMMIE_PORT === undefined ? null : `TJOMMIE_PORT=${port}`].filter((setting) => setting !== null);
  return `Start the Rust daemon in development mode first: cargo run -p daemon --bin tjommie-daemon -- --dev (inside tjommie-rs)${same.length ? `, with the same ${same.join(" and ")} as this app` : ""}.`;
}

export async function stopWithin(stop: Promise<void>, timeoutMs: number): Promise<void> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    await Promise.race([stop, new Promise<never>((_resolve, reject) => {
      timer = setTimeout(() => reject(new Error("Tjommie's background service did not stop in time.")), timeoutMs);
    })]);
  } finally {
    clearTimeout(timer);
  }
}

export async function stopDaemonBeforeQuit(
  running: Pick<RunningDaemon, "stop" | "forceStop">,
  timeoutMs: number,
): Promise<"graceful" | "forced"> {
  try { await stopWithin(running.stop(), timeoutMs); return "graceful"; }
  catch { await stopWithin(running.forceStop(), 10_000); return "forced"; }
}

const lastLine = (value: string): string => value.trim().split(/\r?\n/).at(-1) ?? "";

// Stdin stays open after the token line so the daemon stops when its parent quits or crashes.
export function startDaemonProcess(
  launch: DaemonLaunch,
  onOutput: (text: string) => void = () => {},
  timeoutMs = 60_000,
): Promise<RunningDaemon> {
  const token = newToken();
  const desktopHostToken = newToken();
  const child = spawn(launch.execPath, [...launch.args, "--token-stdin"], {
    env: daemonEnv(launch.env, launch.testAccountsDir, launch.runAsNode ?? false),
    stdio: "pipe",
    windowsHide: true,
  });
  let stdout = "";
  let stderr = "";
  // A descendant can hold the pipes open after the daemon process has exited.
  const processExited = new Promise<void>((resolve) => child.once("exit", () => resolve()));
  const exited = new Promise<{ code: number | null }>((resolve) => child.once("close", (code) => resolve({ code })));
  child.stdout.setEncoding("utf8").on("data", (chunk: string) => {
    onOutput(chunk);
    stdout = (stdout + chunk).slice(-8_000);
  });
  child.stderr.setEncoding("utf8").on("data", (chunk: string) => {
    onOutput(chunk);
    stderr = (stderr + chunk).slice(-4_000);
  });
  child.stdin.on("error", () => {}); // An early daemon exit is reported through exited below.
  child.stdin.write(`${JSON.stringify({ token, desktopHostToken })}\n`);

  const killTree = (): void => {
    if (child.exitCode !== null || child.signalCode !== null || child.pid === undefined) return;
    if (process.platform === "win32") {
      try {
        execFileSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { stdio: "ignore", windowsHide: true });
      } catch {
        // The process may have exited before taskkill runs.
      }
    } else {
      child.kill("SIGKILL");
    }
  };

  const stop = async (): Promise<void> => {
    if (child.exitCode !== null || child.signalCode !== null) {
      await exited;
      return;
    }
    child.stdin.end();
    const timer = setTimeout(killTree, STOP_GRACE_MS);
    try {
      await exited;
    } finally {
      clearTimeout(timer);
    }
  };
  const forceStop = async (): Promise<void> => { killTree(); await processExited; };

  return new Promise<RunningDaemon>((resolve, reject) => {
    let settled = false;
    const fail = (error: Error): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      killTree();
      reject(error);
    };
    const timer = setTimeout(() => fail(new Error("Tjommie's background service didn't start in time.")), timeoutMs);
    child.once("error", fail);
    child.stdout.on("data", () => {
      if (settled) return;
      const port = listeningPort(stdout);
      if (port === null) return;
      settled = true;
      clearTimeout(timer);
      resolve({ connection: { port, token }, desktopHostToken, pid: child.pid, exited, stop, forceStop });
    });
    void exited.then(({ code }) => fail(new Error(lastLine(stderr) || `Tjommie's background service stopped (exit code ${code}).`)));
  });
}
