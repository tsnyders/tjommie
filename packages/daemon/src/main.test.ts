import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { randomBytes } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, expect, it } from "vitest";
import WebSocket from "ws";
import { connect } from "./client.ts";

const workspace = fileURLToPath(new URL("../../../", import.meta.url));
const main = fileURLToPath(new URL("./main.ts", import.meta.url));
const cache = join(workspace, "node_modules", ".cache");
const missingMode = "Start Tjommie from the desktop app, or run npm start -w @tjommie/daemon for development.";
type Fixture = {
  root: string;
  child: ChildProcessWithoutNullStreams;
  closed: Promise<number | null>;
  stdout: () => string;
  stderr: () => string;
};
const fixtures: Fixture[] = [];

function launch(args: string[], options: { root?: string; dev?: string } = {}): Fixture {
  mkdirSync(cache, { recursive: true });
  const root = options.root ?? mkdtempSync(join(cache, "tjommie-token-pipe-"));
  mkdirSync(root, { recursive: true });
  const inheritedEnv = Object.fromEntries(Object.entries(process.env).filter(([key]) => key.toUpperCase() !== "PATH"));
  const child = spawn(process.execPath, ["--disable-warning=ExperimentalWarning", main, ...args], {
    cwd: workspace,
    env: { ...inheritedEnv, PATH: "", TJOMMIE_HOME: root, TJOMMIE_PORT: "0", TJOMMIE_DEV: options.dev ?? "" },
    stdio: "pipe",
    windowsHide: true,
  });
  let out = "";
  let err = "";
  child.stdout.setEncoding("utf8").on("data", (chunk: string) => { out += chunk; });
  child.stderr.setEncoding("utf8").on("data", (chunk: string) => { err += chunk; });
  const closed = new Promise<number | null>((resolve) => child.once("close", resolve));
  const fixture = { root, child, closed, stdout: () => out, stderr: () => err };
  fixtures.push(fixture);
  return fixture;
}

async function eventually<T>(read: () => T | undefined, timeoutMs = 20_000): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const value = read();
    if (value !== undefined) return value;
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
  throw new Error("Daemon subprocess did not reach the expected state.");
}

async function ready(fixture: Fixture): Promise<number> {
  return eventually(() => {
    const match = fixture.stdout().match(/Tjommie daemon listening on http:\/\/127\.0\.0\.1:(\d+)/);
    return match ? Number(match[1]) : undefined;
  });
}

async function originAllowed(port: number, origin: string): Promise<boolean> {
  return new Promise((resolve) => {
    const socket = new WebSocket(`ws://127.0.0.1:${port}`, { origin });
    socket.once("open", () => { socket.close(); resolve(true); });
    socket.once("error", () => resolve(false));
  });
}

