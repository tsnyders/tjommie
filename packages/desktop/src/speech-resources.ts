import { createHash } from "node:crypto";
import { lstatSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { z } from "zod";

export const SPEECH_RUNTIME_COMMIT = "d1be6fde11ac6e0407606b4e42fe72d34add8037";
export const SPEECH_RESOURCE_NAMES = ["whisper-cli.exe", "WHISPER-LICENSE.txt", "GGML-LICENSE.txt", "OPENAI-WHISPER-LICENSE.txt", "MODEL-NOTICE.txt"] as const;
const Manifest = z.strictObject({
  version: z.literal(1), repository: z.literal("https://github.com/ggml-org/whisper.cpp"), commit: z.literal(SPEECH_RUNTIME_COMMIT), platform: z.literal("win32"), arch: z.literal("x64"), cpuOnly: z.literal(true),
  files: z.array(z.strictObject({ name: z.enum(SPEECH_RESOURCE_NAMES), bytes: z.number().int().min(1).max(128 * 1024 * 1024), sha256: z.string().regex(/^[a-f0-9]{64}$/) })).length(SPEECH_RESOURCE_NAMES.length),
});

export function verifySpeechResources(directory: string): z.infer<typeof Manifest> {
  const path = join(directory, "manifest.json"), stat = lstatSync(path);
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size > 16384) throw new Error("Invalid packaged speech manifest.");
  const manifest = Manifest.parse(JSON.parse(readFileSync(path, "utf8")));
  if (new Set(manifest.files.map((file) => file.name)).size !== SPEECH_RESOURCE_NAMES.length) throw new Error("Missing packaged speech notice or runtime.");
  for (const file of manifest.files) {
    const target = join(directory, file.name), info = lstatSync(target);
    if (!info.isFile() || info.isSymbolicLink() || info.size !== file.bytes || createHash("sha256").update(readFileSync(target)).digest("hex") !== file.sha256) throw new Error("A packaged speech resource failed its integrity check.");
  }
  return manifest;
}

export function packagedSpeechDigest(directory: string): string | null {
  try { return verifySpeechResources(directory).files.find((file) => file.name === "whisper-cli.exe")!.sha256; }
  catch { return null; }
}
