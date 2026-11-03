import { createHash } from "node:crypto";
import { readdirSync } from "node:fs";
import { expect, it, vi } from "vitest";
import { tempRoot } from "../../daemon/src/testing.ts";
import { SpeechModel } from "./speech-model.ts";

const cleanup = vi.hoisted(() => ({ blocked: true }));
vi.mock("node:fs/promises", async (original) => {
  const native = await original<typeof import("node:fs/promises")>();
  return { ...native, rm: async (...args: Parameters<typeof native.rm>) => { if (cleanup.blocked && String(args[0]).endsWith(".partial")) throw new Error("Synthetic blocked cleanup"); return native.rm(...args); } };
});

it("keeps repeated model cancellation failed until restart verifies the retained partial was removed", async () => {
  const directory = tempRoot(), bytes = Buffer.from("synthetic model");
  const source = { bytes: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex"), url: "https://huggingface.co/synthetic/model" };
  const model = new SpeechModel({ directory, source, changed: () => {}, fetch: async () => new Response(Buffer.alloc(bytes.length)) });
  await expect(model.downloadModel()).rejects.toThrow("could not be cleared");
  expect(readdirSync(directory)).toHaveLength(1);
  await expect(model.cancelDownload()).rejects.toThrow("cleanup failed"); await expect(model.cancelDownload()).rejects.toThrow("cleanup failed");
  expect(await model.verify()).toBe(false); expect((await model.status()).state).toBe("error");
  cleanup.blocked = false;
  await expect(model.cancelDownload()).rejects.toThrow("cleanup failed");
  const restarted = new SpeechModel({ directory, source, changed: () => {} }); expect((await restarted.status()).state).toBe("missing");
  await restarted.cancelDownload(); expect(readdirSync(directory)).toEqual([]);
});