async function exited(fixture: Fixture): Promise<number | null> {
  let timer: NodeJS.Timeout | undefined;
  try {
    return await Promise.race([
      fixture.closed,
      new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error("Daemon subprocess did not exit.")), 15_000); }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

async function stop(fixture: Fixture): Promise<void> {
  if (fixture.child.exitCode === null && fixture.child.signalCode === null) fixture.child.kill();
  await exited(fixture);
}

afterEach(async () => {
  const completed = fixtures.splice(0);
  for (const fixture of completed) await stop(fixture);
  for (const root of new Set(completed.map((fixture) => fixture.root))) {
    const target = resolve(root);
    if (dirname(target) !== resolve(cache) || !target.startsWith(join(resolve(cache), "tjommie-token-pipe-"))) {
      throw new Error("Refusing to remove a folder outside the test root.");
    }
    rmSync(target, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
  }
});

it.each([false, true])("authenticates a piped production token, removes a stale file (%s), and stays up after stdin closes", async (stale) => {
  mkdirSync(cache, { recursive: true });
  const root = mkdtempSync(join(cache, "tjommie-token-pipe-"));
  const tokenFile = join(root, "daemon-token");
  if (stale) writeFileSync(tokenFile, randomBytes(32).toString("base64url"));
  const token = randomBytes(32).toString("base64url");
  const fixture = launch(["--token-stdin"], { root });
  fixture.child.stdin.end(`${token}\nignored second line\n`);
  const port = await ready(fixture);
  expect(await originAllowed(port, "http://localhost:3000")).toBe(false);
  expect(existsSync(tokenFile)).toBe(false);
  if (stale) expect(fixture.stdout() + fixture.stderr()).toContain("Removed a leftover development daemon-token file.");
  const client = await connect(port, token);
  try {
    expect(await client.call("bots.list", {})).toEqual([]);
    await expect(connect(port, randomBytes(32).toString("base64url"))).rejects.toMatchObject({ code: 4401 });
  } finally {
    await client.close();
  }
  expect(fixture.child.exitCode).toBeNull();
  expect(existsSync(tokenFile)).toBe(false);
}, 30_000);

it.each(["short", "invalid", "missing", "timeout"] as const)("exits plainly for a %s production token", async (caseName) => {
  const fixture = launch(["--token-stdin"]);
  if (caseName === "short") fixture.child.stdin.end("too-short\n");
  else if (caseName === "invalid") fixture.child.stdin.end(`${"x".repeat(43)}=\n`);
  else if (caseName === "missing") fixture.child.stdin.end();
  expect(await exited(fixture)).toBe(1);
  expect(fixture.stderr().trim()).toBe(caseName === "short" || caseName === "invalid"
    ? "Daemon token must be at least 43 base64url characters."
    : caseName === "missing" ? "stdin closed before a daemon token arrived."
      : "No daemon token arrived on stdin within 5 seconds.");
  expect(fixture.stdout()).toBe("");
  expect(existsSync(join(fixture.root, "daemon-token"))).toBe(false);
}, 20_000);

it("requires an explicit mode", async () => {
  const fixture = launch([]);
  expect(await exited(fixture)).toBe(1);
  expect(fixture.stderr().trim()).toBe(missingMode);
  expect(fixture.stdout()).toBe("");
  expect(existsSync(join(fixture.root, "daemon-token"))).toBe(false);
});

it("rejects an unknown argument with its name", async () => {
  const fixture = launch(["--dev", "--port", "1"]);
  expect(await exited(fixture)).toBe(1);
  expect(fixture.stderr().trim()).toBe("Unknown argument: --port");
  expect(fixture.stdout()).toBe("");
  expect(existsSync(join(fixture.root, "daemon-token"))).toBe(false);
});

it("prints an unexpected startup error with its stack and without the token", async () => {
  mkdirSync(cache, { recursive: true });
  const root = mkdtempSync(join(cache, "tjommie-token-pipe-"));
  mkdirSync(join(root, "tjommie.db"));
  const token = randomBytes(32).toString("base64url");
  const fixture = launch(["--token-stdin"], { root });
  fixture.child.stdin.end(`${token}\n`);
  expect(await exited(fixture)).toBe(1);
  expect(fixture.stderr()).toContain("    at ");
  expect(fixture.stderr()).not.toContain(token);
  expect(fixture.stdout()).toBe("");
}, 20_000);

it.each([{ args: ["--dev"], dev: "" }, { args: [], dev: "1" }])("writes and reuses the development token file with $args and TJOMMIE_DEV=$dev", async ({ args, dev }) => {
  const first = launch(args, { dev });
  await ready(first);
  const tokenFile = join(first.root, "daemon-token");
  const token = readFileSync(tokenFile, "utf8");
  expect(token).toMatch(/^[A-Za-z0-9_-]{43}$/);
  await stop(first);
  const second = launch(args, { dev, root: first.root });
  const port = await ready(second);
  expect(await originAllowed(port, "http://localhost:3000")).toBe(true);
  expect(readFileSync(tokenFile, "utf8")).toBe(token);
  const client = await connect(port, token);
  await client.close();
}, 30_000);

it("rejects conflicting production and development modes before reading stdin", async () => {
  const fixture = launch(["--token-stdin"], { dev: "1" });
  expect(await exited(fixture)).toBe(1);
  expect(fixture.stderr().trim()).toBe("Choose either --dev or --token-stdin.");
  expect(existsSync(join(fixture.root, "daemon-token"))).toBe(false);
});
