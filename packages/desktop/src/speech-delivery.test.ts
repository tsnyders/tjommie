import { readFileSync } from "node:fs";
import { expect, it } from "vitest";
import { SPEECH_RUNTIME_COMMIT } from "./speech-resources.ts";

it("builds the pinned speech recipe in CI and before the Windows release assembles the installer's payload", () => {
  const read = (relative: string) => readFileSync(new URL(relative, import.meta.url), "utf8").replaceAll("\r\n", "\n");
  const ci = read("../../../.github/workflows/ci.yml"), release = read("../../../.github/workflows/release.yml");
  const recipe = "run: ./packages/desktop/scripts/build-speech-runtime.ps1";
  expect(ci).toContain(recipe);
  const windows = release.split("\n  windows:\n")[1]?.split("\n  android:\n")[0];
  expect(windows).toBeDefined();
  // The payload (resources\speech\ beside the app) is where the installer gets the speech runtime from; Electron packaging no longer exists.
  const built = windows!.indexOf(recipe), assembled = windows!.indexOf("./installer/assemble-payload.ps1");
  expect(built).toBeGreaterThanOrEqual(0); expect(assembled).toBeGreaterThan(built);
  const script = read("../scripts/build-speech-runtime.ps1");
  expect(script).toContain(`$commit = '${SPEECH_RUNTIME_COMMIT}'`);
  expect(script).toContain("$actual.Trim() -ne $commit");
  expect(script).toContain("--config Release --target whisper-cli");
  expect(script).toContain("'check-speech-resources.ts'");
  expect(script).toContain("(Join-Path $source 'LICENSE') -Destination (Join-Path $output 'GGML-LICENSE.txt')");
  expect(script).not.toContain("'ggml/LICENSE'");
  expect(read("../scripts/check-speech-resources.ts")).toContain("verifySpeechResources(process.argv[2])");
});
