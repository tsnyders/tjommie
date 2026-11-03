import { createHash } from "node:crypto";
import { mkdtemp, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SpeechModel } from "./speech-model.ts";

const directories: string[] = [];
const bytes = Buffer.from("synthetic speech model");
const source = { bytes: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex"), url: "https://huggingface.co/synthetic/model" };
async function directory(): Promise<string> { const path = await mkdtemp(join(tmpdir(), "tjommie-speech-model-test-")); directories.push(path); return path; }
afterEach(async () => { for (const path of directories.splice(0)) { expect(dirname(resolve(path))).toBe(resolve(tmpdir())); await rm(path, { recursive: true, force: true }); } });

describe("verified local speech model", () => {
  it("publishes only exact bytes/hash, revalidates an existing model, and removes only its file", async () => {
    const path = await directory();
    const request = vi.fn<typeof fetch>(async () => new Response(bytes, { headers: { "content-length": String(bytes.length) } }));
    const changed = vi.fn();
    const model = new SpeechModel({ directory: path, source, fetch: request, changed });
    expect((await model.status()).state).toBe("missing");
    await model.downloadModel();
    expect((await model.status()).state).toBe("ready");
    expect(await model.verify()).toBe(true);
    await writeFile(join(path, "unrelated.txt"), "keep");
    await model.remove();
    expect(await readdir(path)).toEqual(["unrelated.txt"]);
    expect((await model.status()).state).toBe("missing");
  });
  it.each(["truncated", "corrupt", "oversized"])("never publishes %s data", async (kind) => {
    const path = await directory();
    const data = kind === "truncated" ? bytes.subarray(1) : kind === "oversized" ? Buffer.concat([bytes, bytes]) : Buffer.alloc(bytes.length, 1);
    const request = vi.fn<typeof fetch>(async () => new Response(data));
    const model = new SpeechModel({ directory: path, source, fetch: request, changed: () => {} });
    await expect(model.downloadModel()).rejects.toThrow("verify");
    expect((await model.status()).state).toBe("error");
    expect(await readdir(path)).toEqual([]);
  });
  it("cancels a pending download and deletes the partial instead of publishing a late result", async () => {
    const path = await directory();
    let received: () => void = () => {};
    const progress = new Promise<void>((done) => { received = done; });
    const request = vi.fn<typeof fetch>(async (_url, options) => new Response(new ReadableStream<Uint8Array>({ start(controller) {
      controller.enqueue(bytes.subarray(0, 2));
      options?.signal?.addEventListener("abort", () => controller.error(new DOMException("Aborted", "AbortError")), { once: true });
    } })));
    let model: SpeechModel;
    model = new SpeechModel({ directory: path, source, fetch: request, changed: () => { void model.status().then((status) => { if (status.received) received(); }); } });
    const downloading = model.downloadModel();
    void downloading.catch(() => {});
    await progress;
    await model.cancelDownload();
    await expect(downloading).rejects.toThrow("cancelled");
    expect((await model.status()).state).toBe("missing");
    expect(await readdir(path)).toEqual([]);
  });
  it("rejects redirects outside upstream HTTPS model hosts without requesting them", async () => {
    const request = vi.fn<typeof fetch>(async () => new Response(null, { status: 302, headers: { location: "http://127.0.0.1/private" } }));
    const model = new SpeechModel({ directory: await directory(), source, fetch: request, changed: () => {} });
    await expect(model.downloadModel()).rejects.toThrow();
    expect(request).toHaveBeenCalledOnce();
  });
  it("cleans only stale owned partial model files after an interrupted app run", async () => {
    const path = await directory();
    await writeFile(join(path, `.${source.sha256}.00000000-0000-0000-0000-000000000000.partial`), "incomplete synthetic data");
    await writeFile(join(path, "unrelated.partial"), "keep");
    const model = new SpeechModel({ directory: path, source, changed: () => {} });
    expect((await model.status()).state).toBe("missing");
    expect(await readdir(path)).toEqual(["unrelated.partial"]);
  });
});
