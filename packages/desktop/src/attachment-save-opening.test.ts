import { createHash, randomUUID } from "node:crypto";
import { readdirSync } from "node:fs";
import { join } from "node:path";
import { expect, it, vi } from "vitest";
import { tempRoot } from "./testing.ts";
import { AttachmentSaves } from "./attachment-save.ts";

const checkpoint = vi.hoisted(() => ({ ready: undefined as (() => void) | undefined, release: undefined as (() => void) | undefined, failures: 2 }));
vi.mock("node:fs/promises", async (original) => {
  const native = await original<typeof import("node:fs/promises")>();
  return {
    ...native,
    open: async (...args: Parameters<typeof native.open>) => { const file = await native.open(...args); await new Promise<void>((resolve) => { checkpoint.release = resolve; checkpoint.ready?.(); }); return file; },
    rm: async (...args: Parameters<typeof native.rm>) => { if (checkpoint.failures-- > 0) throw new Error("Synthetic cleanup failure"); return native.rm(...args); },
  };
});

it("waits for cancelled native opens, reports deletion failure and retains the partial for retry", async () => {
  const root = tempRoot(), saves = new AttachmentSaves();
  const ready = new Promise<void>((resolve) => { checkpoint.ready = resolve; });
  const opening = saves.start("owner", { id: randomUUID(), hostId: "host", name: "file", size: 0, contentType: "application/octet-stream", sha256: createHash("sha256").digest("hex") }, join(root, "destination"));
  const failedOpen = expect(opening).rejects.toThrow("Synthetic cleanup failure");
  await ready; const cancelled = saves.cancelOwner("owner"), failedClear = expect(cancelled).rejects.toThrow("Synthetic cleanup failure"); checkpoint.release?.();
  await failedOpen; await failedClear; expect(readdirSync(root)).toHaveLength(1);
  await saves.cancelOwner("owner"); expect(readdirSync(root)).toEqual([]);
});
