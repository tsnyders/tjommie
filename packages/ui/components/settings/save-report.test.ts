import { createHash, webcrypto } from "node:crypto";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { AttachmentInfo, AttachmentNativeApi } from "../../../protocol/src/attachments";
import { saveDiagnosticReport } from "./save-report";

function bridge(result: "saved" | "cancelled" = "saved", pick = true) {
  let info: AttachmentInfo | null = null, content = "";
  const native: AttachmentNativeApi = {
    startSave: vi.fn(async (value) => { info = value; return pick ? { saveId: "save" } : null; }),
    appendSave: vi.fn(async (_id, _offset, bytes) => { content += Buffer.from(bytes, "base64").toString("utf8"); }),
    finishSave: vi.fn(async () => ({ status: result })), cancelSave: vi.fn(async () => {}),
    preview: async () => { throw new Error("unused"); }, onRead: () => () => {}, respond: async () => {}, closePreview: async () => {}, invalidate: async () => {},
  };
  return { native, info: () => info, content: () => content };
}
afterEach(() => vi.unstubAllGlobals());

describe("current-device diagnostic save", () => {
  it("saves identical bounded UTF-8 bytes with a verified digest through the system picker", async () => {
    vi.stubGlobal("crypto", webcrypto);
    const { native, info, content } = bridge();
    const report = '{"message":"device ✓"}';
    expect(await saveDiagnosticReport(report, native)).toBe("saved");
    expect(content()).toBe(report);
    expect(info()).toMatchObject({ name: "tjommie-diagnostics.json", contentType: "application/json", size: Buffer.byteLength(report), sha256: createHash("sha256").update(report).digest("hex") });
    expect(native.cancelSave).toHaveBeenCalledWith("save");
  });
  it("does not call a cancellation success or write when the picker is cancelled", async () => {
    const first = bridge("cancelled", false);
    expect(await saveDiagnosticReport("{}", first.native)).toBe("cancelled");
    expect(first.native.appendSave).not.toHaveBeenCalled();
    const second = bridge("cancelled");
    expect(await saveDiagnosticReport("{}", second.native)).toBe("cancelled");
    expect(second.native.cancelSave).toHaveBeenCalledOnce();
  });
  it("rejects oversized output before requesting a destination and cleans failed saves", async () => {
    const { native } = bridge();
    await expect(saveDiagnosticReport("x".repeat(16385), native)).rejects.toThrow("too large");
    expect(native.startSave).not.toHaveBeenCalled();
    vi.mocked(native.appendSave).mockRejectedValueOnce(new Error("interrupted"));
    await expect(saveDiagnosticReport("{}", native)).rejects.toThrow("interrupted");
    expect(native.finishSave).not.toHaveBeenCalled(); expect(native.cancelSave).toHaveBeenCalledOnce();
  });
});
