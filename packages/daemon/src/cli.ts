import { spawn, type ChildProcess, type ChildProcessWithoutNullStreams } from "node:child_process";
import { existsSync } from "node:fs";
import { delimiter, join } from "node:path";

export type CliName = "claude" | "codex";
export type CliCommand = { file: string; args: string[] };

const NPM_ENTRY: Record<CliName, string[]> = {
  claude: ["@anthropic-ai", "claude-code", "cli.js"],
  codex: ["@openai", "codex", "bin", "codex.js"],
};

// Finds a CLI on PATH. A native executable wins. An npm-installed CLI runs its JS entry with our own
// Node, because Windows can't spawn npm's .cmd/.ps1 shims without a shell.
export function resolveCli(name: CliName, env: NodeJS.ProcessEnv = process.env): CliCommand | null {
  const path = Object.entries(env).find(([key]) => key.toUpperCase() === "PATH")?.[1] ?? "";
  const dirs = path.split(delimiter).filter(Boolean);
  const exe = process.platform === "win32" ? `${name}.exe` : name;
  for (const dir of dirs) if (existsSync(join(dir, exe))) return { file: join(dir, exe), args: [] };
  for (const dir of dirs) {
    const entry = join(dir, "node_modules", ...NPM_ENTRY[name]);
    if (existsSync(entry)) return { file: process.execPath, args: [entry] };
  }
  return null;
}

const STRIPPED_ENV = [
  "ANTHROPIC_API_KEY",
  "ANTHROPIC_AUTH_TOKEN",
  "CLAUDE_CODE_OAUTH_TOKEN",
  "CLAUDE_CONFIG_DIR",
  "CLAUDECODE",
  "CLAUDE_CODE_ENTRYPOINT",
  "OPENAI_API_KEY",
  "CODEX_API_KEY",
  "CODEX_HOME",
  "ANTHROPIC_BASE_URL",
  "ANTHROPIC_CUSTOM_HEADERS",
  "CLAUDE_CODE_USE_BEDROCK",
  "CLAUDE_CODE_USE_VERTEX",
  "OPENAI_BASE_URL",
  "OPENAI_API_BASE",
];
const SECRET_ENV = /(TOKEN|SECRET|PASSWORD|PASSWD|CREDENTIAL|CREDENTIALS|PRIVATE_KEY|API_KEY|ACCESS_KEY|_KEY)$/i;
const CLOUD_ENV = /^(AWS_|AZURE_|GOOGLE_|GCP_)/i;

// A CLI must use the login in its Tjommie account folder, never keys or config folders we happen to inherit.
export function cliEnv(extra: Record<string, string>, base: NodeJS.ProcessEnv = process.env): Record<string, string> {
  const env: Record<string, string> = {};
  for (const [key, value] of Object.entries(base)) {
    if (value !== undefined && !STRIPPED_ENV.includes(key.toUpperCase()) && !SECRET_ENV.test(key) && !CLOUD_ENV.test(key)) env[key] = value;
  }
  for (const [key, value] of Object.entries(extra)) {
    if (!SECRET_ENV.test(key) && !CLOUD_ENV.test(key)) env[key] = value;
  }
  return env;
}

export function spawnCli(cmd: CliCommand, args: string[], opts: { cwd: string; env: Record<string, string> }): ChildProcessWithoutNullStreams {
  // This is a new pipe for the CLI protocol. It never shares the daemon's token input pipe.
  return spawn(cmd.file, [...cmd.args, ...args], { cwd: opts.cwd, env: nodeEnv(opts.env), stdio: ["pipe", "pipe", "pipe"], windowsHide: true });
}

function safeChildEnv(env: NodeJS.ProcessEnv): Record<string, string> {
  const accountFolders: Record<string, string> = {};
  for (const [key, value] of Object.entries(env)) {
    if (value !== undefined && (key.toUpperCase() === "CLAUDE_CONFIG_DIR" || key.toUpperCase() === "CODEX_HOME")) {
      accountFolders[key] = value;
    }
  }
  return cliEnv(accountFolders, env);
}

