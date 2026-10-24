import { randomBytes } from "node:crypto";
import { join } from "node:path";
import { connect, type Client } from "@tjommie/client";
import type { ProviderSetup } from "@tjommie/protocol";
import { afterEach, expect, it, vi } from "vitest";
import { AccountRuntime } from "./account-runtime.ts";
import type { CredentialStore } from "./account-credentials.ts";
import { startDaemon, type Daemon } from "./daemon.ts";
import type { ProviderEvent, ProviderSession, StartOptions } from "./providers/types.ts";
import { tempRoot } from "./testing.ts";

// ACL behavior has its own Windows tests. These RPC tests never alter account ACLs.
vi.mock("./windows-acl.ts", async (original) => ({ ...(await original<typeof import("./windows-acl.ts")>()),
  protectCodexData: () => ({ status: "ready", ready: true, reason: null }),
  protectCodexDataAsync: async () => ({ status: "ready", ready: true, reason: null }),
}));

const cleanup: (() => Promise<void>)[] = [];
afterEach(async () => { for (const close of cleanup.splice(0).reverse()) await close(); });
async function eventually<T>(read: () => Promise<T | undefined>): Promise<T> {
  for (let attempt = 0; attempt < 150; attempt++) {
    const value = await read();
    if (value !== undefined) return value;
    await new Promise((done) => setTimeout(done, 10));
  }
  throw new Error("Synthetic account integration did not settle.");
}
async function fixture(fixtureOptions: { logout?: () => Promise<void>; waitForExit?: () => Promise<void>; usageCostUsd?: number | null; limitFirstTurn?: boolean } = {}) {
  const root = tempRoot(), keys = new Map<string, string>(), envs: StartOptions[] = [];
  const credentials: CredentialStore = { reference: (id) => `synthetic/${id}`, read: async (id) => keys.get(id) ?? null,
    write: async (id, key) => { keys.set(id, key); }, delete: async (id) => { keys.delete(id); } };
  let sequence = 0;
  const daemon: Daemon = await startDaemon({ root, port: 0, auth: { mode: "production", token: randomBytes(32).toString("base64url") },
    createAccountsRuntime: (runtimeOptions) => new AccountRuntime({ ...runtimeOptions, credentials, logout: fixtureOptions.logout ?? (async () => {}), probeLimit: async () => null,
      probe: async (account): Promise<ProviderSetup> => ({ provider: account.provider, installed: true, version: account.provider === "claude" ? "2.1.304" : "0.160.0", signedIn: true, account: `${account.id}@example.test`, plan: "synthetic", error: null,
        ...(account.provider === "codex" ? { sandbox: "ready" as const } : {}) }) as ProviderSetup }),
    createSession: (_bot, options): ProviderSession => {
      envs.push(options);
      const sessionId = `synthetic-session-${++sequence}`;
      const limited = fixtureOptions.limitFirstTurn && sequence === 1;
      let listener: (event: ProviderEvent) => void = () => {};
      return { onEvent: (next) => { listener = next; }, start: async () => { listener({ type: "session", cliSessionId: sessionId }); },
        send: async (text) => {
          if (limited) {
            listener({ type: "limit", state: "reached", usedPercent: 100, resetsAt: null });
            listener({ type: "turn_done", ok: false, error: "Synthetic quota limit" });
            return;
          }
          listener({ type: "text", delta: text }); listener({ type: "text_end" });
          listener({ type: "usage", model: options.model, input: 10, cachedInput: 5, output: 2, reasoning: 1, costUsd: fixtureOptions.usageCostUsd === undefined ? 1 : fixtureOptions.usageCostUsd });
          listener({ type: "turn_done", ok: true, error: null });
        }, steer: async () => false, interrupt: async () => true, answer: () => {}, close: () => {}, waitForExit: fixtureOptions.waitForExit ?? (async () => {}), pid: () => undefined };
    },
  });
  let closing: Promise<void> | undefined;
  const close = (): Promise<void> => closing ??= daemon.close();
  cleanup.push(close);
  const client: Client = await connect(daemon.port, daemon.token);
  cleanup.push(() => client.close());
  await client.call("accounts.check", {});
  return { root, keys, credentials, client, envs, close };
}
const input = { name: "Synthetic", color: "blue", shape: "circle", provider: "claude", model: "sonnet", instructions: "Synthetic account tests", autonomy: "ask", browser: false } as const;
async function idle(client: Client, id: string) {
  await eventually(async () => (await client.call("bots.list", {})).find((bot) => bot.id === id && bot.state === "idle"));
}
async function replied(client: Client, id: string, text: string) {
  await eventually(async () => (await client.call("chat.history", { botId: id })).messages.find((message) => message.author === id && message.kind === "text" && message.status === "done" && message.body.includes(text)));
}

