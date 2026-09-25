import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { openDb } from "./db.ts";
import { getSettings, setSettings } from "./settings.ts";
import { tempRoot } from "./testing.ts";

describe("settings", () => {
  it("defaults the chain limit to 5 and remembers a change", () => {
    const db = openDb(join(tempRoot(), "t.db"));
    expect(getSettings(db)).toEqual({ chainLimit: 5 });
    expect(setSettings(db, { chainLimit: 2 })).toEqual({ chainLimit: 2 });
    expect(getSettings(db)).toEqual({ chainLimit: 2 });
  });
});
