import { describe, it } from "vitest";
import { probeProjectConfig } from "../../scripts/codex-project-config-spike.ts";
import { codexPolicy } from "./codex.ts";

describe("Codex project config protection against the real CLI", () => {
  it.each(["create", "edit"] as const)("asks before using apply_patch to %s project config with folder autonomy", async (operation) => {
    await probeProjectConfig(operation, codexPolicy("folder"));
  });
});
