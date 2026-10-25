import { randomUUID } from "node:crypto";
import { normalizeCodexUsage, readClaudeAccountUsage, type AccountUsageRead } from "./account-quota.ts";
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, isAbsolute, join, resolve } from "node:path";
import type { CodexSandboxReadiness, ModelInfo, Provider, ProviderSetup } from "@tjommie/protocol";
import { cliEnv, killTreeAndWait, openUrl, resolveCli, runCli, spawnCli } from "./cli.ts";
import { bindLegacyBots, getDefaultAccount, listAccounts, type AccountLimitWindow } from "./accounts-store.ts";
import { nowIso, type Db } from "./db.ts";
import { startCodexServer } from "./providers/codex-rpc.ts";
import { sweepClaudeSessionFiles } from "./providers/claude-files.ts";
import { RpcError } from "./server.ts";

export type Account = { id: string; provider: Provider; label: string; folder: string };

const SIGN_IN_TIMEOUT_MS = 10 * 60_000;

// Aliases always resolve to the newest model of each family.
export const CLAUDE_MODELS: ModelInfo[] = [
  { id: "sonnet", label: "Claude Sonnet (latest)", isDefault: true },
  { id: "opus", label: "Claude Opus (latest)", isDefault: false },
  { id: "haiku", label: "Claude Haiku (latest)", isDefault: false },
];

// Phase 1 has one Tjommie-owned account per provider. Its folder is the CLI's config home, so bots never
// load the person's own CLI setup (hooks, plugins, skills, memories, MCP servers).
export function ensureAccounts(db: Db, accountsDir: string, dataRoot = dirname(accountsDir)): Account[] {
  for (const provider of ["claude", "codex"] as const) {
    if (listAccounts(db, { includeDeleted: true }).some((entry) => entry.provider === provider)) continue;
    const id = `${provider}-main`;
    const folder = join(accountsDir, id);
    mkdirSync(folder, { recursive: true });
    db.prepare("INSERT OR IGNORE INTO accounts (id, provider, label, kind, folder, created_at) VALUES (?, ?, ?, 'subscription', ?, ?)").run(
      id,
      provider,
      provider === "claude" ? "Claude" : "Codex",
      folder,
      nowIso(),
    );
  }
  for (const account of listAccounts(db)) prepareAccountFolder(account, dataRoot);
  for (const provider of ["claude", "codex"] as const) {
    if (listAccounts(db).some((account) => account.provider === provider) && !listAccounts(db).some((account) => account.provider === provider && account.isDefault)) db.prepare("UPDATE accounts SET is_default = 1 WHERE id = ?").run(getDefaultAccount(db, provider).id);
  }
  bindLegacyBots(db);
  return listAccounts(db).map(({ id, provider, label, folder }) => ({ id, provider, label, folder }));
}

export function prepareAccountFolder(account: Account, dataRoot: string): void {
  mkdirSync(account.folder, { recursive: true });
  if (lstatSync(account.folder).isSymbolicLink()) throw new Error("The account folder must not be a link.");
  if (account.provider === "claude") { sweepClaudeSessionFiles(account.folder); hardenClaudeSettings(account.folder, dataRoot); }
}

function hardenClaudeSettings(folder: string, dataRoot: string): void {
  const file = join(folder, "settings.json");
  const parsed = readClaudeSettings(file);
  if (!parsed) return;
  const { settings, permissions, existing } = parsed;
  const stale = staleClaudeReadRules(existing);
  const deny = [...new Set([...existing.filter((rule) => !stale.has(rule)), "Edit(**/.claude/**)", "Write(**/.claude/**)", ...claudeReadRules(dataRoot)])];
  if (existing.length === deny.length && existing.every((rule, index) => rule === deny[index])) return;
  writeFileSync(file, `${JSON.stringify({ ...settings, permissions: { ...permissions, deny } }, null, 2)}\n`, { mode: 0o600 });
}

function claudeReadRules(dataRoot: string): string[] {
  // Claude matches absolute Windows paths as POSIX drive paths, prefixed with //.
  const root = resolve(dataRoot).replace(/\\/g, "/").replace(/^([A-Za-z]):/, (_match, drive: string) => `/${drive.toLowerCase()}`);
  return [`Read(/${root}/accounts/**)`, `Read(/${root}/daemon-token)`, `Read(/${root}/data/**)`];
}

