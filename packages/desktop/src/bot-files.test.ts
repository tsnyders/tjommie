import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { tempRoot } from "../../daemon/src/testing.ts";
import { notesFile } from "./bot-files.ts";

describe("notesFile", () => {
  const root = tempRoot();
  const bots = join(root, "bots");
  const home = join(bots, "b1");
  mkdirSync(join(home, "inbox"), { recursive: true });
  writeFileSync(join(home, "notes.md"), "# b1's notes\n");
  writeFileSync(join(home, "inbox", "notes.md"), "an attachment");
  mkdirSync(join(bots, "b2"), { recursive: true });
  mkdirSync(join(root, "elsewhere"), { recursive: true });
  writeFileSync(join(root, "elsewhere", "notes.md"), "not a bot");

  it("opens only a bot's own notes.md", () => {
    expect(notesFile(bots, home)).toBe(join(home, "notes.md"));
    expect(notesFile(bots, join(home, "inbox"))).toBeNull();
    expect(notesFile(bots, join(root, "elsewhere"))).toBeNull();
    expect(notesFile(bots, join(bots, "..", "elsewhere"))).toBeNull();
    expect(notesFile(bots, join(bots, "b2"))).toBeNull(); // no notes.md there
    expect(notesFile(bots, bots)).toBeNull();
  });
});
