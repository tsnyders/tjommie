import { createHash, randomBytes, randomUUID, timingSafeEqual } from "node:crypto";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { z } from "zod";
import { GoogleClientConfig, GoogleServices, type GoogleAuthorization, type GoogleService, type GoogleStatus } from "../../protocol/src/google.ts";
import { GoogleVaultRecord, type GoogleVault } from "./google-vault.ts";
import { RpcError } from "./server.ts";

const AUTHORIZATION = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN = "https://oauth2.googleapis.com/token";
const USERINFO = "https://openidconnect.googleapis.com/v1/userinfo";
const REVOKE = "https://oauth2.googleapis.com/revoke";
const CALLBACK = "/oauth/google/callback";
const SCOPES: Record<GoogleService, string> = {
  gmail: "https://www.googleapis.com/auth/gmail.readonly",
  drive: "https://www.googleapis.com/auth/drive.readonly",
  calendar: "https://www.googleapis.com/auth/calendar.readonly",
};
const TokenResponse = z.object({
  access_token: z.string().min(1).max(16_384), token_type: z.string().refine((value) => value.toLowerCase() === "bearer"),
  expires_in: z.number().int().positive().max(86_400), refresh_token: z.string().min(1).max(16_384).optional(),
  scope: z.string().max(16_384).optional(),
});
const Identity = z.object({ sub: z.string().min(1).max(255), email: z.string().email().max(320).optional(), email_verified: z.boolean().optional() });
export type GoogleHttpRequest = { url: string; method: "GET" | "POST"; headers: Record<string, string>; body?: string; signal: AbortSignal };
export type GoogleHttpResponse = { status: number; body: unknown };
export type GoogleTransport = (request: GoogleHttpRequest) => Promise<GoogleHttpResponse>;
type Options = { vault: GoogleVault; onChange?: (status: GoogleStatus) => void; transport?: GoogleTransport; consentTimeoutMs?: number; requestTimeoutMs?: number; now?: () => number };
type Pending = { id: string; state: string; verifier: string; server: Server; redirect: string; services: GoogleService[]; controller: AbortController; timer?: NodeJS.Timeout; used: boolean };
type Access = { value: string; expires: number };
const problem = (message: string): RpcError => new RpcError("bad_state", message);

// No redirects: neither bearer tokens nor client secrets may follow a Google redirect elsewhere.
const network: GoogleTransport = async (request) => {
  const response = await fetch(request.url, { method: request.method, headers: request.headers, body: request.body, signal: request.signal, redirect: "error" });
  const chunks: Uint8Array[] = [];
  let size = 0;
  if (response.body) {
    const reader = response.body.getReader();
    try {
      for (;;) {
        const next = await reader.read();
        if (next.done) break;
        size += next.value.byteLength;
        if (size > 1024 * 1024) { await reader.cancel(); throw problem("Google returned too much data. Request a smaller result."); }
        chunks.push(next.value);
      }
    } finally { reader.releaseLock(); }
  }
  const text = Buffer.concat(chunks).toString("utf8");
  return { status: response.status, body: text ? JSON.parse(text) as unknown : null };
};

