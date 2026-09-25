import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { openDb } from "./db.ts";
import { RpcError } from "./server.ts";
import { getSettings, setSettings } from "./settings.ts";
import { tempRoot } from "./testing.ts";

describe("settings", () => {
  it("defaults the chain limit to 5 and remembers a change", () => {
    const db = openDb(join(tempRoot(), "t.db"));
    expect(getSettings(db)).toEqual({ chainLimit: 5 });
    expect(setSettings(db, { chainLimit: 2 })).toEqual({ chainLimit: 2 });
    expect(getSettings(db)).toEqual({ chainLimit: 2 });
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
    expect(getSettings(db)).toEqual({ chainLimit: 2 });
  });

  it("uses the default when a stored setting is corrupt", () => {
    const db = openDb(join(tempRoot(), "t.db"));
    db.prepare("INSERT INTO settings (key, value) VALUES ('chainLimit', '0')").run();
    expect(getSettings(db)).toEqual({ chainLimit: 5 });
    db.prepare("UPDATE settings SET value = 'not json' WHERE key = 'chainLimit'").run();
    expect(getSettings(db)).toEqual({ chainLimit: 5 });
  });
});