function staleClaudeReadRules(existing: string[]): Set<string> {
  const rules = new Set(existing);
  const checked = new Set<string>();
  const stale = new Set<string>();
  for (const rule of rules) {
    const root = /^Read\((\/\/.+)\/(?:accounts\/\*\*|daemon-token|data\/\*\*|tjommie\.db\*)\)$/.exec(rule)?.[1];
    if (!root || /[*?\[\]{}]/.test(root) || checked.has(root)) continue;
    checked.add(root);
    const folder = process.platform === "win32" ? root.slice(1).replace(/^\/([a-z])(?=\/|$)/, "$1:") : root.slice(1);
    if (!isAbsolute(folder)) continue;
    const managed = claudeReadRules(folder);
    // Tjommie always writes this exact group together. Preserve partial groups and user globs.
    const legacyDbRule = `Read(${root}/tjommie.db*)`;
    if (!managed.includes(rule) && rule !== legacyDbRule) continue;
    if (!managed.every((entry) => rules.has(entry)) && !(rule === legacyDbRule && rules.has(legacyDbRule))) continue;
    try {
      lstatSync(folder);
    } catch (error) {
      // Keep live roots, links and uncertain states such as access denied.
      if (isRecord(error) && error.code === "ENOENT") {
        for (const entry of managed) stale.add(entry);
        if (rules.has(legacyDbRule)) stale.add(legacyDbRule);
      }
    }
  }
  return stale;
}

function readClaudeSettings(file: string): { settings: Record<string, unknown>; permissions: Record<string, unknown>; existing: string[] } | null {
  const contents = existsSync(file) ? readFileSync(file, "utf8") : "{}";
  try {
    const settings: unknown = JSON.parse(contents);
    if (!isRecord(settings) || Array.isArray(settings)) throw new Error("Expected a settings object");
    const permissions = settings.permissions === undefined ? {} : settings.permissions;
    if (!isRecord(permissions) || Array.isArray(permissions)) throw new Error("Expected a permissions object");
    const existing = permissions.deny === undefined ? [] : permissions.deny;
    if (!Array.isArray(existing) || !existing.every((rule): rule is string => typeof rule === "string")) throw new Error("Expected a list of deny rules");
    return { settings, permissions, existing };
  } catch {
    // Do not include a JSON parser error: it can contain values from the settings file.
    console.error("Could not read Claude's settings. The existing file was left unchanged.", file);
    return null;
  }
}

export function accountFor(db: Db, provider: Provider): Account {
  const { id, label, folder } = getDefaultAccount(db, provider);
  return { id, provider, label, folder };
}

export function accountEnv(account: Account): Record<string, string> {
  return cliEnv(account.provider === "claude" ? { CLAUDE_CONFIG_DIR: account.folder } : { CODEX_HOME: account.folder });
}

export const parseVersion = (text: string): string | null => text.match(/\d+\.\d+\.\d+/)?.[0] ?? null;

type ClaudeStatus = { signedIn: boolean; account: string | null; plan: string | null };

function tryParseClaudeStatus(text: string): ClaudeStatus | null {
  try {
    const status = JSON.parse(text) as unknown;
    if (!isRecord(status) || typeof status.loggedIn !== "boolean") return null;
    return {
      signedIn: status.loggedIn && !["api_key", "api_key_helper", "third_party"].includes(String(status.authMethod)),
      account: typeof status.email === "string" ? status.email : null,
      plan: typeof status.subscriptionType === "string" ? status.subscriptionType : null,
    };
  } catch {
    return null;
  }
}

export function parseClaudeStatus(text: string): ClaudeStatus {
  return tryParseClaudeStatus(text) ?? { signedIn: false, account: null, plan: null };
}

const notInstalled = (provider: Provider): ProviderSetup => {
  const fields = { installed: false, version: null, signedIn: false, account: null, plan: null,
    error: `${provider === "claude" ? "Claude Code" : "Codex"} is not installed` };
  return provider === "codex" ? { provider: "codex", ...fields, sandbox: "unavailable" } : { provider: "claude", ...fields };
};

const failedCheck = (provider: Provider, version: string | null, error: string): ProviderSetup => {
  const fields = { installed: true, version, signedIn: false, account: null, plan: null, error };
  return provider === "codex" ? { provider: "codex", ...fields, sandbox: "unavailable" } : { provider: "claude", ...fields };
};

