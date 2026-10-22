import { createHash } from "node:crypto";
import { request as httpRequest } from "node:http";
import { afterEach, describe, expect, it, vi } from "vitest";
import { GoogleAuth, type GoogleHttpRequest, type GoogleHttpResponse, type GoogleTransport } from "./google-auth.ts";
import type { GoogleVault, GoogleVaultRecord } from "./google-vault.ts";
import type { GoogleAuthorization, GoogleService, GoogleStatus } from "../../protocol/src/google.ts";

const client = { clientId: "123-fixture.apps.googleusercontent.com", clientSecret: "synthetic-client-secret" };
const scopes = "openid email https://www.googleapis.com/auth/gmail.readonly";
const token = { access_token: "synthetic-access", refresh_token: "synthetic-refresh", expires_in: 3600, token_type: "Bearer", scope: scopes };
const identity = { sub: "fixture-subject", email: "fixture@example.test", email_verified: true };
const opened: GoogleAuth[] = [];
afterEach(async () => { await Promise.all(opened.splice(0).map((auth) => auth.close())); vi.unstubAllGlobals(); });
const deferred = <T>() => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
};
class MemoryVault implements GoogleVault {
  value: GoogleVaultRecord | null = null;
  load = vi.fn(async () => structuredClone(this.value));
  save = vi.fn(async (value: GoogleVaultRecord) => { this.value = structuredClone(value); });
  clear = vi.fn(async () => { this.value = null; });
}
async function fixture(options: { handle?: GoogleTransport; now?: () => number; consentTimeoutMs?: number; vault?: MemoryVault } = {}) {
  const vault = options.vault ?? new MemoryVault();
  const changes: GoogleStatus[] = [];
  const transport = vi.fn<GoogleTransport>(options.handle ?? (async (req) => {
    if (req.url.endsWith("/token")) return { status: 200, body: token };
    if (req.url.endsWith("/userinfo")) return { status: 200, body: identity };
    if (req.url.endsWith("/revoke")) return { status: 200, body: null };
    return { status: 200, body: { messages: [{ id: "fixture-message" }] } };
  }));
  const auth = await GoogleAuth.open({ vault, transport, onChange: (status) => changes.push(status), now: options.now, consentTimeoutMs: options.consentTimeoutMs });
  opened.push(auth);
  return { auth, vault, transport, changes };
}
function callbackUrl(authorization: GoogleAuthorization): URL {
  const url = new URL(authorization.url);
  const callback = new URL(url.searchParams.get("redirect_uri")!);
  callback.search = new URLSearchParams({ state: url.searchParams.get("state")!, code: "synthetic-code" }).toString();
  return callback;
}
async function callback(url: URL, options: { method?: string; host?: string; path?: string } = {}): Promise<{ status: number; body: string }> {
  return new Promise((resolve, reject) => {
    const req = httpRequest({ hostname: url.hostname, port: url.port, method: options.method ?? "GET", path: options.path ?? `${url.pathname}${url.search}`, headers: { host: options.host ?? url.host } }, (res) => {
      let body = ""; res.on("data", (part: Buffer) => { body += part.toString(); });
      res.on("end", () => resolve({ status: res.statusCode ?? 0, body }));
    });
    req.on("error", reject); req.end();
  });
}
async function connect(auth: GoogleAuth, services: GoogleService[] = ["gmail"]): Promise<GoogleAuthorization> {
  if (!auth.status().configured) await auth.configure(client);
  const authorization = await auth.begin(services);
  expect((await callback(callbackUrl(authorization))).status).toBe(200);
  await vi.waitFor(() => expect(auth.status().attempt).toBeNull());
  expect(auth.status().connected).toBe(true);
  return authorization;
}

