import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { expect, it } from "vitest";
import { removeCodexDenyEntries } from "./test-acl.ts";
import { tempRoot } from "./testing.ts";

// A disposable folder with no Codex deny entry: the pass finds nothing to remove (or no CodexSandboxUsers group on this
// PC) and reports no failure. It reads the folder's ACL only.
it("reports no failure for a folder that has no Codex deny entry, and for none at all", () => {
  const folder = join(tempRoot(), "data");
  mkdirSync(folder);
  expect(removeCodexDenyEntries([folder]).failures).toEqual([]);
  expect(removeCodexDenyEntries([]).failures).toEqual([]);
});

it("reports a path it cannot read instead of passing it", () => {
  const result = removeCodexDenyEntries([join(tempRoot(), "missing")]);
  // Without the CodexSandboxUsers group there is nothing to check, so the pass stops before it looks at any path.
  if (result.groupMissing) expect(result.failures).toEqual([]);
  else expect(result.failures).toHaveLength(1);
});
