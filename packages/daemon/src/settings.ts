import { Settings } from "@tjommie/protocol";
import type { Db } from "./db.ts";

const DEFAULTS: Settings = { chainLimit: 5 };

export function getSettings(db: Db): Settings {
  const rows = db.prepare("SELECT key, value FROM settings").all() as { key: string; value: string }[];
  return Settings.parse({ ...DEFAULTS, ...Object.fromEntries(rows.map((row) => [row.key, JSON.parse(row.value)])) });
}

export function setSettings(db: Db, patch: Partial<Settings>): Settings {
  for (const [key, value] of Object.entries(patch)) {
    if (value === undefined) continue;
    db.prepare("INSERT INTO settings (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value").run(key, JSON.stringify(value));
  }
  return getSettings(db);
}