describe("Google OAuth connection", () => {
  it("uses a fixed Google endpoint, loopback callback, PKCE and server-verified identity; persists no access token", async () => {
    const { auth, vault, transport, changes } = await fixture();
    const authorization = await connect(auth);
    const url = new URL(authorization.url);
    expect(url.origin + url.pathname).toBe("https://accounts.google.com/o/oauth2/v2/auth");
    expect(url.searchParams.get("scope")).toBe(scopes);
    expect(url.searchParams.get("code_challenge_method")).toBe("S256");
    expect(url.searchParams.get("access_type")).toBe("offline");
    expect(new URL(url.searchParams.get("redirect_uri")!).hostname).toBe("127.0.0.1");
    const exchange = transport.mock.calls.find(([request]) => request.url.endsWith("/token"))![0];
    const body = new URLSearchParams(exchange.body);
    expect(body.get("client_secret")).toBe(client.clientSecret);
    expect(createHash("sha256").update(body.get("code_verifier")!).digest("base64url")).toBe(url.searchParams.get("code_challenge"));
    expect(body.get("redirect_uri")).toBe(url.searchParams.get("redirect_uri"));
    expect(transport.mock.calls.some(([request]) => request.url === "https://openidconnect.googleapis.com/v1/userinfo")).toBe(true);
    expect(auth.status()).toEqual({ configured: true, connected: true, services: ["gmail"], grantedBotIds: [], email: identity.email, attempt: null });
    expect(vault.value?.connection?.refreshToken).toBe(token.refresh_token);
    expect(JSON.stringify(vault.value)).not.toContain(token.access_token);
    for (const secret of [token.access_token, token.refresh_token, client.clientSecret, "synthetic-code"]) expect(JSON.stringify(changes)).not.toContain(secret);
  });
  it("validates exact host, method, raw path, unique params and constant-time state without consuming the attempt", async () => {
    const { auth, transport } = await fixture();
    await auth.configure(client);
    const authorization = await auth.begin(["gmail"]), url = callbackUrl(authorization);
    const invalid = new URL(url); invalid.searchParams.set("state", "é".repeat(43));
    expect((await callback(invalid)).status).toBe(400);
    invalid.searchParams.set("state", "wrong");
    expect((await callback(invalid)).status).toBe(400);
    expect((await callback(url, { host: "localhost:" + url.port })).status).toBe(400);
    expect((await callback(url, { method: "POST" })).status).toBe(400);
    expect((await callback(url, { path: "/other/../oauth/google/callback" + url.search })).status).toBe(400);
    expect((await callback(url, { path: url.pathname + url.search + "&code=duplicate" })).status).toBe(400);
    expect((await callback(url, { path: url.pathname + url.search + "&error=access_denied" })).status).toBe(400);
    expect(transport).not.toHaveBeenCalled();
    expect((await callback(url)).status).toBe(200);
    await vi.waitFor(() => expect(auth.status().connected).toBe(true));
    expect(transport.mock.calls.filter(([req]) => req.url.endsWith("/token"))).toHaveLength(1);
  });
  it("consumes the valid callback exactly once while the exchange is in flight", async () => {
    const held = deferred<GoogleHttpResponse>();
    const { auth, transport } = await fixture({ handle: async () => held.promise });
    await auth.configure(client); const begin = await auth.begin(["gmail"]); const url = callbackUrl(begin);
    expect((await callback(url)).status).toBe(200);
    expect((await callback(url)).status).toBe(409);
    expect(transport).toHaveBeenCalledTimes(1);
    await auth.cancel(begin.attempt); held.resolve({ status: 200, body: token });
    expect(auth.status().connected).toBe(false);
  });
  it("cancels a pending exchange and discards its late credentials", async () => {
    const held = deferred<GoogleHttpResponse>();
    const { auth, vault } = await fixture({ handle: async () => held.promise });
    await auth.configure(client); const begin = await auth.begin(["gmail"]);
    await callback(callbackUrl(begin)); await auth.cancel(begin.attempt);
    held.resolve({ status: 200, body: token });
    await new Promise<void>((resolve) => setImmediate(resolve));
    expect(auth.status().attempt).toBeNull(); expect(auth.status().connected).toBe(false); expect(vault.value?.connection).toBeNull();
    await expect(callback(callbackUrl(begin))).rejects.toThrow();
  });
  it("times out consent and ignores cancellation of an older attempt", async () => {
    const { auth } = await fixture({ consentTimeoutMs: 50 });
    await auth.configure(client); const first = await auth.begin(["gmail"]); const second = await auth.begin(["gmail"]);
    await auth.cancel(first.attempt); expect(auth.status().attempt?.id).toBe(second.attempt);
    await vi.waitFor(() => expect(auth.status().attempt?.state).toBe("failed"));
    expect(auth.status().attempt?.message).toMatch(/timed out/);
    await expect(callback(callbackUrl(second))).rejects.toThrow();
  });
  it("rejects missing/partial granted scopes and forged ID-token-only identity", async () => {
    for (const response of [{ ...token, scope: "openid email" }, { ...token, scope: undefined }, { ...token, refresh_token: undefined }]) {
      const { auth } = await fixture({ handle: async () => ({ status: 200, body: response }) });
      await auth.configure(client); const begin = await auth.begin(["gmail"]); await callback(callbackUrl(begin));
      await vi.waitFor(() => expect(auth.status().attempt?.state).toBe("failed"));
      expect(auth.status().connected).toBe(false); expect(auth.status().services).toEqual([]);
    }
    const { auth } = await fixture({ handle: async (req) => ({ status: 200, body: req.url.endsWith("/token") ? { ...token, id_token: "unverified" } : { email: identity.email } }) });
    await auth.configure(client); const begin = await auth.begin(["gmail"]); await callback(callbackUrl(begin));
    await vi.waitFor(() => expect(auth.status().attempt?.state).toBe("failed"));
    expect(auth.status().connected).toBe(false);
  });
  it("preserves an established connection after failed/declined consent and forbids connected reconfiguration", async () => {
    const { auth, transport } = await fixture(); await connect(auth); await auth.setGrant("one", true);
    await expect(auth.configure({ ...client, clientSecret: "replacement" })).rejects.toThrow(/Disconnect/);
    const begin = await auth.begin(["gmail"]), url = callbackUrl(begin); url.searchParams.delete("code"); url.searchParams.set("error", "secret-provider-error");
    await callback(url); await vi.waitFor(() => expect(auth.status().attempt?.state).toBe("failed"));
    expect(auth.status().connected).toBe(true); expect(auth.status().grantedBotIds).toEqual(["one"]);
    expect(JSON.stringify(auth.status())).not.toContain("secret-provider-error");
    expect(transport.mock.calls.filter(([req]) => req.url.endsWith("/token"))).toHaveLength(1);
  });
  it("clears grants when the verified account or authorized service set changes", async () => {
    const { auth, transport } = await fixture(); await connect(auth); await auth.setGrant("one", true);
    transport.mockImplementation(async (req) => ({ status: 200, body: req.url.endsWith("/token") ? { ...token, scope: scopes + " https://www.googleapis.com/auth/drive.readonly" } : { ...identity, sub: "different-subject" } }));
    await connect(auth); expect(auth.status().grantedBotIds).toEqual([]);
    await auth.setGrant("one", true); await connect(auth, ["gmail", "drive"]);
    expect(auth.status().grantedBotIds).toEqual([]); expect(auth.status().services).toEqual(["drive", "gmail"]);
  });
  it("does not overwrite an unreadable vault until explicit Disconnect", async () => {
    const vault = new MemoryVault(); vault.load.mockRejectedValue(new Error("secret-decryption-diagnostic"));
    const { auth } = await fixture({ vault });
    expect(auth.status().attempt?.message).not.toContain("secret-decryption-diagnostic");
    await expect(auth.configure(client)).rejects.toThrow(/Disconnect/); expect(vault.save).not.toHaveBeenCalled();
    const result = await auth.disconnect(); expect(vault.clear).toHaveBeenCalledOnce(); expect(result.attempt?.message).toMatch(/Remote consent/);
    await auth.configure(client); expect(auth.status().configured).toBe(true);
  });
});