function apiUrl(service: GoogleService, path: string): string {
  // GET-only metadata/read routes. No arbitrary host, upload/download, batch, export, or write API.
  if (path.length > 8192 || !path.startsWith("/") || /[\\#\r\n]/.test(path)) throw problem("Unsupported Google read request.");
  const raw = path.split("?")[0];
  const patterns: Record<GoogleService, RegExp> = {
    gmail: /^\/gmail\/v1\/users\/me\/(?:messages(?:\/[a-zA-Z0-9_-]+)?|threads(?:\/[a-zA-Z0-9_-]+)?|labels(?:\/[a-zA-Z0-9_-]+)?)$/,
    drive: /^\/drive\/v3\/files(?:\/[a-zA-Z0-9_-]+)?$/,
    calendar: /^\/calendar\/v3\/(?:users\/me\/calendarList|calendars\/[^/]+\/events(?:\/[^/]+)?)$/,
  };
  if (!patterns[service]?.test(raw) || /%(?:2f|5c|2e|00)/i.test(raw)) throw problem("Unsupported Google read request.");
  const origins: Record<GoogleService, string> = { gmail: "https://gmail.googleapis.com", drive: "https://www.googleapis.com", calendar: "https://www.googleapis.com" };
  const url = new URL(path, origins[service]);
  if (url.origin !== origins[service] || url.pathname !== raw || [...url.searchParams.keys()].some((key) => ["access_token", "oauth_token", "key", "alt", "callback"].includes(key))) throw problem("Unsupported Google read request.");
  return url.href;
}

export class GoogleAuth {
  readonly #options: Options;
  #record: GoogleVaultRecord | null = null;
  #loadFailed = false;
  #pending: Pending | null = null;
  #attempt: GoogleStatus["attempt"] = null;
  #access: Access | null = null;
  #refresh: Promise<void> | null = null;
  #epoch = 0;
  #closed = false;
  #disconnecting = false;
  #tail: Promise<unknown> = Promise.resolve();
  #requests = new Set<AbortController>();
  private constructor(options: Options) { this.#options = options; }

  static async open(options: Options): Promise<GoogleAuth> {
    const auth = new GoogleAuth(options);
    try {
      const record = await options.vault.load();
      auth.#record = record === null ? null : GoogleVaultRecord.parse(record);
      const connection = auth.#record?.connection;
      if (connection && connection.services.some((service) => !connection.scopes.includes(SCOPES[service]))) throw problem("Invalid stored scope.");
    } catch {
      auth.#record = null; auth.#loadFailed = true;
      auth.#attempt = { id: "load", state: "failed", message: "Google's encrypted settings could not be read. Disconnect to remove local settings, then configure again. Remote consent will need removing in your Google Account." };
    }
    return auth;
  }
  status(): GoogleStatus {
    const connection = this.#closed || this.#loadFailed ? null : this.#record?.connection;
    return { configured: !!this.#record && !this.#loadFailed, connected: !!connection, email: connection?.email ?? null,
      services: [...(connection?.services ?? [])], grantedBotIds: connection ? [...this.#record!.grantedBotIds] : [],
      attempt: this.#attempt ? { ...this.#attempt } : null };
  }
  #notify(): void { try { this.#options.onChange?.(this.status()); } catch { /* observers cannot roll back credential persistence */ } }
  #serial<T>(run: () => Promise<T>): Promise<T> {
    const next = this.#tail.then(run); this.#tail = next.catch(() => {}); return next;
  }
  #ready(): void {
    if (this.#closed) throw problem("The Google connection is closed.");
    if (this.#loadFailed) throw problem("Google's encrypted settings are unavailable. Disconnect before configuring again.");
    if (this.#disconnecting) throw new RpcError("busy", "Google is disconnecting. Try again shortly.");
  }
  async #save(record: GoogleVaultRecord): Promise<void> {
    try { await this.#options.vault.save(record); }
    catch { throw problem("Google's encrypted settings could not be saved. Check the app's data folder and try again."); }
    this.#record = record;
  }
  #stopPending(): void {
    const pending = this.#pending; this.#pending = null;
    if (!pending) return;
    clearTimeout(pending.timer); pending.controller.abort();
    pending.server.close(); pending.server.closeAllConnections();
  }
  configure(config: GoogleClientConfig): Promise<GoogleStatus> {
    return this.#serial(async () => {
      this.#ready();
      if (this.#record?.connection) throw problem("Disconnect Google before changing the client configuration.");
      const parsed = GoogleClientConfig.safeParse(config);
      if (!parsed.success) throw new RpcError("bad_params", "Enter the client ID and secret from a Google Desktop app client.");
      this.#stopPending(); this.#epoch++; this.#access = null;
      await this.#save({ version: 1, client: parsed.data, connection: null, grantedBotIds: [] });
      this.#attempt = null; this.#notify(); return this.status();
    });
  }
  begin(services: GoogleService[]): Promise<GoogleAuthorization> {
    return this.#serial(async () => {
      this.#ready();
      if (!this.#record) throw problem("Configure a Google Desktop app client first.");
      const parsed = GoogleServices.safeParse(services);
      if (!parsed.success) throw new RpcError("bad_params", "Choose supported Google services once each.");
      this.#stopPending();
      const pending: Pending = { id: randomUUID(), state: randomBytes(32).toString("base64url"), verifier: randomBytes(48).toString("base64url"),
        server: createServer(), redirect: "", services: [...parsed.data].sort(), controller: new AbortController(), used: false };
      pending.server.headersTimeout = 5000; pending.server.requestTimeout = 5000; pending.server.maxHeadersCount = 30;
      pending.server.on("request", (req, res) => { void this.#callback(pending, req, res); });
      try {
        await new Promise<void>((resolve, reject) => { pending.server.once("error", reject); pending.server.listen(0, "127.0.0.1", () => { pending.server.removeListener("error", reject); resolve(); }); });
      } catch { pending.server.close(); throw problem("Google sign-in could not open its local callback. Try again."); }
      pending.server.on("error", () => { void this.#fail(pending, "Google sign-in's local callback closed. Try again."); });
      const address = pending.server.address();
      if (!address || typeof address === "string") { pending.server.close(); throw problem("Google sign-in could not open its local callback."); }
      pending.redirect = `http://127.0.0.1:${address.port}${CALLBACK}`;
      this.#pending = pending; this.#attempt = { id: pending.id, state: "waiting" };
      pending.timer = setTimeout(() => { void this.#fail(pending, "Google sign-in timed out. Start again when you are ready."); }, this.#options.consentTimeoutMs ?? 5 * 60_000);
      pending.timer.unref(); pending.server.unref();
      const url = new URL(AUTHORIZATION);
      url.search = new URLSearchParams({ client_id: this.#record.client.clientId, redirect_uri: pending.redirect, response_type: "code",
        scope: ["openid", "email", ...pending.services.map((service) => SCOPES[service])].join(" "), state: pending.state,
        code_challenge: createHash("sha256").update(pending.verifier).digest("base64url"), code_challenge_method: "S256", access_type: "offline", prompt: "consent select_account" }).toString();
      this.#notify(); return { attempt: pending.id, url: url.href };
    });
  }
  cancel(attempt: string): Promise<GoogleStatus> {
    return this.#serial(async () => {
      if (this.#pending?.id === attempt) this.#stopPending();
      if (this.#attempt?.id === attempt) this.#attempt = null;
      this.#notify(); return this.status();
    });
  }
  async #fail(pending: Pending, message: string): Promise<void> {
    await this.#serial(async () => {
      if (this.#pending !== pending) return;
      this.#stopPending(); this.#attempt = { id: pending.id, state: "failed", message }; this.#notify();
    });
  }
  async #send(request: Omit<GoogleHttpRequest, "signal">, parent?: AbortSignal): Promise<GoogleHttpResponse> {
    const controller = new AbortController(); this.#requests.add(controller);
    const signal = parent ? AbortSignal.any([controller.signal, parent]) : controller.signal;
    const timer = setTimeout(() => controller.abort(), this.#options.requestTimeoutMs ?? 15_000);
    const aborted = (): RpcError => problem("Google did not complete the request. Try again.");
    let listener: () => void = () => {};
    try {
      if (signal.aborted) throw aborted();
      const stop = new Promise<never>((_resolve, reject) => { listener = () => reject(aborted()); signal.addEventListener("abort", listener, { once: true }); });
      return await Promise.race([(this.#options.transport ?? network)({ ...request, signal }), stop]);
    } catch { throw aborted(); }
    finally { clearTimeout(timer); signal.removeEventListener("abort", listener); this.#requests.delete(controller); }
  }
  #post(url: string, values: Record<string, string>, parent?: AbortSignal): Promise<GoogleHttpResponse> {
    return this.#send({ url, method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" }, body: new URLSearchParams(values).toString() }, parent);
  }
  async #callback(pending: Pending, req: IncomingMessage, res: ServerResponse): Promise<void> {
    const respond = (code: number, text: string): void => {
      if (!res.destroyed) res.writeHead(code, { "content-type": "text/plain; charset=utf-8", "cache-control": "no-store", "referrer-policy": "no-referrer", "content-security-policy": "default-src 'none'", connection: "close" }).end(text);
    };
    const raw = req.url ?? "";
    const hosts = req.rawHeaders.filter((_value, index) => index % 2 === 0 && req.rawHeaders[index].toLowerCase() === "host");
    if (this.#pending !== pending || pending.used) { respond(409, "This sign-in attempt is no longer available."); return; }
    if (req.method !== "GET" || hosts.length !== 1 || req.headers.host !== new URL(pending.redirect).host || raw.length > 8192 || raw.split("?")[0] !== CALLBACK || raw.includes("#")) { respond(400, "Invalid sign-in callback."); return; }
    const params = new URL(raw, pending.redirect).searchParams;
    const state = params.get("state") ?? "";
    const code = params.get("code"), error = params.get("error");
    if ([...params.keys()].some((key) => params.getAll(key).length !== 1) || Buffer.byteLength(state) !== Buffer.byteLength(pending.state) || !timingSafeEqual(Buffer.from(state), Buffer.from(pending.state)) ||
        (!!code === !!error) || (code !== null && (code.length > 4096 || /[\r\n\0]/.test(code)))) { respond(400, "Invalid sign-in callback."); return; }
    pending.used = true;
    // Acknowledge before async exchange; no Google-supplied strings or credentials enter this page.
    respond(200, "Return to Tjommie to see whether Google connected. You can close this tab.");
    if (error) { await this.#fail(pending, "Google sign-in was declined or could not finish. Try again when ready."); return; }
    const client = this.#record?.client;
    if (!client) { await this.#fail(pending, "Google client settings changed. Start sign-in again."); return; }
    try {
      const response = await this.#post(TOKEN, { grant_type: "authorization_code", code: code!, client_id: client.clientId, client_secret: client.clientSecret, redirect_uri: pending.redirect, code_verifier: pending.verifier }, pending.controller.signal);
      const parsed = TokenResponse.safeParse(response.body);
      if (response.status !== 200 || !parsed.success || !parsed.data.refresh_token || !parsed.data.scope) throw problem("Google did not provide offline read access. Review consent and try again.");
      const token = parsed.data;
      const scopes = token.scope!.split(/\s+/);
      if (!scopes.includes("openid") || !(scopes.includes("email") || scopes.includes("https://www.googleapis.com/auth/userinfo.email")) || pending.services.some((service) => !scopes.includes(SCOPES[service]))) throw problem("Google did not grant all requested read permissions. Review consent and try again.");
      const user = await this.#send({ url: USERINFO, method: "GET", headers: { authorization: `Bearer ${token.access_token}` } }, pending.controller.signal);
      const identity = Identity.safeParse(user.body);
      if (user.status !== 200 || !identity.success) throw problem("Google's account identity could not be verified. Try connecting again.");
      await this.#serial(async () => {
        if (this.#pending !== pending || this.#closed) return;
        const previous = this.#record?.connection;
        const sameGrant = previous?.subject === identity.data.sub && JSON.stringify(previous.services) === JSON.stringify(pending.services);
        try {
          await this.#save({ version: 1, client, connection: { refreshToken: token.refresh_token!, subject: identity.data.sub,
            email: identity.data.email_verified === true ? identity.data.email ?? null : null, scopes: [...new Set(scopes)], services: pending.services },
            grantedBotIds: sameGrant ? [...(this.#record?.grantedBotIds ?? [])] : [] });
        } catch {
          clearTimeout(pending.timer);
          let message = "Google's encrypted settings could not be saved. Check the app's data folder and try again.";
          // Google revocation removes the user's project-wide grant, not just one token.
          // Never invalidate a working same-account connection to clean up a failed reconnect.
          if (previous && (previous.subject === identity.data.sub || previous.refreshToken === token.refresh_token)) {
            message += " Your existing connection was kept; new consent for this account cannot be revoked separately. Disconnect Google to revoke the account's consent.";
          } else {
            // Stay in the mutation queue until bounded cleanup finishes: a later connection must
            // not be accepted and then invalidated by this attempt's delayed revocation.
            try {
              if ((await this.#post(REVOKE, { token: token.refresh_token! })).status !== 200) throw problem("Revocation failed.");
              message += " Google accepted revocation of the new consent.";
            } catch {
              message += " The new consent could not be confirmed revoked; remove app access for that account in your Google Account settings.";
            }
          }
          throw problem(message);
        }
        this.#epoch++; this.#refresh = null; this.#access = { value: token.access_token, expires: this.#now() + token.expires_in * 1000 };
        this.#stopPending(); this.#attempt = null; this.#notify();
      });
    } catch (error) {
      await this.#fail(pending, error instanceof RpcError ? error.message : "Google sign-in could not finish. Check the client settings and try again.");
    }
  }
  #now(): number { return this.#options.now?.() ?? Date.now(); }
  setGrant(botId: string, enabled: boolean): Promise<GoogleStatus> {
    return this.#serial(async () => {
      this.#ready();
      if (!botId || botId.length > 200) throw new RpcError("bad_params", "Choose a valid bot.");
      if (enabled && !this.#record?.connection) throw new RpcError("no_account", "Connect Google before granting bot access.");
      if (!this.#record) return this.status();
      const ids = this.#record.grantedBotIds.filter((id) => id !== botId);
      if (enabled) ids.push(botId);
      await this.#save({ ...this.#record, grantedBotIds: ids.sort() }); this.#notify(); return this.status();
    });
  }
  unregisterBot(botId: string): Promise<GoogleStatus> { return this.setGrant(botId, false); }
  #authorized(botId: string, service: GoogleService, epoch?: number): void {
    this.#ready();
    if ((epoch !== undefined && epoch !== this.#epoch) || !this.#record?.connection || !this.#record.connection.services.includes(service) || !this.#record.grantedBotIds.includes(botId)) throw new RpcError("no_account", "This bot does not have access to the connected Google service.");
  }
  async #refreshAccess(): Promise<void> {
    if (this.#access && this.#access.expires > this.#now() + 30_000) return;
    if (this.#refresh) return this.#refresh;
    const epoch = this.#epoch, record = this.#record;
    if (!record?.connection) throw new RpcError("no_account", "Connect Google first.");
    const refresh = (async () => {
      const response = await this.#post(TOKEN, { grant_type: "refresh_token", refresh_token: record.connection!.refreshToken, client_id: record.client.clientId, client_secret: record.client.clientSecret });
      if (response.status !== 200) {
        const invalid = z.object({ error: z.literal("invalid_grant") }).safeParse(response.body).success;
        if (invalid) await this.#serial(async () => {
          if (this.#epoch !== epoch || this.#closed) return;
          this.#epoch++; this.#access = null;
          const disconnected = { ...this.#record!, connection: null, grantedBotIds: [] };
          this.#record = disconnected;
          try { await this.#save(disconnected); } catch { this.#loadFailed = true; }
          this.#attempt = { id: randomUUID(), state: "failed", message: "Google access expired or was revoked. Connect again to continue." }; this.#notify();
        });
        throw problem(invalid ? "Google access expired or was revoked. Connect again." : "Google could not refresh access. Try again.");
      }
      const parsed = TokenResponse.safeParse(response.body);
      if (!parsed.success) throw problem("Google returned an invalid access response. Connect again.");
      const token = parsed.data;
      const scopes = token.scope?.split(/\s+/) ?? record.connection!.scopes;
      if (record.connection!.services.some((service) => !scopes.includes(SCOPES[service]))) throw problem("Google's read permissions changed. Connect again.");
      await this.#serial(async () => {
        if (this.#epoch !== epoch || this.#closed || !this.#record?.connection) return;
        if (token.refresh_token) await this.#save({ ...this.#record, connection: { ...this.#record.connection, refreshToken: token.refresh_token, scopes } });
        this.#access = { value: token.access_token, expires: this.#now() + token.expires_in * 1000 };
      });
    })();
    this.#refresh = refresh;
    try { await refresh; } finally { if (this.#refresh === refresh) this.#refresh = null; }
  }
  async request(botId: string, service: GoogleService, relativePath: string): Promise<unknown> {
    const url = apiUrl(service, relativePath);
    this.#authorized(botId, service); const epoch = this.#epoch;
    await this.#refreshAccess();
    this.#authorized(botId, service, epoch);
    if (!this.#access) throw problem("Google access is unavailable. Connect again.");
    const response = await this.#send({ url, method: "GET", headers: { authorization: `Bearer ${this.#access.value}` } });
    this.#authorized(botId, service, epoch);
    if (response.status !== 200) { if (response.status === 401) this.#access = null; throw problem("Google could not complete this read request. Check access and try again."); }
    return response.body;
  }
  async disconnect(): Promise<GoogleStatus> {
    const operation = await this.#serial(async () => {
      if (this.#closed) throw problem("The Google connection is closed.");
      if (this.#disconnecting) throw new RpcError("busy", "Google is already disconnecting.");
      this.#disconnecting = true; this.#stopPending(); this.#access = null; this.#refresh = null;
      const epoch = ++this.#epoch, refreshToken = this.#record?.connection?.refreshToken;
      let message: string | null = this.#loadFailed ? "Local Google settings removed. Remote consent could not be checked; remove app access in your Google Account." : null;
      this.#record = this.#record ? { ...this.#record, connection: null, grantedBotIds: [] } : null;
      try {
        if (this.#record) await this.#save(this.#record); else await this.#options.vault.clear();
        this.#loadFailed = false;
      } catch { this.#loadFailed = true; message = "Google access is disabled locally, but saved settings could not be removed. Retry Disconnect and remove app access in your Google Account."; }
      this.#attempt = null; this.#notify(); return { epoch, refreshToken, message };
    });
    let message = operation.message;
    if (operation.refreshToken) {
      try { if ((await this.#post(REVOKE, { token: operation.refreshToken })).status !== 200) throw problem("Revocation failed."); }
      catch { message ??= "Google is disconnected locally. Remote revocation could not be confirmed; remove app access in your Google Account."; }
    }
    return this.#serial(async () => {
      this.#disconnecting = false;
      if (this.#epoch === operation.epoch) this.#attempt = message ? { id: randomUUID(), state: "failed", message } : null;
      this.#notify(); return this.status();
    });
  }
  async close(): Promise<void> {
    await this.#serial(async () => { this.#closed = true; this.#epoch++; this.#stopPending(); this.#attempt = null; this.#access = null; for (const controller of this.#requests) controller.abort(); });
  }
}
