import { readFileSync } from "node:fs";
import { expect, it } from "vitest";

function dependencies(workspace: string): Record<string, string> {
  const manifest: { dependencies?: Record<string, string> } = JSON.parse(readFileSync(new URL(`../../${workspace}/package.json`, import.meta.url), "utf8"));
  return manifest.dependencies ?? {};
}

// The UI parses with zod itself (saved PCs, pairing identities, push taps), so it declares it instead of leaning on another workspace's copy.
it("declares zod, with the range the other workspaces use", () => {
  expect(dependencies("ui").zod).toBe(dependencies("protocol").zod);
});
