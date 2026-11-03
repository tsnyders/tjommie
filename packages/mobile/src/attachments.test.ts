import { describe, expect, it, vi } from "vitest";
import { attachmentApi, type AttachmentPlugin } from "./attachments.ts";

describe("native attachment wrapper", () => {
  it("removes a native read subscription even when disposal precedes listener registration", async () => {
    let registered: (value: { remove(): Promise<void> }) => void = () => {};
    const remove = vi.fn(async () => {}), plugin = { addListener: vi.fn(() => new Promise((accept) => { registered = accept as typeof registered; })) } as unknown as AttachmentPlugin;
    const close = attachmentApi(plugin).onRead(() => {}); close(); registered({ remove }); await Promise.resolve(); expect(remove).toHaveBeenCalledTimes(1);
  });
  it("rejects oversized download chunks before crossing the native bridge", async () => {
    const appendSave = vi.fn(), api = attachmentApi({ appendSave } as unknown as AttachmentPlugin);
    await expect(api.appendSave("save", 0, "A".repeat(349532))).rejects.toThrow(/Invalid/); expect(appendSave).not.toHaveBeenCalled();
  });
});
