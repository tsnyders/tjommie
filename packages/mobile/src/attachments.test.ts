import { describe, expect, it, vi } from "vitest";
import type { AttachmentInfo } from "../../protocol/src/attachments.ts";
import { attachmentApi, type AttachmentPlugin, type AttachmentSaveGuard } from "./attachments.ts";
const info: AttachmentInfo = { id: "a2a10531-c884-4178-889a-b4f7af49edab", hostId: "storage-host", name: "test.md", size: 0, contentType: "text/markdown", sha256: "a".repeat(64) };
function fixture() {
  const plugin: AttachmentPlugin = { startSave: vi.fn(async ({ requestId }) => ({ saveId: requestId })), activateSave: vi.fn(async () => {}), appendSave: vi.fn(async () => {}), finishSave: vi.fn(async () => ({ status: "saved" as const })), cancelSave: vi.fn(async () => {}), preview: vi.fn(async () => ({ previewId: "p", url: "https://localhost/_attachments/p" })), respond: vi.fn(async () => {}), closePreview: vi.fn(async () => {}), invalidate: vi.fn(async () => {}), addListener: vi.fn(async () => ({ remove: async () => {} })) };
  let selected = true, connected = true; const guard: AttachmentSaveGuard = { current: () => selected, activate: () => connected ? () => selected && connected : null };
  return { plugin, api: attachmentApi(plugin, () => guard), select: (value: boolean) => { selected = value; }, connect: (value: boolean) => { connected = value; } };
}
describe("Android destination-first bridge", () => {
  it("awaits the first provider cleanup failure instead of hiding it behind a second cancellation", async () => {
    const { api, plugin } = fixture(), save = (await api.startSave(info))!; await api.activateSave!(save.saveId);
    let fail: (error: Error) => void = () => {};
    vi.mocked(plugin.cancelSave).mockImplementationOnce(() => new Promise<void>((_resolve, reject) => { fail = reject; }));
    vi.mocked(plugin.finishSave).mockRejectedValueOnce(new Error("Copy unavailable"));
    const finishing = api.finishSave(save.saveId), first = expect(finishing).rejects.toMatchObject({ code: "cleanup_failed" });
    await Promise.resolve(); await Promise.resolve(); const cancelling = api.cancelSave(save.saveId), second = expect(cancelling).rejects.toMatchObject({ code: "cleanup_failed" });
    fail(Object.assign(new Error("Provider retained document"), { code: "cleanup_failed" })); await first; await second; expect(plugin.cancelSave).toHaveBeenCalledOnce();
  });
  it("requires trusted selection even when metadata claims diagnostics", async () => {
    const { plugin } = fixture(); await expect(attachmentApi(plugin).startSave({ ...info, hostId: "diagnostics" })).rejects.toThrow(/connection/); expect(plugin.startSave).not.toHaveBeenCalled();
  });
  it("preserves a byte-free picker through transport loss and activates only after fresh readiness", async () => {
    const { api, plugin, connect } = fixture(); let choose: (value: { saveId: string }) => void = () => {}; let id = "";
    vi.mocked(plugin.startSave).mockImplementationOnce(({ requestId }) => { id = requestId; return new Promise((resolve) => { choose = resolve; }); });
    const pending = api.startSave(info); connect(false); await api.invalidate({ scope: "transfers" });
    expect(plugin.cancelSave).not.toHaveBeenCalled(); expect(plugin.activateSave).not.toHaveBeenCalled(); expect(plugin.appendSave).not.toHaveBeenCalled();
    choose({ saveId: id }); expect(await pending).toEqual({ saveId: id }); connect(true); await api.activateSave!(id);
    expect(plugin.activateSave).toHaveBeenCalledExactlyOnceWith({ saveId: id }); await api.cancelSave(id);
  });
  it("cancels active staging on real disconnect and never rebinds it", async () => {
    const { api, plugin, connect } = fixture(), save = (await api.startSave(info))!; await api.activateSave!(save.saveId);
    connect(false); await api.invalidate({ scope: "transfers" }); connect(true);
    await expect(api.appendSave(save.saveId, 0, "YQ==")).rejects.toThrow(/closed/); expect(plugin.appendSave).not.toHaveBeenCalled(); expect(plugin.cancelSave).toHaveBeenCalledWith(save);
  });
  it("hard invalidation settles an open picker and cancels its late result", async () => {
    const { api, plugin } = fixture(); let choose: (value: { saveId: string }) => void = () => {}; let id = "";
    vi.mocked(plugin.startSave).mockImplementationOnce(({ requestId }) => { id = requestId; return new Promise((resolve) => { choose = resolve; }); });
    const pending = api.startSave(info); await api.invalidate(); expect(await pending).toBeNull(); choose({ saveId: id }); await Promise.resolve(); expect(plugin.cancelSave).toHaveBeenCalledWith({ saveId: id }); expect(plugin.activateSave).not.toHaveBeenCalled();
  });
  it("rejects stale selection after the picker and checks again at activation", async () => {
    const { api, plugin, select } = fixture(), save = (await api.startSave(info))!; select(false);
    await expect(api.activateSave!(save.saveId)).rejects.toThrow(/closed/); expect(plugin.activateSave).not.toHaveBeenCalled();
  });
  it("never accepts append or finish before explicit activation", async () => {
    const { api, plugin } = fixture(), save = (await api.startSave(info))!;
    await expect(api.appendSave(save.saveId, 0, "YQ==")).rejects.toThrow(/closed/); expect(plugin.appendSave).not.toHaveBeenCalled(); expect(plugin.finishSave).not.toHaveBeenCalled();
  });
  it("cancels a late activation result and bounds every chunk", async () => {
    const { api, plugin } = fixture(), save = (await api.startSave(info))!; let release = () => {};
    vi.mocked(plugin.activateSave).mockImplementationOnce(() => new Promise<void>((resolve) => { release = resolve; }));
    const activating = api.activateSave!(save.saveId), failed = expect(activating).rejects.toThrow(/closed/); await api.invalidate({ scope: "transfers" }); release(); await failed;
    await expect(api.appendSave(save.saveId, 0, "A".repeat(349532))).rejects.toThrow(/Invalid/); expect(plugin.appendSave).not.toHaveBeenCalled();
  });
  it("preserves native cancellation and provider errors without reporting saved", async () => {
    const { api, plugin } = fixture(), save = (await api.startSave(info))!; await api.activateSave!(save.saveId); vi.mocked(plugin.finishSave).mockResolvedValueOnce({ status: "cancelled" });
    expect(await api.finishSave(save.saveId)).toEqual({ status: "cancelled" }); expect(plugin.cancelSave).toHaveBeenCalled();
  });
});
