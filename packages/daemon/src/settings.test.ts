import { join } from "node:path";
import { DEFAULT_RELAY_URL } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { openDb } from "./db.ts";
import { RpcError } from "./server.ts";
import { getSettings, setSettings } from "./settings.ts";
import { tempRoot } from "./testing.ts";

describe("settings", () => {
  it("defaults the chain limit to 5 and remembers a change", () => {
    const db = openDb(join(tempRoot(), "t.db"));
    expect(getSettings(db)).toEqual({ chainLimit: 5, relayUrl: DEFAULT_RELAY_URL });
    expect(setSettings(db, { chainLimit: 2 })).toEqual({ chainLimit: 2, relayUrl: DEFAULT_RELAY_URL });
    expect(getSettings(db)).toEqual({ chainLimit: 2, relayUrl: DEFAULT_RELAY_URL });
  });

  it("rejects an invalid change without changing the stored settings", () => {
    const db = openDb(join(tempRoot(), "t.db"));
    setSettings(db, { chainLimit: 2 });
    let caught: unknown;
    try {
      setSettings(db, { chainLimit: 0 });
    } catch (error) {
      caught = error;
    }
    expect(caught).toBeInstanceOf(RpcError);
    expect(caught).toMatchObject({ code: "bad_params", message: "Invalid settings" });
    expect(db.prepare("SELECT value FROM settings WHERE key = 'chainLimit'").get()).toEqual({ value: "2" });
    expect(getSettings(db)).toEqual({ chainLimit: 2, relayUrl: DEFAULT_RELAY_URL });
  });

  it("uses the default when a stored setting is corrupt", () => {
    const db = openDb(join(tempRoot(), "t.db"));
    db.prepare("INSERT INTO settings (key, value) VALUES ('chainLimit', '0')").run();
    expect(getSettings(db)).toEqual({ chainLimit: 5, relayUrl: DEFAULT_RELAY_URL });
    db.prepare("UPDATE settings SET value = 'not json' WHERE key = 'chainLimit'").run();
    expect(getSettings(db)).toEqual({ chainLimit: 5, relayUrl: DEFAULT_RELAY_URL });
  });

  it("stores only patched keys so a later relay default can take effect", () => {
    const db = openDb(join(tempRoot(), "t.db"));
    setSettings(db, { chainLimit: 2 });
    expect(db.prepare("SELECT key, value FROM settings ORDER BY key").all()).toEqual([{ key: "chainLimit", value: "2" }]);
    setSettings(db, { relayUrl: "https://relay.example/old-base" });
    expect(db.prepare("SELECT key FROM settings ORDER BY key").all()).toEqual([{ key: "chainLimit" }, { key: "relayUrl" }]);
  });

  it("reads back a stored opt-out and a custom relay over the default", () => {
    const db = openDb(join(tempRoot(), "t.db"));
    expect(getSettings(db).relayUrl).toBe(DEFAULT_RELAY_URL);
    setSettings(db, { relayUrl: null });
    expect(getSettings(db).relayUrl).toBeNull();
    setSettings(db, { relayUrl: "https://relay.example/old-base" });
    expect(getSettings(db).relayUrl).toBe("https://relay.example/old-base");
  });
});