async function claudeSetup(account: Account): Promise<ProviderSetup> {
  const cmd = resolveCli("claude");
  if (!cmd) return notInstalled("claude");
  const env = accountEnv(account);
  const [versionResult, statusResult] = await Promise.all([runCli(cmd, ["--version"], env), runCli(cmd, ["auth", "status"], env)]);
  const version = versionResult.code === 0 ? parseVersion(versionResult.stdout) : null;
  if (versionResult.code !== 0) return failedCheck("claude", null, "Claude Code version check failed");
  const status = tryParseClaudeStatus(statusResult.stdout);
  if (statusResult.code === null || status === null) return failedCheck("claude", version, "Claude sign-in check failed");
  return { provider: "claude", installed: true, version, ...status, error: null };
}

async function codexSetup(account: Account): Promise<ProviderSetup> {
  const cmd = resolveCli("codex");
  if (!cmd) return notInstalled("codex");
  const env = accountEnv(account);
  const versionResult = await runCli(cmd, ["--version"], env);
  if (versionResult.code !== 0) return failedCheck("codex", null, "Codex version check failed");
  const version = parseVersion(versionResult.stdout);
  try {
    const server = await startCodexServer(cmd, env, account.folder);
    try {
      const { account: signedIn } = await server.rpc.request<{ account: { type?: string; email?: string; planType?: string } | null }>("account/read", {});
      const sandbox = await codexSandboxStatus(server.rpc, account.folder);
      return { provider: "codex", installed: true, version, signedIn: signedIn !== null && signedIn.type !== "apiKey", account: signedIn?.email ?? null, plan: signedIn?.planType ?? null, error: null, sandbox };
    } finally {
      server.close();
      await server.waitForExit?.();
    }
  } catch {
    return { provider: "codex", installed: true, version, signedIn: false, account: null, plan: null, error: "Codex account check failed.", sandbox: "unavailable" };
  }
}

export type AccountLimitSnapshot = { limitId: string; windows: AccountLimitWindow[] };

// A malformed or incomplete read must never clear a known exhausted window.
export function parseCodexAccountLimits(value: unknown): AccountLimitSnapshot[] | null {
  if (!isRecord(value) || Array.isArray(value)) return null;
  const multi = value.rateLimitsByLimitId;
  if (multi !== undefined && multi !== null && (!isRecord(multi) || Array.isArray(multi))) return null;
  const entries = isRecord(multi) ? Object.entries(multi) : [[undefined, value.rateLimits]] as const;
  if (entries.length === 0 || entries.length > 32) return null;
  const snapshots: AccountLimitSnapshot[] = [];
  for (const [key, bucket] of entries) {
    if (!isRecord(bucket) || Array.isArray(bucket)) return null;
    const limitId = key ?? bucket.limitId ?? "codex";
    if (typeof limitId !== "string" || !limitId.trim() || limitId.length > 100 || key !== undefined && bucket.limitId != null && key !== bucket.limitId) return null;
    const windows: AccountLimitWindow[] = [];
    for (const windowKey of ["primary", "secondary"] as const) {
      const window = bucket[windowKey]; if (window == null) continue;
      if (!isRecord(window) || Array.isArray(window)) return null;
      const used = window.usedPercent, duration = window.windowDurationMins, reset = window.resetsAt;
      if (typeof used !== "number" || !Number.isFinite(used) || used < 0 || used > 100) return null;
      if (duration != null && (typeof duration !== "number" || !Number.isFinite(duration) || duration <= 0)) return null;
      if (reset != null && (typeof reset !== "number" || !Number.isFinite(reset) || !Number.isFinite(new Date(reset * 1000).getTime()))) return null;
      windows.push({ windowKey, durationMinutes: typeof duration === "number" ? duration : null, usedPercent: used,
        resetsAt: typeof reset === "number" ? new Date(reset * 1000).toISOString() : null,
        state: used >= 100 ? "reached" : used >= 90 ? "near" : "ok" });
    }
    if (bucket.rateLimitReachedType != null) {
      if (typeof bucket.rateLimitReachedType !== "string" || !bucket.rateLimitReachedType.trim()) return null;
      if (!windows.some((window) => window.state === "reached")) windows.push({ windowKey: "provider", durationMinutes: null, usedPercent: null, resetsAt: null, state: "reached" });
    }
    if (windows.length === 0) return null;
    snapshots.push({ limitId, windows });
  }
  return snapshots;
}

