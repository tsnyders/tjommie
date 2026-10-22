import { GoogleServices, type GoogleAuthorization, type GoogleService } from "@tjommie/protocol";

const serviceScopes: Record<GoogleService, string[]> = {
  gmail: ["https://www.googleapis.com/auth/gmail.readonly"],
  drive: ["https://www.googleapis.com/auth/drive.readonly", "https://www.googleapis.com/auth/drive.metadata.readonly"],
  calendar: ["https://www.googleapis.com/auth/calendar.readonly"],
};
const invalid = (): Error => new Error("Google authorization returned an invalid consent address.");

// This opener is deliberately narrower than a general external-link bridge. A website in the bot's
// browser never gets the preload, and even app-renderer requests cannot open arbitrary URLs here.
export function googleConsentUrl(value: string, services: GoogleService[]): string {
  let url: URL;
  try { url = new URL(value); } catch { throw invalid(); }
  if (url.origin !== "https://accounts.google.com" || url.pathname !== "/o/oauth2/v2/auth" || url.hash || url.username || url.password) throw invalid();
  const params = url.searchParams;
  const allowedKeys = new Set(["client_id", "redirect_uri", "response_type", "scope", "state", "code_challenge", "code_challenge_method", "access_type", "prompt", "include_granted_scopes"]);
  for (const key of params.keys()) if (!allowedKeys.has(key) || params.getAll(key).length !== 1) throw invalid();
  if (!/^[a-zA-Z0-9_-]+\.apps\.googleusercontent\.com$/.test(params.get("client_id") ?? "") || params.get("response_type") !== "code" || params.get("code_challenge_method") !== "S256") throw invalid();
  for (const key of ["state", "code_challenge"]) if (!/^[A-Za-z0-9_-]{43,128}$/.test(params.get(key) ?? "")) throw invalid();
  let redirect: URL;
  try { redirect = new URL(params.get("redirect_uri") ?? ""); } catch { throw invalid(); }
  if (redirect.protocol !== "http:" || redirect.hostname !== "127.0.0.1" || !redirect.port || Number(redirect.port) <= 0 || redirect.username || redirect.password || redirect.search || redirect.hash || !/^\/[A-Za-z0-9/_-]+$/.test(redirect.pathname)) throw invalid();
  const allowedScopes = new Set(["openid", "email", "https://www.googleapis.com/auth/userinfo.email", ...services.flatMap((service) => serviceScopes[service])]);
  const scopes = (params.get("scope") ?? "").split(/\s+/).filter(Boolean);
  if (!scopes.length || scopes.some((scope) => !allowedScopes.has(scope))) throw invalid();
  return url.href;
}

export async function beginGoogleConsent(options: {
  services: unknown;
  begin: (services: GoogleService[]) => Promise<GoogleAuthorization>;
  cancel: (attempt: string) => Promise<unknown>;
  open: (url: string) => Promise<void>;
}): Promise<{ attempt: string }> {
  const services = GoogleServices.parse(options.services);
  const authorization = await options.begin(services);
  try { await options.open(googleConsentUrl(authorization.url, services)); }
  catch {
    await options.cancel(authorization.attempt).catch(() => {});
    throw new Error("The Google consent window could not open. Try connecting again.");
  }
  return { attempt: authorization.attempt };
}
