import { DEFAULT_RELAY_URL, Settings } from "@tjommie/protocol";
import { transaction, type Db } from "./db.ts";
import { RpcError } from "./server.ts";

const DEFAULTS: Settings = { chainLimit: 5, relayUrl: DEFAULT_RELAY_URL };

export function getSettings(db: Db): Settings {
  const rows = db.prepare("SELECT key, value FROM settings").all() as { key: string; value: string }[];
  let settings = DEFAULTS;
  for (const row of rows) {
    try {
      const parsed = Settings.safeParse({ ...settings, [row.key]: JSON.parse(row.value) });
      if (parsed.success) settings = parsed.data;
    } catch {
      // Keep the current valid settings when stored JSON is corrupt.
    }
  }
  return settings;
}

export function setSettings(db: Db, patch: Partial<Settings>): Settings {
  const parsed = Settings.safeParse({ ...getSettings(db), ...patch });
  if (!parsed.success) throw new RpcError("bad_params", "Invalid settings");
  transaction(db, () => {
    for (const [key, value] of Object.entries(parsed.data)) {
      db.prepare("INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value").run(key, JSON.stringify(value));
    }
  });
  return parsed.data;
}
