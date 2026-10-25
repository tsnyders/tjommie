import { execFileSync, spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { readFileSync } from "node:fs";
import { get } from "node:https";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dirname, "../../..");

function healthy(base: string, certPath?: string): Promise<boolean> {
  if (!certPath) return fetch(`${base}/v1/health`, { signal: AbortSignal.timeout(2_000) })
    .then(async (response) => response.ok && (await response.json() as { relay?: string }).relay === "tjommie-relay/1")
    .catch(() => false);
  return new Promise((done) => {
    const request = get(`${base}/v1/health`, { ca: readFileSync(certPath) }, (response) => {
      response.resume();
      done(response.statusCode === 200);
    });
    request.setTimeout(2_000, () => request.destroy());
    request.once("error", () => done(false));
  });
}

export async function startLocalRelay(configDir: string, https?: { keyPath: string; certPath: string }): Promise<{ child: ChildProcessWithoutNullStreams; base: string }> {
  const protocol = https ? "https" : "http";
  const args = [join(root, "node_modules/wrangler/bin/wrangler.js"), "dev", "--local", "--port", "0",
    ...(https ? ["--local-protocol", "https", "--https-key-path", https.keyPath, "--https-cert-path", https.certPath] : []),
    "--persist-to", configDir, "--config", "packages/relay/wrangler.toml"];
  const child = spawn(process.execPath, args, {
    cwd: root, windowsHide: true, stdio: ["pipe", "pipe", "pipe"],
    env: { PATH: process.env.PATH ?? "", SystemRoot: process.env.SystemRoot ?? "C:\\Windows", WINDIR: process.env.SystemRoot ?? "C:\\Windows",
      TEMP: tmpdir(), TMP: tmpdir(), USERPROFILE: configDir, APPDATA: configDir, LOCALAPPDATA: configDir, HOME: configDir,
      CI: "true", WRANGLER_SEND_METRICS: "false", XDG_CONFIG_HOME: configDir },
  });
  try {
    const base = await new Promise<string>((resolveBase, reject) => {
      let output = "", checking = false, finished = false;
      const collect = (chunk: Buffer): void => { output = (output + chunk.toString()).slice(-4_000); };
      const finish = (error?: Error, url?: string): void => {
        if (finished) return;
        finished = true; clearTimeout(timeout); clearInterval(probe);
        child.stdout.off("data", collect); child.stderr.off("data", collect);
        child.off("error", onError); child.off("exit", onExit);
        if (error) reject(error); else resolveBase(url!);
      };
      const onError = () => finish(new Error("Local relay could not start."));
      const onExit = () => finish(new Error(`Local relay exited before it was ready: ${output.slice(-1_000)}`));
      const timeout = setTimeout(() => finish(new Error(`Local relay did not start: ${output.slice(-1_000)}`)), 30_000);
      const probe = setInterval(() => {
        const port = [...output.matchAll(new RegExp(`${protocol}:\\/\\/(?:127\\.0\\.0\\.1|localhost):(\\d+)`, "g"))].at(-1)?.[1];
        if (finished || checking || !port || port === "0") return;
        checking = true;
        const url = `${protocol}://127.0.0.1:${port}`;
        void healthy(url, https?.certPath).then((ok) => { if (ok) finish(undefined, url); }).finally(() => { checking = false; });
      }, 250);
      child.stdout.on("data", collect); child.stderr.on("data", collect);
      child.once("error", onError); child.once("exit", onExit);
    });
    return { child, base };
  } catch (error) {
    await stopLocalRelay(child).catch(() => {});
    throw error;
  }
}

export async function stopLocalRelay(child: ChildProcessWithoutNullStreams): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return;
  child.stdin.end("q\n");
  const exited = await new Promise<boolean>((done) => {
    const timer = setTimeout(() => done(false), 3_000);
    child.once("exit", () => { clearTimeout(timer); done(true); });
  });
  if (exited) return;
  if (process.platform === "win32" && child.pid) {
    try { execFileSync("taskkill", ["/PID", String(child.pid), "/T", "/F"], { windowsHide: true, stdio: "ignore" }); }
    catch { /* The exit check below decides whether cleanup failed. */ }
  } else child.kill();
  if (child.exitCode === null && child.signalCode === null) await new Promise<void>((done) => {
    const timer = setTimeout(done, 1_000);
    child.once("exit", () => { clearTimeout(timer); done(); });
  });
  if (child.exitCode === null && child.signalCode === null) throw new Error("The local relay did not stop.");
}