// ChatGPT subscription metadata only: never creates a thread or starts a model turn.
async function readCodexAccountLimits(account: Account): Promise<unknown> {
  if (account.provider !== "codex") return null;
  const cmd = resolveCli("codex"); if (!cmd) return null;
  try {
    const server = await startCodexServer(cmd, accountEnv(account), account.folder);
    try { return await server.rpc.request("account/rateLimits/read", {}, 15_000); }
    finally {
      server.close();
      let timer: NodeJS.Timeout | undefined;
      try {
        await Promise.race([server.waitForExit?.(), new Promise<never>((_resolve, reject) => {
          timer = setTimeout(() => reject(new Error("Account check process did not close.")), 5_000);
        })]);
      } finally { clearTimeout(timer); }
    }
  } catch { return null; }
}

export async function checkAccountLimits(account: Account): Promise<AccountLimitSnapshot[] | null> {
  return parseCodexAccountLimits(await readCodexAccountLimits(account));
}

export async function checkAccountUsage(account: Account, accountsRoot: string): Promise<AccountUsageRead> {
  if (account.provider === "claude") return readClaudeAccountUsage(account, { accountsRoot });
  const value = await readCodexAccountLimits(account);
  return normalizeCodexUsage(value, parseCodexAccountLimits(value));
}

type RpcRequester = { request: <T = unknown>(method: string, params?: unknown, timeoutMs?: number) => Promise<T> };

async function codexSandboxStatus(rpc: RpcRequester, cwd: string): Promise<CodexSandboxReadiness> {
  if (process.platform !== "win32") return "unavailable";
  try {
    const config: unknown = await rpc.request("config/read", { cwd });
    const readiness: unknown = await rpc.request("windowsSandbox/readiness", {});
    if (!isRecord(config) || !isRecord(config.config) || !isRecord(readiness)) return "unavailable";
    if (!isRecord(config.config.windows) || config.config.windows.sandbox !== "elevated") return "notConfigured";
    switch (readiness.status) {
      case "ready": case "notConfigured": case "updateRequired": return readiness.status;
      default: return "unavailable";
    }
  } catch {
    return "unavailable";
  }
}

const sandboxSetupInProgress = new Set<string>();
const SANDBOX_SETUP_TIMEOUT_MS = 15 * 60_000;

// This is an explicit user action. A failed or declined setup is never retried automatically.
export async function prepareCodexSandbox(db: Db, timeoutMs = SANDBOX_SETUP_TIMEOUT_MS): Promise<void> {
  return prepareAccountCodexSandbox(accountFor(db, "codex"), timeoutMs);
}
export async function prepareAccountCodexSandbox(account: Account, timeoutMs = SANDBOX_SETUP_TIMEOUT_MS): Promise<void> {
  if (process.platform !== "win32") throw new RpcError("bad_state", "Codex's sandbox setup is available on Windows only.");
  if (account.provider !== "codex") throw new RpcError("bad_params", "Choose a Codex account.");
  if (sandboxSetupInProgress.has(account.folder)) throw new RpcError("busy", "Codex's sandbox setup is already running.");
  const cmd = resolveCli("codex");
  if (!cmd) throw new RpcError("not_installed", "Codex is not installed");
  sandboxSetupInProgress.add(account.folder);
  let cwd: string | null = null;
  try {
    cwd = mkdtempSync(join(tmpdir(), "tjommie-codex-setup-"));
    const server = await startCodexServer(cmd, accountEnv(account), cwd);
    try {
      let timer: NodeJS.Timeout | undefined;
      let complete: (value: { success: boolean; error: string | null }) => void = () => {};
      let fail: (error: Error) => void = () => {};
      const finished = new Promise<{ success: boolean; error: string | null }>((resolve, reject) => {
        complete = resolve;
        fail = reject;
      });
      server.rpc.onNotification = (method, params) => {
        if (method !== "windowsSandbox/setupCompleted" || !isRecord(params) || params.mode !== "elevated") return;
        complete({ success: params.success === true, error: typeof params.error === "string" ? params.error : null });
      };
      try {
        const started = await server.rpc.request<{ started: boolean }>("windowsSandbox/setupStart", { mode: "elevated", cwd });
        if (started.started !== true) throw new RpcError("bad_state", "Codex did not start sandbox setup.");
        timer = setTimeout(() => fail(new RpcError("bad_state", "Codex's sandbox setup timed out after 15 minutes.")), timeoutMs);
        const result = await finished;
        if (!result.success) throw new RpcError("bad_state", result.error ?? "Codex's sandbox setup was declined or failed.");
      } finally {
        clearTimeout(timer);
      }
    } finally {
      server.close();
      await server.waitForExit?.();
    }
    const fresh = await startCodexServer(cmd, accountEnv(account), cwd);
    try {
      if (await codexSandboxStatus(fresh.rpc, cwd) !== "ready") throw new RpcError("bad_state", "Codex's sandbox is still not ready after setup.");
    } finally {
      fresh.close();
      await fresh.waitForExit?.();
    }
  } finally {
    sandboxSetupInProgress.delete(account.folder);
    if (cwd) rmSync(cwd, { recursive: true, force: true });
  }
}

