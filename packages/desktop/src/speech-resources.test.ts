import { createHash } from "node:crypto";
import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { tempRoot } from "./testing.ts";
import { packagedSpeechDigest, SPEECH_RESOURCE_NAMES, SPEECH_RUNTIME_COMMIT, verifySpeechResources } from "./speech-resources.ts";

function fixture() {
  const root = tempRoot();
  const files = SPEECH_RESOURCE_NAMES.map((name) => { const bytes = Buffer.from(`synthetic resource: ${name}`); writeFileSync(join(root, name), bytes); return { name, bytes: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") }; });
  const manifest = { version: 1, repository: "https://github.com/ggml-org/whisper.cpp", commit: SPEECH_RUNTIME_COMMIT, platform: "win32", arch: "x64", cpuOnly: true, files };
  writeFileSync(join(root, "manifest.json"), JSON.stringify(manifest)); return { root, manifest };
}
describe("trusted packaged speech resources", () => {
  it("accepts only the pinned CPU runtime and exact checked notice/runtime bytes", () => {
    const { root, manifest } = fixture(); expect(verifySpeechResources(root)).toEqual(manifest); expect(packagedSpeechDigest(root)).toBe(manifest.files[0].sha256);
    writeFileSync(join(root, "whisper-cli.exe"), "different runtime"); expect(packagedSpeechDigest(root)).toBeNull(); expect(() => verifySpeechResources(root)).toThrow("integrity");
  });
  it("fails closed for missing, wrong-commit, missing-notice or path-bearing manifests", () => {
    expect(packagedSpeechDigest(tempRoot())).toBeNull();
    for (const changed of [{ commit: "0".repeat(40) }, { cpuOnly: false }, { files: [] }, { files: Array(5).fill({ name: "../outside", bytes: 1, sha256: "0".repeat(64) }) }]) {
      const { root, manifest } = fixture(); writeFileSync(join(root, "manifest.json"), JSON.stringify({ ...manifest, ...changed })); expect(packagedSpeechDigest(root)).toBeNull();
    }
  });
});