const nodeEnv = (env: Record<string, string>): Record<string, string> => {
  const safe = safeChildEnv(env);
  return process.versions.electron ? { ...safe, ELECTRON_RUN_AS_NODE: "1" } : safe;
};

// Runs a short command (version and status checks) and collects its output.
export function runCli(
  cmd: CliCommand,
  args: string[],
  env: Record<string, string>,
  timeoutMs = 20_000,
): Promise<{ code: number | null; stdout: string; stderr: string }> {
  return new Promise((resolve) => {
    const child = spawn(cmd.file, [...cmd.args, ...args], { env: nodeEnv(env), stdio: ["ignore", "pipe", "pipe"], windowsHide: true });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => (stdout += chunk));
    child.stderr.on("data", (chunk) => (stderr += chunk));
    const timer = setTimeout(() => killTree(child.pid), timeoutMs);
    child.on("error", (error) => {
      clearTimeout(timer);
      resolve({ code: null, stdout, stderr: String(error) });
    });
    child.on("close", (code) => {
      clearTimeout(timer);
      resolve({ code, stdout, stderr });
    });
  });
}

// The CLIs start their own children (shells, MCP servers), so stopping one means stopping its whole tree.
export function killTree(pid: number | undefined): void {
  if (pid === undefined) return;
  if (process.platform === "win32") {
    const child = spawn("taskkill", ["/pid", String(pid), "/T", "/F"], { env: cliEnv({}), stdio: "ignore", windowsHide: true });
    child.on("error", () => {});
    return;
  }
  try {
    process.kill(pid, "SIGKILL");
  } catch {
    // already gone
  }
}

// Stop must wait for taskkill /T to finish, not just for the parent CLI's exit event.
// The caller supplies only a PID from its own live ChildProcess.
export async function killTreeAndWait(pid: number | undefined): Promise<void> {
  if (pid === undefined) throw new Error("Cannot stop a CLI without its process ID");
  if (process.platform !== "win32") {
    try { process.kill(pid, "SIGKILL"); } catch (error) {
      if (!(error instanceof Error && "code" in error && error.code === "ESRCH")) throw error;
    }
    return;
  }
  const result = await new Promise<{ code: number | null; stdout: string; stderr: string; stdoutTruncated: boolean; stderrTruncated: boolean }>((resolve, reject) => {
    const executable = join(process.env.SystemRoot ?? "C:\\Windows", "System32", "taskkill.exe");
    const child = spawn(executable, ["/pid", String(pid), "/T", "/F"], { env: cliEnv({}), stdio: ["ignore", "pipe", "pipe"], windowsHide: true });
    let stdout = "";
    let stderr = "";
    let stdoutTruncated = false;
    let stderrTruncated = false;
    const limit = 65_536;
    child.stdout.setEncoding("utf8").on("data", (chunk: string) => {
      stdoutTruncated ||= stdout.length + chunk.length > limit;
      stdout = (stdout + chunk).slice(0, limit);
    });
    child.stderr.setEncoding("utf8").on("data", (chunk: string) => {
      stderrTruncated ||= stderr.length + chunk.length > limit;
      stderr = (stderr + chunk).slice(0, limit);
    });
    child.once("error", reject);
    child.once("close", (code) => resolve({ code, stdout, stderr, stdoutTruncated, stderrTruncated }));
  });
  if (result.code === 0) return;
  // Sandboxed descendants can refuse taskkill yet disappear when their owner is terminated.
  // A nonzero exit is recoverable only with a complete reported tree and proof every PID is gone.
  const targets = result.code !== null && result.code > 0 && !result.stdoutTruncated && !result.stderrTruncated
    ? taskkillTargets(pid, result.stdout, result.stderr) : null;
  if (targets && await taskkillTargetsGone(pid, targets)) return;
  throw new Error(`Could not stop the CLI process tree (taskkill exit ${result.code}). Try Stop again.`, { cause: { pid, ...result } });
}