describe("Google read access lifecycle", () => {
  it("requires a connected scope and explicit bot grant; persists grants across restart", async () => {
    const { auth, vault } = await fixture();
    await expect(auth.setGrant("one", true)).rejects.toThrow(/Connect/);
    await connect(auth); await expect(auth.request("one", "gmail", "/gmail/v1/users/me/messages")).rejects.toThrow(/access/);
    await auth.setGrant("one", true);
    expect(await auth.request("one", "gmail", "/gmail/v1/users/me/messages?maxResults=10")).toEqual({ messages: [{ id: "fixture-message" }] });
    await expect(auth.request("one", "drive", "/drive/v3/files")).rejects.toThrow(/access/);
    const second = await fixture({ vault }); expect(second.auth.status().grantedBotIds).toEqual(["one"]);
    await second.auth.request("one", "gmail", "/gmail/v1/users/me/messages");
    expect(new URLSearchParams(second.transport.mock.calls[0][0].body).get("grant_type")).toBe("refresh_token");
    await second.auth.unregisterBot("one"); expect(second.auth.status().grantedBotIds).toEqual([]);
  });
  it("rejects foreign hosts, traversal, token queries, media and write routes before transport", async () => {
    const { auth, transport } = await fixture(); await connect(auth); await auth.setGrant("one", true); transport.mockClear();
    for (const path of ["https://evil.test/", "//evil.test/x", "/gmail/v1/users/me/messages/../send", "/gmail/v1/users/me/messages/send/../../drafts", "/gmail/v1/users/me/messages%2fsecret", "/gmail/v1/users/me/messages?access_token=secret", "/gmail/v1/users/me/messages?alt=media", "/gmail/v1/users/me/messages#x", "/gmail/v1/users/me/messages\\x", "/gmail/v1/users/me/drafts", "/gmail/v1/users/me/messages/one/modify"]) {
      await expect(auth.request("one", "gmail", path)).rejects.toThrow(/Unsupported/);
    }
    expect(transport).not.toHaveBeenCalled();
    await auth.request("one", "gmail", "/gmail/v1/users/me/messages/one?format=metadata");
    expect(transport.mock.calls[0][0]).toMatchObject({ method: "GET", url: "https://gmail.googleapis.com/gmail/v1/users/me/messages/one?format=metadata" });
  });
  it("serializes simultaneous refresh and checks bot revocation before releasing requests", async () => {
    let time = 1_000;
    const { auth, transport } = await fixture({ now: () => time }); await connect(auth); await auth.setGrant("one", true); await auth.setGrant("two", true);
    time += 3_600_000;
    const held = deferred<GoogleHttpResponse>();
    transport.mockImplementation(async (req) => req.url.endsWith("/token") ? held.promise : { status: 200, body: { fixture: true } });
    transport.mockClear();
    const first = auth.request("one", "gmail", "/gmail/v1/users/me/messages");
    const denied = expect(first).rejects.toThrow(/access/);
    const second = auth.request("two", "gmail", "/gmail/v1/users/me/messages");
    await vi.waitFor(() => expect(transport).toHaveBeenCalledTimes(1));
    await auth.setGrant("one", false); held.resolve({ status: 200, body: { ...token, refresh_token: "rotated-synthetic" } });
    await denied; expect(await second).toEqual({ fixture: true });
    expect(transport.mock.calls.filter(([req]) => req.url.endsWith("/token"))).toHaveLength(1);
    expect(transport.mock.calls.filter(([req]) => req.method === "GET")).toHaveLength(1);
  });
  it("withholds an already-requested read response after its bot grant is removed", async () => {
    const { auth, transport } = await fixture(); await connect(auth); await auth.setGrant("one", true);
    const held = deferred<GoogleHttpResponse>(); transport.mockClear(); transport.mockImplementation(async () => held.promise);
    const request = auth.request("one", "gmail", "/gmail/v1/users/me/messages"); const denied = expect(request).rejects.toThrow(/access/);
    await vi.waitFor(() => expect(transport).toHaveBeenCalledTimes(1));
    await auth.unregisterBot("one"); held.resolve({ status: 200, body: { private: "fixture" } }); await denied;
  });
  it("does not resurrect credentials when refresh returns after disconnect", async () => {
    let time = 0;
    const { auth, transport, vault } = await fixture({ now: () => time }); await connect(auth); await auth.setGrant("one", true); time += 3_600_000;
    const held = deferred<GoogleHttpResponse>(); transport.mockImplementation(async (req) => req.url.endsWith("/token") ? held.promise : { status: 200, body: null });
    const read = auth.request("one", "gmail", "/gmail/v1/users/me/messages"); const denied = expect(read).rejects.toThrow(/access/);
    await auth.disconnect(); held.resolve({ status: 200, body: token }); await denied;
    expect(vault.value?.connection).toBeNull(); expect(auth.status().connected).toBe(false);
  });
  it("turns revoked refresh credentials into a disconnected status without exposing Google errors", async () => {
    let time = 0;
    const { auth, transport, vault } = await fixture({ now: () => time }); await connect(auth); await auth.setGrant("one", true); time += 3_600_000;
    transport.mockResolvedValue({ status: 400, body: { error: "invalid_grant", error_description: "secret-token-and-account" } });
    await expect(auth.request("one", "gmail", "/gmail/v1/users/me/messages")).rejects.toThrow(/expired or was revoked/);
    expect(auth.status().connected).toBe(false); expect(auth.status().grantedBotIds).toEqual([]); expect(vault.value?.connection).toBeNull();
    expect(JSON.stringify(auth.status())).not.toContain("secret-token-and-account");
  });
  it("clears local access when remote revocation fails and keeps the failure message redacted", async () => {
    const { auth, transport, vault } = await fixture(); await connect(auth); await auth.setGrant("one", true);
    transport.mockRejectedValue(new Error("synthetic-refresh secret-upstream-response"));
    const status = await auth.disconnect();
    expect(status.connected).toBe(false); expect(status.grantedBotIds).toEqual([]); expect(vault.value?.connection).toBeNull();
    expect(status.attempt?.message).toMatch(/Remote revocation could not be confirmed/);
    expect(JSON.stringify(status)).not.toContain("synthetic-refresh");
    await expect(auth.request("one", "gmail", "/gmail/v1/users/me/messages")).rejects.toThrow(/access/);
  });
  it("fails closed on disconnect persistence errors and allows a later explicit cleanup retry", async () => {
    const { auth, vault } = await fixture(); await connect(auth); await auth.setGrant("one", true);
    vault.save.mockRejectedValueOnce(new Error("private-path-and-token"));
    const result = await auth.disconnect();
    expect(result.connected).toBe(false); expect(result.grantedBotIds).toEqual([]); expect(result.attempt?.message).toMatch(/saved settings could not be removed/);
    expect(JSON.stringify(result)).not.toContain("private-path-and-token");
    await expect(auth.configure(client)).rejects.toThrow(/Disconnect/);
    await auth.disconnect(); expect(vault.value?.connection).toBeNull(); await auth.configure(client);
  });
  it("never replaces a working connection when saving reconnect credentials fails", async () => {
    const { auth, vault, transport } = await fixture(); await connect(auth); await auth.setGrant("one", true);
    const previous = structuredClone(vault.value);
    transport.mockImplementation(async (req) => ({ status: 200, body: req.url.endsWith("/token") ? { ...token, refresh_token: "new-same-account-refresh" } : req.url.endsWith("/userinfo") ? identity : { messages: [] } }));
    vault.save.mockRejectedValueOnce(new Error("private-persistence-diagnostic"));
    const begin = await auth.begin(["gmail"]); await callback(callbackUrl(begin));
    await vi.waitFor(() => expect(auth.status().attempt?.state).toBe("failed"));
    expect(auth.status().connected).toBe(true); expect(auth.status().grantedBotIds).toEqual(["one"]);
    expect(auth.status().attempt?.message).toMatch(/encrypted settings could not be saved/);
    expect(auth.status().attempt?.message).toMatch(/cannot be revoked separately/);
    expect(JSON.stringify(auth.status())).not.toContain("private-persistence-diagnostic");
    expect(vault.value).toEqual(previous);
    expect(transport.mock.calls.some(([request]) => request.url.endsWith("/revoke"))).toBe(false);
    await expect(auth.request("one", "gmail", "/gmail/v1/users/me/messages")).resolves.toEqual({ messages: [] });
    expect(transport.mock.calls.at(-1)?.[0].headers.authorization).toBe(`Bearer ${token.access_token}`);
  });
  it("revokes an unsaved first connection through the fixed endpoint without exposing its credentials", async () => {
    const { auth, vault, transport, changes } = await fixture();
    await auth.configure(client); vault.save.mockRejectedValueOnce(new Error("synthetic-refresh private-save-error"));
    const begin = await auth.begin(["gmail"]); await callback(callbackUrl(begin));
    await vi.waitFor(() => expect(auth.status().attempt?.state).toBe("failed"));
    const revocations = transport.mock.calls.filter(([request]) => request.url.endsWith("/revoke"));
    expect(revocations).toHaveLength(1);
    expect(revocations[0][0]).toMatchObject({ url: "https://oauth2.googleapis.com/revoke", method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" } });
    expect(new URLSearchParams(revocations[0][0].body).get("token")).toBe(token.refresh_token);
    expect(auth.status().connected).toBe(false); expect(vault.value?.connection).toBeNull();
    expect(auth.status().attempt?.message).toMatch(/accepted revocation/);
    for (const secret of [token.refresh_token, token.access_token, client.clientSecret, "private-save-error"]) expect(JSON.stringify(changes)).not.toContain(secret);
  });
  it.each(["rejected", "failed"] as const)("keeps an unsaved first connection local-only when cleanup is %s, with a redacted warning", async (mode) => {
    const { auth, vault, transport, changes } = await fixture();
    const normal = transport.getMockImplementation()!;
    transport.mockImplementation(async (req) => {
      if (!req.url.endsWith("/revoke")) return normal(req);
      if (mode === "failed") throw new Error("synthetic-refresh secret-upstream-error");
      return { status: 400, body: { error: "synthetic-refresh secret-upstream-error" } };
    });
    await auth.configure(client); vault.save.mockRejectedValueOnce(new Error("private-save-error"));
    const begin = await auth.begin(["gmail"]); await callback(callbackUrl(begin));
    await vi.waitFor(() => expect(auth.status().attempt?.state).toBe("failed"));
    expect(auth.status().connected).toBe(false); expect(vault.value?.connection).toBeNull();
    expect(auth.status().attempt?.message).toMatch(/could not be confirmed revoked/);
    expect(JSON.stringify(changes)).not.toContain("synthetic-refresh"); expect(JSON.stringify(changes)).not.toContain("secret-upstream-error");
  });
  it("revokes only the unsaved different account and preserves the prior account and bot grants", async () => {
    const { auth, vault, transport } = await fixture(); await connect(auth); await auth.setGrant("one", true);
    const previous = structuredClone(vault.value);
    transport.mockImplementation(async (req) => ({ status: 200, body: req.url.endsWith("/token") ? { ...token, refresh_token: "new-account-refresh" } : req.url.endsWith("/userinfo") ? { ...identity, sub: "different-account", email: "different@example.test" } : null }));
    vault.save.mockRejectedValueOnce(new Error("private-save-error"));
    const begin = await auth.begin(["gmail"]); await callback(callbackUrl(begin));
    await vi.waitFor(() => expect(auth.status().attempt?.state).toBe("failed"));
    const revocations = transport.mock.calls.filter(([request]) => request.url.endsWith("/revoke"));
    expect(revocations).toHaveLength(1); expect(new URLSearchParams(revocations[0][0].body).get("token")).toBe("new-account-refresh");
    expect(vault.value).toEqual(previous); expect(auth.status()).toMatchObject({ connected: true, email: identity.email, services: ["gmail"], grantedBotIds: ["one"] });
  });
  it("never revokes a token identical to the saved token even if a different identity is reported", async () => {
    const { auth, vault, transport } = await fixture(); await connect(auth);
    transport.mockImplementation(async (req) => ({ status: 200, body: req.url.endsWith("/token") ? token : { ...identity, sub: "contradictory-account" } }));
    vault.save.mockRejectedValueOnce(new Error("private-save-error"));
    const begin = await auth.begin(["gmail"]); await callback(callbackUrl(begin));
    await vi.waitFor(() => expect(auth.status().attempt?.state).toBe("failed"));
    expect(transport.mock.calls.some(([request]) => request.url.endsWith("/revoke"))).toBe(false);
    expect(auth.status().connected).toBe(true);
  });
  it("uses no redirects or URL credentials in the production HTTP transport and rejects oversized results", async () => {
    const vault = new MemoryVault();
    vault.value = { version: 1, client, connection: { refreshToken: token.refresh_token, subject: identity.sub, email: identity.email, scopes: scopes.split(" "), services: ["gmail"] }, grantedBotIds: ["one"] };
    const fetcher = vi.fn<typeof fetch>(async () => new Response(JSON.stringify(token), { status: 200 }));
    vi.stubGlobal("fetch", fetcher);
    const auth = await GoogleAuth.open({ vault }); opened.push(auth);
    fetcher.mockResolvedValueOnce(new Response(JSON.stringify(token), { status: 200 }));
    fetcher.mockResolvedValueOnce(new Response(new Uint8Array(1024 * 1024 + 1), { status: 200 }));
    await expect(auth.request("one", "gmail", "/gmail/v1/users/me/messages")).rejects.toThrow(/Google did not complete/);
    expect(fetcher.mock.calls).toHaveLength(2);
    expect(fetcher.mock.calls[0][0]).toBe("https://oauth2.googleapis.com/token");
    for (const [url, options] of fetcher.mock.calls) {
      expect(options?.redirect).toBe("error"); expect(options?.signal).toBeInstanceOf(AbortSignal);
      expect(String(url)).not.toContain(token.access_token); expect(String(url)).not.toContain(token.refresh_token);
    }
  });
  it("bounds a transport that does not respond and aborts owned requests when closing", async () => {
    const vault = new MemoryVault();
    vault.value = { version: 1, client, connection: { refreshToken: token.refresh_token, subject: identity.sub, email: identity.email, scopes: scopes.split(" "), services: ["gmail"] }, grantedBotIds: ["one"] };
    const transport = vi.fn<GoogleTransport>(async () => new Promise<GoogleHttpResponse>(() => {}));
    const auth = await GoogleAuth.open({ vault, transport, requestTimeoutMs: 30 }); opened.push(auth);
    await expect(auth.request("one", "gmail", "/gmail/v1/users/me/messages")).rejects.toThrow(/did not complete/);
    const read = auth.request("one", "gmail", "/gmail/v1/users/me/messages"); const result = expect(read).rejects.toThrow(/did not complete/);
    await vi.waitFor(() => expect(transport).toHaveBeenCalledTimes(2)); await auth.close(); await result;
    expect(transport.mock.calls.at(-1)?.[0].signal.aborted).toBe(true);
  });
});