export function checkSetup(db: Db): Promise<ProviderSetup[]> {
  return Promise.all((["claude", "codex"] as const).map((provider) => {
    try { return checkAccountSetup(accountFor(db, provider)); }
    catch { return Promise.resolve(failedCheck(provider, null, "Add an account in Accounts.")); }
  }));
}
export function checkAccountSetup(account: Account): Promise<ProviderSetup> {
  return account.provider === "claude" ? claudeSetup(account) : codexSetup(account);
}

// Explicit owner removal only. The provider removes its own authentication using the
// selected account home; Tjommie never reads or copies a credential file.
export async function logoutAccount(account: Account): Promise<void> {
  const command = resolveCli(account.provider);
  if (!command) throw new RpcError("not_installed", "Install this account's provider before removing its local sign-in.");
  try {
    const result = await runCli(command, account.provider === "claude" ? ["auth", "logout"] : ["logout"], accountEnv(account));
    if (result.code !== 0) throw new Error("Logout failed");
  } catch { throw new RpcError("bad_state", "This account could not sign out. It has been kept so you can retry removal."); }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

// Starts the provider's own sign-in for its Tjommie account and calls onDone once it finishes or times out.
// Claude: hidden `claude auth login` opens its own browser callback flow. Codex: the
// app-server sign-in URL opens in the browser. Tjommie never sees a password or token; each CLI stores its login in the account folder.
type SignInAttempt = { id: string; provider: Provider; url: string | null | undefined; cancel?: () => Promise<void> };
const signInProgress = new Map<string, SignInAttempt>();

export async function signIn(db: Db, provider: Provider, onDone: (ok: boolean, attempt: string) => void): Promise<{ url: string | null; attempt: string }> {
  const account = accountFor(db, provider);
  return signInAccount(account, onDone);
}
export async function cancelAccountSignIn(account: Account, id: string): Promise<void> {
  const attempt = signInProgress.get(account.folder);
  if (!attempt || attempt.id !== id) throw new RpcError("bad_state", "That sign-in attempt is no longer active.");
  await attempt.cancel?.();
}
export async function signInAccount(account: Account, onDone: (ok: boolean, attempt: string) => void): Promise<{ url: string | null; attempt: string }> {
  const provider = account.provider;
  const running = signInProgress.get(account.folder);
  if (running) {
    if (running.provider !== provider || running.url === undefined) throw new RpcError("busy", `${provider === "claude" ? "Claude" : "Codex"} sign-in is already running.`);
    if (provider === "codex" && running.url !== null) openUrl(running.url);
    return { url: running.url, attempt: running.id };
  }
  const attempt: SignInAttempt = { id: randomUUID(), provider, url: undefined };
  signInProgress.set(account.folder, attempt);
  let finished = false;
  let cleanup = (): void => {};
  const finish = (ok: boolean) => {
    if (finished) return;
    finished = true;
    try {
      cleanup();
    } finally {
      if (signInProgress.get(account.folder) === attempt) signInProgress.delete(account.folder);
      onDone(ok, attempt.id);
    }
  };
  attempt.cancel = async () => { finish(false); };
  try {
    const cmd = resolveCli(provider);
    if (!cmd) throw new RpcError("not_installed", notInstalled(provider).error!);
    const env = accountEnv(account);

    if (provider === "claude") {
      const beforeResult = await runCli(cmd, ["auth", "status"], env);
      if (finished) throw new RpcError("bad_state", "The sign-in attempt was cancelled.");
      const before = tryParseClaudeStatus(beforeResult.stdout);
      if (beforeResult.code === null || !before) throw new RpcError("bad_state", "Claude sign-in check failed.");
      const child = spawnCli(cmd, ["auth", "login", "--claudeai"], { cwd: account.folder, env });
      // auth login opens the automatic loopback URL itself. Its printed URL is the
      // manual-code fallback, so it must not be offered as an equivalent reopen link.
      // No auth output is logged or returned; this integration supports callback login.
      attempt.url = null;
      child.stdout.resume(); child.stderr.resume(); child.stdin.on("error", () => {});
      let exited = false, cancelling = false;
      let resolveExit: () => void = () => {};
      const exitedPromise = new Promise<void>((resolve) => { resolveExit = resolve; });
      const deadlineTimer = setTimeout(() => {
        void attempt.cancel?.().catch(() => { /* Retain the attempt for an explicit retry if containment fails. */ });
      }, SIGN_IN_TIMEOUT_MS);
      deadlineTimer.unref();
      const onExit = (code: number | null): void => { exited = true; resolveExit(); finish(!cancelling && code === 0); };
      const onError = (): void => { exited = true; resolveExit(); finish(false); };
      cleanup = () => { clearTimeout(deadlineTimer); child.removeListener("close", onExit); child.removeListener("exit", onExit); child.removeListener("error", onError); };
      child.once("close", onExit); child.once("exit", onExit); child.once("error", onError);
      attempt.cancel = async () => {
        cancelling = true;
        if (!exited && child.exitCode === null && child.signalCode === null) await killTreeAndWait(child.pid);
        await exitedPromise;
        finish(false);
      };
      return { url: null, attempt: attempt.id };
    }

    const server = await startCodexServer(cmd, env, account.folder);
    if (finished) { server.close(); await server.waitForExit?.(); throw new RpcError("bad_state", "The sign-in attempt was cancelled."); }
    const timer = setTimeout(() => finish(false), SIGN_IN_TIMEOUT_MS);
    cleanup = () => {
      clearTimeout(timer);
      server.close();
    };
    let loginId: string | null = null;
    const early: Record<string, unknown>[] = [];
    server.rpc.onNotification = (method, params) => {
      if (method !== "account/login/completed" || !isRecord(params)) return;
      if (loginId === null) { if (early.length < 10) early.push(params); return; }
      if (params.loginId === loginId) finish(params.success === true);
    };
    attempt.cancel = async () => { if (loginId) await server.rpc.request("account/login/cancel", { loginId }).catch(() => {}); finish(false); await server.waitForExit?.(); };
    const login = await server.rpc.request<{ authUrl: string; loginId: string }>("account/login/start", { type: "chatgpt" });
    if (typeof login.loginId !== "string" || !login.loginId || typeof login.authUrl !== "string") throw new RpcError("bad_state", "Codex returned an invalid sign-in attempt.");
    loginId = login.loginId;
    if (finished) { await server.rpc.request("account/login/cancel", { loginId }).catch(() => {}); throw new RpcError("bad_state", "The sign-in attempt was cancelled."); }
    attempt.url = login.authUrl;
    const completed = early.find((event) => event.loginId === loginId);
    if (completed) finish(completed.success === true);
    if (!finished) openUrl(login.authUrl);
    return { url: login.authUrl, attempt: attempt.id };
  } catch (error) {
    finish(false);
    throw error;
  }
}

export async function listModels(db: Db, provider: Provider): Promise<ModelInfo[]> {
  if (provider === "claude") return CLAUDE_MODELS;
  const cmd = resolveCli("codex");
  if (!cmd) throw new RpcError("not_installed", "Codex is not installed");
  const account = accountFor(db, "codex");
  const server = await startCodexServer(cmd, accountEnv(account), account.folder);
  try {
    const { data } = await server.rpc.request<{ data: { model: string; displayName: string; hidden: boolean; isDefault: boolean }[] }>("model/list", {});
    return data.filter((model) => !model.hidden).map((model) => ({ id: model.model, label: model.displayName, isDefault: model.isDefault }));
  } finally {
    server.close();
    await server.waitForExit?.();
  }
}
