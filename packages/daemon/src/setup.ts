import { mkdirSync } from "node:fs";
import { join } from "node:path";
import type { ModelInfo, Provider, ProviderSetup } from "@tjommie/protocol";
import { cliEnv, openConsole, openUrl, resolveCli, runCli } from "./cli.ts";
import { nowIso, type Db } from "./db.ts";
import { startCodexServer } from "./providers/codex-rpc.ts";
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
export function ensureAccounts(db: Db, accountsDir: string): Account[] {
  for (const provider of ["claude", "codex"] as const) {
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
  return db.prepare("SELECT id, provider, label, folder FROM accounts ORDER BY created_at, id").all() as Account[];
}

export function accountFor(db: Db, provider: Provider): Account {
  const account = db.prepare("SELECT id, provider, label, folder FROM accounts WHERE provider = ? ORDER BY created_at, id LIMIT 1").get(provider) as
    | Account
    | undefined;
  if (!account) throw new RpcError("no_account", `No ${provider} account`);
  return account;
}

export function accountEnv(account: Account): Record<string, string> {
  return cliEnv(account.provider === "claude" ? { CLAUDE_CONFIG_DIR: account.folder } : { CODEX_HOME: account.folder });
}

export const parseVersion = (text: string): string | null => text.match(/\d+\.\d+\.\d+/)?.[0] ?? null;

export function parseClaudeStatus(text: string): { signedIn: boolean; account: string | null; plan: string | null } {
  try {
    const status = JSON.parse(text) as { loggedIn?: boolean; email?: string; subscriptionType?: string };
    return { signedIn: status.loggedIn === true, account: status.email ?? null, plan: status.subscriptionType ?? null };
  } catch {
    return { signedIn: false, account: null, plan: null };
  }
}

const notInstalled = (provider: Provider): ProviderSetup => ({
  provider,
  installed: false,
  version: null,
  signedIn: false,
  account: null,
  plan: null,
  error: `${provider === "claude" ? "Claude Code" : "Codex"} is not installed`,
});

const failedCheck = (provider: Provider, version: string | null, error: string): ProviderSetup => ({
  provider,
  installed: true,
  version,
  signedIn: false,
  account: null,
  plan: null,
  error,
});

async function claudeSetup(account: Account): Promise<ProviderSetup> {
  const cmd = resolveCli("claude");
  if (!cmd) return notInstalled("claude");
  const env = accountEnv(account);
  const [versionResult, statusResult] = await Promise.all([runCli(cmd, ["--version"], env), runCli(cmd, ["auth", "status"], env)]);
  const version = versionResult.code === 0 ? parseVersion(versionResult.stdout) : null;
  if (versionResult.code !== 0) return failedCheck("claude", null, "Claude Code version check failed");
  if (statusResult.code !== 0) return failedCheck("claude", version, "Claude sign-in check failed");
  return { provider: "claude", installed: true, version, ...parseClaudeStatus(statusResult.stdout), error: null };
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
      const { account: signedIn } = await server.rpc.request<{ account: { email?: string; planType?: string } | null }>("account/read", {});
      return { provider: "codex", installed: true, version, signedIn: signedIn !== null, account: signedIn?.email ?? null, plan: signedIn?.planType ?? null, error: null };
    } finally {
      server.close();
    }
  } catch (error) {
    return { provider: "codex", installed: true, version, signedIn: false, account: null, plan: null, error: String(error) };
  }
}

export function checkSetup(db: Db): Promise<ProviderSetup[]> {
  return Promise.all([claudeSetup(accountFor(db, "claude")), codexSetup(accountFor(db, "codex"))]);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

// Starts the provider's own sign-in for its Tjommie account and calls onDone once it finishes or times out.
// Claude: `claude auth login` in a console window. Codex: OpenAI's sign-in URL from the app-server, opened
// in the browser. Tjommie never sees a password or token; each CLI stores its login in the account folder.
export async function signIn(db: Db, provider: Provider, onDone: (ok: boolean) => void): Promise<{ url: string | null }> {
  const account = accountFor(db, provider);
  const cmd = resolveCli(provider);
  if (!cmd) throw new RpcError("not_installed", notInstalled(provider).error!);
  const env = accountEnv(account);

  if (provider === "claude") {
    openConsole("Sign in to Claude for Tjommie", "claude auth login", env);
    const deadline = Date.now() + SIGN_IN_TIMEOUT_MS;
    const poll = async (): Promise<void> => {
      if (parseClaudeStatus((await runCli(cmd, ["auth", "status"], env)).stdout).signedIn) return onDone(true);
      if (Date.now() > deadline) return onDone(false);
      setTimeout(() => void poll(), 3000);
    };
    setTimeout(() => void poll(), 3000);
    return { url: null };
  }

  const server = await startCodexServer(cmd, env, account.folder);
  let finished = false;
  const finish = (ok: boolean) => {
    if (finished) return;
    finished = true;
    clearTimeout(timer);
    server.close();
    onDone(ok);
  };
  const timer = setTimeout(() => finish(false), SIGN_IN_TIMEOUT_MS);
  server.rpc.onNotification = (method, params) => {
    if (method === "account/login/completed" && isRecord(params)) finish(params.success === true);
  };
  try {
    const login = await server.rpc.request<{ authUrl: string }>("account/login/start", { type: "chatgpt" });
    openUrl(login.authUrl);
    return { url: login.authUrl };
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
  }
}