it("switches a bot to another subscription and attributes each completed turn to its actual account", async () => {
  const { client, envs } = await fixture();
  const other = await client.call("accounts.create", { provider: "claude", label: "Other", kind: "subscription" });
  await client.call("accounts.check", { id: other.id });
  const bot = await client.call("bots.create", input);
  await client.call("chat.send", { botId: bot.id, text: "First account" });
  await replied(client, bot.id, "First account");
  await idle(client, bot.id);
  await client.call("bots.update", { id: bot.id, patch: { accountId: other.id } });
  await client.call("chat.send", { botId: bot.id, text: "Other account" });
  await replied(client, bot.id, "Other account");
  await idle(client, bot.id);
  const usage = await client.call("usage.query", { from: new Date(Date.now() - 60_000).toISOString(), to: new Date(Date.now() + 60_000).toISOString(), botId: bot.id });
  expect(usage.byAccount.map((row) => row.accountId).sort()).toEqual([bot.accountId, other.id].sort());
  expect(usage.summary.records).toBe(2);
  expect(usage.summary.costs.apiValue.usd).toBe(2);
  expect(usage.summary.costs.apiSpend.knownRecords).toBe(0);
  expect(envs).toHaveLength(2);
  expect(envs[0].env.CLAUDE_CONFIG_DIR).not.toBe(envs[1].env.CLAUDE_CONFIG_DIR);
  expect(envs[1].resumeId).toBeNull();
  const history = await client.call("chat.history", { botId: bot.id });
  expect(history.messages.some((message) => message.body.includes("First account"))).toBe(true);
  expect(history.messages.some((message) => message.body.includes("Other account"))).toBe(true);
});

it("keeps the pin stable when choosing another provider default", async () => {
  const { client } = await fixture();
  const bot = await client.call("bots.create", input);
  const other = await client.call("accounts.create", { provider: "claude", label: "New default", kind: "subscription" });
  await client.call("accounts.update", { id: other.id, patch: { isDefault: true } });
  expect((await client.call("bots.list", {})).find((entry) => entry.id === bot.id)?.accountId).toBe(bot.accountId);
  expect((await client.call("bots.create", { ...input, name: "Next" })).accountId).toBe(other.id);
});

it("does not leak an API key in account results and preserves history metadata on removal", async () => {
  const { client, keys } = await fixture();
  const key = "synthetic-only-not-a-real-api-key";
  const account = await client.call("accounts.create", { provider: "claude", label: "Synthetic API", kind: "api_key", apiKey: key, spendingCapUsd: 2 });
  expect(keys.get(account.id)).toBe(key);
  expect(JSON.stringify(await client.call("accounts.list", {}))).not.toContain(key);
  expect(account).not.toHaveProperty("credentialRef");
  expect(account).not.toHaveProperty("folder");
  await client.call("accounts.remove", { id: account.id });
  expect(keys.has(account.id)).toBe(false);
  expect((await client.call("accounts.list", {})).some((entry) => entry.id === account.id)).toBe(false);
});

it("keeps a subscription visible after a failed local logout and allows retrying removal", async () => {
  let fail = true, attempts = 0;
  const { client } = await fixture({ logout: async () => { attempts++; if (fail) throw new Error("synthetic logout failure"); } });
  const account = await client.call("accounts.create", { provider: "claude", label: "Disposable subscription", kind: "subscription" });
  await expect(client.call("accounts.remove", { id: account.id })).rejects.toThrow();
  expect((await client.call("accounts.list", {})).some((entry) => entry.id === account.id)).toBe(true);
  fail = false;
  await client.call("accounts.remove", { id: account.id });
  expect(attempts).toBe(2);
  expect((await client.call("accounts.list", {})).some((entry) => entry.id === account.id)).toBe(false);
});

