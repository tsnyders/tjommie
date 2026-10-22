import { execFileSync, spawn } from "node:child_process";
import { randomBytes } from "node:crypto";

export type DaemonConnection = { port: number; token: string };
export type DaemonLaunch = {
  execPath: string;
  args: string[];
  env: NodeJS.ProcessEnv;
  testAccountsDir?: string;
};
export type RunningDaemon = {
  connection: DaemonConnection;
  pid: number | undefined;
  exited: Promise<{ code: number | null }>;
  stop: () => Promise<void>;
};

const LISTENING = /Tjommie daemon listening on http:\/\/127\.0\.0\.1:(\d+)/;
const STOP_GRACE_MS = 10_000;

// A fresh token for every daemon start: 32 random bytes, 43 base64url characters.
export const newToken = (): string => randomBytes(32).toString("base64url");

export function listeningPort(output: string): number | null {
  const match = LISTENING.exec(output);
  return match ? Number(match[1]) : null;
}

// Production mode cannot inherit development settings or the test-only account-folder hook. The explicit
// testAccountsDir option is the only way a desktop test can send that hook to its disposable daemon.
export function daemonEnv(base: NodeJS.ProcessEnv, testAccountsDir?: string): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = Object.fromEntries(
    Object.entries(base).filter(([key]) => !["TJOMMIE_DEV", "TJOMMIE_ACCOUNTS_DIR", "ELECTRON_RUN_AS_NODE"].includes(key.toUpperCase())),
  );
  env.ELECTRON_RUN_AS_NODE = "1";
  if (testAccountsDir !== undefined) env.TJOMMIE_ACCOUNTS_DIR = testAccountsDir;
  return env;
}

const lastLine = (value: string): string => value.trim().split(/\r?\n/).at(-1) ?? "";

// Stdin stays open after the token line so the daemon stops when its parent quits or crashes.
export function startDaemonProcess(
  launch: DaemonLaunch,
  onOutput: (text: string) => void = () => {},
  timeoutMs = 60_000,
): Promise<RunningDaemon> {
  const token = newToken();
  const child = spawn(launch.execPath, [...launch.args, "--token-stdin"], {
    env: daemonEnv(launch.env, launch.testAccountsDir),
    stdio: "pipe",
    windowsHide: true,
  });
  let stdout = "";
  let stderr = "";
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
  child.stdin.write(`${token}\n`);

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
      resolve({ connection: { port, token }, pid: child.pid, exited, stop });
    });
    void exited.then(({ code }) => fail(new Error(lastLine(stderr) || `Tjommie's background service stopped (exit code ${code}).`)));
  });
}
