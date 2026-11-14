import { randomUUID } from "node:crypto";
import { dirname, join } from "node:path";
import { mkdirSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { LanEndpoint, RelayUrl, RemoteKey, RemoteName } from "@tjommie/protocol";
import { z } from "zod";
import { speechOrigin } from "./speech-permissions.ts";

// The paired PCs and the appearance live in the page's localStorage, which the next desktop app can't read. This keeps a copy
// of them in the data folder, which that app reads once, the first time it starts. Zoom isn't in localStorage, so it isn't here.

// The same fields as RemoteHostSchema in packages/ui/lib/remote-hosts.ts. That module is UI code (it loads Capacitor), so
// the desktop's main process can't import it; change both together.
const Host = z.object({
  id: z.uuid(), name: RemoteName, deviceId: z.uuid().nullable(), hostKey: RemoteKey, lan: z.array(LanEndpoint).min(1).max(16), relay: RelayUrl.nullable().default(null),
}).strict();
type Host = z.infer<typeof Host>;

const THEME = "tjommie.theme", TEXT_SIZE = "tjommie.textSize", DENSITY = "tjommie.density", HOSTS = "tjommie.mobile.hosts.v1", PENDING = "tjommie.mobile.pending.v1";
const MAX_HOSTS = 16;
const THEMES = ["dark", "light", "system"] as const, TEXT_SIZES = ["system", "large", "larger"] as const, DENSITIES = ["comfortable", "compact"] as const;

type Read = (key: string) => Promise<string | null>;
type Log = (line: string) => void;

// A key the page never set is normal and gives the default; a value the UI would not accept is dropped and logged (without its content).
function choice<T extends string>(key: string, raw: string | null, allowed: readonly T[], fallback: T, log: Log): T {
  if (raw === null) return fallback;
  const match = allowed.find((value) => value === raw);
  if (match !== undefined) return match;
  log(`Desktop UI export: dropped ${key}, which is not one of its choices.`);
  return fallback;
}

// Unlike the UI, which drops the whole list for one bad entry, this keeps every entry that is valid.
function hosts(key: string, raw: string | null, log: Log): Host[] {
  if (raw === null) return [];
  let list: unknown;
  try { list = JSON.parse(raw); }
  catch { log(`Desktop UI export: dropped ${key}, which is not valid JSON.`); return []; }
  if (!Array.isArray(list)) { log(`Desktop UI export: dropped ${key}, which is not a list.`); return []; }
  const kept: Host[] = [];
  list.forEach((entry: unknown, index) => {
    const parsed = Host.safeParse(entry);
    if (parsed.success) kept.push(parsed.data);
    else log(`Desktop UI export: dropped ${key}[${index}]: ${parsed.error.issues.map((issue) => `${issue.path.join(".") || "entry"} (${issue.code})`).join(", ")}.`);
  });
  if (kept.length <= MAX_HOSTS) return kept;
  log(`Desktop UI export: kept the first ${MAX_HOSTS} of ${kept.length} entries in ${key}.`);
  return kept.slice(0, MAX_HOSTS);
}

// Writes <dataRoot>\data\desktop-ui-export.json. It never throws into the app. If the page can't be read (it is reloading, or
// is not the app's own origin) nothing is written, so an earlier export is never replaced by defaults.
export async function exportDesktopUi(read: Read, dataRoot: string, log: Log = () => {}): Promise<void> {
  try {
    const stored: Record<string, string | null> = {};
    for (const key of [THEME, TEXT_SIZE, DENSITY, HOSTS, PENDING]) stored[key] = await read(key);
    const text = `${JSON.stringify({
      version: 1,
      theme: choice(THEME, stored[THEME] ?? null, THEMES, "dark", log),
      textSize: choice(TEXT_SIZE, stored[TEXT_SIZE] ?? null, TEXT_SIZES, "system", log),
      density: choice(DENSITY, stored[DENSITY] ?? null, DENSITIES, "comfortable", log),
      hosts: hosts(HOSTS, stored[HOSTS] ?? null, log),
      pending: hosts(PENDING, stored[PENDING] ?? null, log),
    }, null, 2)}\n`;
    const file = join(dataRoot, "data", "desktop-ui-export.json");
    mkdirSync(dirname(file), { recursive: true });
    const temporary = `${file}.${randomUUID()}.tmp`;
    try {
      writeFileSync(temporary, text);
      renameSync(temporary, file);
    } catch (error) {
      rmSync(temporary, { force: true });
      throw error;
    }
  } catch (error) {
    log(`Desktop UI export failed: ${error instanceof Error ? error.message : String(error)}`);
  }
}

// The parts of the app's window this needs. Electron's BrowserWindow has them all.
export type UiWindow = {
  webContents: {
    isDestroyed(): boolean;
    getURL(): string;
    executeJavaScript(code: string): Promise<unknown>;
    on(event: "did-finish-load", listener: () => void): unknown;
  };
  on(event: "hide", listener: () => void): unknown;
};

// Exports when the app's page finishes loading and when the window hides to the tray, so a change made during a long session in
// the tray is kept at the next hide. Returns the same export, for the moments the app is about to quit. Production only, and only
// while the page is the app's own: any other origin's storage is not the app's.
export function keepDesktopUiExported(window: UiWindow, options: { production: boolean; dataRoot: string; log: Log }): () => Promise<void> {
  const page = window.webContents; // taken now: a window that is gone no longer answers .webContents
  const now = async (): Promise<void> => {
    if (!options.production) return;
    const read: Read = async (key) => {
      if (page.isDestroyed() || !speechOrigin(page.getURL(), options.production)) throw new Error("The page is not the app's own.");
      const value = await page.executeJavaScript(`localStorage.getItem(${JSON.stringify(key)})`);
      return typeof value === "string" ? value : null;
    };
    await exportDesktopUi(read, options.dataRoot, options.log);
  };
  page.on("did-finish-load", () => { void now(); });
  window.on("hide", () => { void now(); });
  return now;
}
