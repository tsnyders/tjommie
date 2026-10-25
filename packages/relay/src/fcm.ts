import { importPKCS8, SignJWT } from "jose";
import { type PushData, type PushKind } from "@tjommie/protocol";

export type FcmEnv = { FCM_SERVICE_ACCOUNT?: string; FCM_ENDPOINT?: string; FCM_TOKEN_ENDPOINT?: string };
type ServiceAccount = { client_email: string; private_key: string; project_id: string };
type PushResult = { ok: boolean; unregistered: boolean };

const scope = "https://www.googleapis.com/auth/firebase.messaging";
const bodies: Record<PushKind, string> = {
  approval: "A bot needs you",
  limit: "An account hit its limit",
  routine_failed: "A routine failed",
};
let cached: { secret: string; endpoint: string; token: string; expiresAt: number } | null = null;

function accountFrom(secret: string): ServiceAccount {
  const value: unknown = JSON.parse(secret);
  if (typeof value !== "object" || value === null) throw new Error("Invalid FCM credentials.");
  const fields = value as Record<string, unknown>;
  if (typeof fields.client_email !== "string" || typeof fields.private_key !== "string" || typeof fields.project_id !== "string")
    throw new Error("Invalid FCM credentials.");
  return { client_email: fields.client_email, private_key: fields.private_key, project_id: fields.project_id };
}

async function accessToken(secret: string, account: ServiceAccount, endpoint: string): Promise<string> {
  const now = Date.now();
  if (cached?.secret === secret && cached.endpoint === endpoint && now < cached.expiresAt - 60_000) return cached.token;
  const key = await importPKCS8(account.private_key, "RS256");
  const issuedAt = Math.floor(now / 1000);
  const assertion = await new SignJWT({ scope }).setProtectedHeader({ alg: "RS256", typ: "JWT" })
    .setIssuer(account.client_email).setSubject(account.client_email).setAudience(endpoint)
    .setIssuedAt(issuedAt).setExpirationTime(issuedAt + 3600).sign(key);
  const response = await fetch(endpoint, {
    method: "POST", headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({ grant_type: "urn:ietf:params:oauth:grant-type:jwt-bearer", assertion }),
  });
  if (!response.ok) { await response.body?.cancel(); throw new Error("FCM authorization failed."); }
  const value: unknown = await response.json();
  if (typeof value !== "object" || value === null) throw new Error("Invalid FCM authorization response.");
  const fields = value as Record<string, unknown>;
  if (typeof fields.access_token !== "string" || typeof fields.expires_in !== "number" || fields.expires_in <= 0)
    throw new Error("Invalid FCM authorization response.");
  cached = { secret, endpoint, token: fields.access_token, expiresAt: now + fields.expires_in * 1000 };
  return fields.access_token;
}

export async function sendPush(env: FcmEnv, tokens: string[], data: PushData): Promise<PushResult[]> {
  const failed = tokens.map(() => ({ ok: false, unregistered: false }));
  if (!env.FCM_SERVICE_ACCOUNT) return failed;
  let account: ServiceAccount, token: string;
  try {
    account = accountFrom(env.FCM_SERVICE_ACCOUNT);
    token = await accessToken(env.FCM_SERVICE_ACCOUNT, account, env.FCM_TOKEN_ENDPOINT ?? "https://oauth2.googleapis.com/token");
  } catch { return failed; }
  const endpoint = `${(env.FCM_ENDPOINT ?? "https://fcm.googleapis.com/v1").replace(/\/$/, "")}/projects/${encodeURIComponent(account.project_id)}/messages:send`;
  const results: PushResult[] = [];
  for (const deviceToken of tokens) {
    try {
      const response = await fetch(endpoint, {
        method: "POST",
        headers: { authorization: `Bearer ${token}`, "content-type": "application/json" },
        body: JSON.stringify({ message: {
          token: deviceToken,
          notification: { title: "Tjommie", body: bodies[data.kind] }, data,
          android: { priority: "high", notification: { tag: `${data.kind}:${data.botId}`, channel_id: "bots" } },
        } }),
      });
      const unregistered = response.status === 404 || (!response.ok && (await response.text()).includes('"UNREGISTERED"'));
      if (response.ok || response.status === 404) await response.body?.cancel();
      results.push({ ok: response.ok, unregistered });
    } catch { results.push({ ok: false, unregistered: false }); }
  }
  return results;
}
