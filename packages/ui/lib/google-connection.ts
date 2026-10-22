// The Google connection as the UI sees it: the daemon's status (protocol google.ts) and nothing else. The UI holds no token.

import type { GoogleService, GoogleStatus } from "@tjommie/protocol";

export type { GoogleService, GoogleStatus };

// What the card needs from the app: the daemon's google.* calls, plus the desktop app's connect, which opens the consent
// page in the system browser (the renderer never opens it) and returns the attempt it began.
export type GoogleApi = {
  status(): Promise<GoogleStatus>;
  configure(clientId: string, clientSecret: string): Promise<GoogleStatus>;
  connect(): Promise<{ attempt: string }>;
  cancel(attempt: string): Promise<unknown>;
  disconnect(): Promise<GoogleStatus>;
  grant(botId: string, enabled: boolean): Promise<GoogleStatus>;
  onChanged(listener: (status: GoogleStatus) => void): () => void;
};

// The shape Google gives a Desktop app client's ID; the daemon rejects anything else.
export const validClientId = (id: string): boolean => /^[a-zA-Z0-9_-]+\.apps\.googleusercontent\.com$/.test(id.trim());

export const POLL_MS = 1000;

export type GoogleStep = "setup" | "connect" | "waiting" | "connected";

export function googleStep(status: GoogleStatus): GoogleStep {
  if (status.connected) return "connected";
  if (status.attempt?.state === "waiting") return "waiting";
  return status.configured ? "connect" : "setup";
}

// What a connection asks for: Gmail, Drive and Calendar, with full access (send and manage, create, edit and delete).
export const GOOGLE_SERVICES: GoogleService[] = ["gmail", "drive", "calendar"];
const SERVICE_NAME: Record<GoogleService, string> = { gmail: "Gmail", drive: "Drive", calendar: "Calendar" };

// The services a connection covers, in words ("Gmail, Drive and Calendar"). Before one is connected, what it will ask for.
export function serviceList(services: GoogleService[]): string {
  const names = (services.length === 0 ? GOOGLE_SERVICES : services).map((each) => SERVICE_NAME[each]);
  return names.length === 1 ? names[0] : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

const ABILITY: Record<GoogleService, string> = {
  gmail: "send and manage your email",
  drive: "create, edit and delete Drive files",
  calendar: "create, edit and delete Calendar events",
};

// What full access lets a bot do, for the services covered: "send and manage your email, create, edit and delete Drive files
// and create, edit and delete Calendar events".
export function abilities(services: GoogleService[]): string {
  const each = (services.length === 0 ? GOOGLE_SERVICES : services).map((service) => ABILITY[service]);
  return each.length === 1 ? each[0] : `${each.slice(0, -1).join(", ")} and ${each[each.length - 1]}`;
}

// A connection made when Google was asked for read-only access can't do the rest. The status may say so with an `access`
// of "read" (the daemon's field, when it has one); that connection must be made again to give full access.
export function needsReconsent(status: GoogleStatus): boolean {
  const extra: GoogleStatus & { access?: unknown } = status;
  return status.connected && extra.access === "read";
}

// Whether the saved local Google settings can be cleared: when nothing is connected but something is saved or went wrong,
// including settings that can't be read, which no other button can reset.
export const canClearLocal = (status: GoogleStatus): boolean => !status.connected && (status.configured || status.attempt?.state === "failed");

export const canRead = (status: GoogleStatus, botId: string): boolean => status.grantedBotIds.includes(botId);

// Why the last sign-in didn't work, if it didn't.
export const attemptProblem = (status: GoogleStatus): string | null => (status.attempt?.state === "failed" ? (status.attempt.message ?? "Google sign-in didn't finish.") : null);

// A short line for the bot panel's card.
export function cardLine(status: GoogleStatus | null, botName: string, botId: string): string {
  if (!status) return "Checking…";
  const step = googleStep(status);
  if (step === "setup") return "Not set up yet";
  if (step === "connect") return "Not connected";
  if (step === "waiting") return "Waiting for you to finish in your browser";
  const who = status.email ?? "Connected";
  if (needsReconsent(status)) return `${who}. Reconnect to give full access.`;
  return canRead(status, botId) ? `${who}. ${botName} has full access to ${serviceList(status.services)}.` : `${who}. ${botName} has no access.`;
}

// Follows the status: once now, then every second while a sign-in is waiting, and whenever the daemon says it changed.
// Returns what stops it. A failed read is reported and the following ones go on.
export function watchGoogle(api: GoogleApi, onStatus: (status: GoogleStatus) => void, onError: (error: unknown) => void, every = POLL_MS): () => void {
  let stopped = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const tick = (): void => {
    api.status().then(
      (status) => {
        if (stopped) return;
        onStatus(status);
        if (status.attempt?.state === "waiting") timer = setTimeout(tick, every);
      },
      (error: unknown) => {
        if (!stopped) onError(error);
      },
    );
  };
  const off = api.onChanged((status) => {
    if (stopped) return;
    clearTimeout(timer);
    onStatus(status);
    if (status.attempt?.state === "waiting") timer = setTimeout(tick, every);
  });
  tick();
  return () => {
    stopped = true;
    clearTimeout(timer);
    off();
  };
}