function taskkillTargets(root: number, stdout: string, stderr: string): number[] | null {
  const parents = new Map<number, number>();
  const lines = (text: string) => text.split(/\r?\n/).map((line) => line.trim()).filter(Boolean);
  const add = (match: RegExpMatchArray | null): number | null => {
    if (!match) return null;
    const pid = Number(match[1]);
    const parent = Number(match[2]);
    if (![pid, parent].every((value) => Number.isSafeInteger(value) && value > 0 && value <= 0xffff_ffff) || parents.has(pid)) return null;
    parents.set(pid, parent);
    return pid;
  };
  let rootTerminated = false;
  // Unrecognized/localized output keeps the original failure; it cannot prove a complete tree.
  for (const line of lines(stdout)) {
    const pid = add(line.match(/^SUCCESS: The process with PID (\d+) \(child process of PID (\d+)\) has been terminated\.$/));
    if (pid === null) return null;
    if (pid === root) rootTerminated = true;
  }
  const errors = lines(stderr);
  for (let index = 0; index < errors.length; index += 2) {
    if (add(errors[index]!.match(/^ERROR: The process with PID (\d+) \(child process of PID (\d+)\) could not be terminated\.$/)) === null
      || !/^Reason: .+$/.test(errors[index + 1] ?? "")) return null;
  }
  // An absent/failed root alone gives no evidence that taskkill enumerated its descendants.
  if (!rootTerminated || parents.get(root) !== process.pid || parents.has(process.pid)) return null;
  for (const pid of parents.keys()) {
    const seen = new Set<number>();
    let ancestor = pid;
    while (ancestor !== root) {
      const parent = parents.get(ancestor);
      if (parent === undefined || seen.has(ancestor)) return null;
      seen.add(ancestor);
      ancestor = parent;
    }
  }
  return [...parents.keys()];
}

function processGone(pid: number): boolean {
  try { process.kill(pid, 0); } catch (error) {
    return error instanceof Error && "code" in error && error.code === "ESRCH";
  }
  return false;
}

async function taskkillTargetsGone(root: number, targets: number[]): Promise<boolean> {
  const deadline = Date.now() + 2_000;
  while (!processGone(root)) {
    if (Date.now() >= deadline) return false;
    await new Promise<void>((resolve) => setTimeout(resolve, 100));
  }
  for (;;) {
    const gone = targets.map(processGone);
    if (gone.every(Boolean)) return true;
    if (Date.now() >= deadline) return false;
    await new Promise<void>((resolve) => setTimeout(resolve, 100));
  }
}

// Opens a sign-in URL in the default browser. No shell: URLs contain & which cmd.exe would split on.
export function openUrl(url: string): void {
  if (!url.startsWith("https://")) throw new Error(`Refusing to open a non-https URL: ${url}`);
  const child =
    process.platform === "win32"
      ? spawn("rundll32", ["url.dll,FileProtocolHandler", url], { env: cliEnv({}), stdio: "ignore", detached: true })
      : spawn(process.platform === "darwin" ? "open" : "xdg-open", [url], { env: cliEnv({}), stdio: "ignore", detached: true });
  child.on("error", () => {});
  child.unref();
}

// Opens a visible console window running a CLI's own interactive command (Claude's sign-in).
// ponytail: Windows only, like the rest of Phase 1; macOS gets its own opener with the macOS build.
export function openConsole(title: string, command: string, env: Record<string, string>): ChildProcess {
  const child = spawn("cmd.exe", ["/c", "start", "/wait", `"${title}"`, "cmd.exe", "/k", command], {
    env: safeChildEnv(env),
    detached: true,
    stdio: "ignore",
    windowsVerbatimArguments: true,
  });
  child.on("error", () => {});
  child.unref();
  return child;
}