it("blocks another API-key turn at its lifetime cap, then resumes queued work after the cap is raised", async () => {
  const { client, envs } = await fixture();
  const account = await client.call("accounts.create", { provider: "claude", label: "Capped synthetic API", kind: "api_key", apiKey: "synthetic-cap-only", spendingCapUsd: 1 });
  await client.call("accounts.check", { id: account.id });
  const bot = await client.call("bots.create", { ...input, accountId: account.id });
  await client.call("chat.send", { botId: bot.id, text: "Use the first dollar" });
  await eventually(async () => (await client.call("accounts.list", {})).find((entry) => entry.id === account.id && entry.spending.blocked));
  await client.call("chat.send", { botId: bot.id, text: "Wait for a higher cap" });
  await new Promise((done) => setTimeout(done, 100));
  expect(envs).toHaveLength(1);
  await client.call("accounts.update", { id: account.id, patch: { spendingCapUsd: 3 } });
  await replied(client, bot.id, "Wait for a higher cap");
  expect(envs).toHaveLength(2);
  const report = await client.call("usage.query", { from: new Date(Date.now() - 60_000).toISOString(), to: new Date(Date.now() + 60_000).toISOString(), accountId: account.id });
  expect(report.summary.costs.apiSpend.usd).toBe(2);
  expect(report.summary.costs.apiValue.knownRecords).toBe(0);
});

it("waits for an owned account process to exit before daemon teardown closes its database", async () => {
  let exit: (() => void) | undefined;
  const exited = new Promise<void>((done) => { exit = done; });
  const { client, close } = await fixture({ waitForExit: () => exited });
  try {
    const bot = await client.call("bots.create", input);
    await client.call("chat.send", { botId: bot.id, text: "Synthetic process still alive" });
    await replied(client, bot.id, "Synthetic process still alive");
    let closed = false;
    const closing = close().then(() => { closed = true; });
    await new Promise((done) => setTimeout(done, 30));
    expect(closed).toBe(false);
    exit?.();
    await closing;
    expect(closed).toBe(true);
  } finally { exit?.(); }
});

it("reports unknown cost honestly instead of claiming the account spent 100 percent of its cap", async () => {
  const { client } = await fixture({ usageCostUsd: null });
  const notices: string[] = [];
  const off = client.subscribe((event) => { if (event.event === "notify") notices.push(event.data.title); });
  try {
    const account = await client.call("accounts.create", { provider: "claude", label: "Unpriced synthetic API", kind: "api_key", apiKey: "synthetic-unpriced-only", spendingCapUsd: 10 });
    await client.call("accounts.check", { id: account.id });
    const bot = await client.call("bots.create", { ...input, accountId: account.id });
    await client.call("chat.send", { botId: bot.id, text: "Synthetic missing price" });
    const blocked = await eventually(async () => (await client.call("accounts.list", {})).find((entry) => entry.id === account.id && entry.spending.unknownRows === 1));
    expect(blocked.spending.blocked).toBe(true);
    expect(notices).toContain("Unpriced synthetic API has unpriced usage");
    expect(notices.some((title) => title.includes("100%"))).toBe(false);
  } finally { off(); }
});

it("retries queued subscription work only after the explicit unknown-reset action", async () => {
  const { client, envs } = await fixture({ limitFirstTurn: true });
  const bot = await client.call("bots.create", input);
  if (!bot.accountId) throw new Error("Synthetic bot has no account pin.");
  await client.call("chat.send", { botId: bot.id, text: "Explicitly retry this queued work" });
  await eventually(async () => (await client.call("bots.list", {})).find((entry) => entry.id === bot.id && entry.state === "limited"));
  expect(envs).toHaveLength(1);
  await client.call("accounts.retryLimit", { id: bot.accountId });
  await replied(client, bot.id, "Explicitly retry this queued work");
  await eventually(async () => (await client.call("accounts.list", {})).find((entry) => entry.id === bot.accountId && !entry.limits.some((window) => window.state === "reached")));
  expect(envs).toHaveLength(2);
  const report = await client.call("usage.query", { from: new Date(Date.now() - 60_000).toISOString(), to: new Date(Date.now() + 60_000).toISOString(), botId: bot.id });
  expect(report.summary.costs.apiValue.usd).toBe(1);
  expect(report.summary.costs.apiSpend.knownRecords).toBe(0);
});
