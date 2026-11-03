import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { saveDiagnosticReport } from "../components/settings/save-report";
import type { AttachmentInfo } from "../../protocol/src/attachments";
const native = vi.hoisted(() => ({ capture: vi.fn(), startSave: vi.fn(async ({ requestId }: { requestId: string }) => ({ saveId: requestId })), activateSave: vi.fn(async () => {}), appendSave: vi.fn(async () => {}), finishSave: vi.fn(async () => ({ status: "saved" as const })), cancelSave: vi.fn(async () => {}), invalidate: vi.fn(async () => {}) }));
vi.mock("@capacitor/core", () => ({ Capacitor: { isNativePlatform: () => true }, registerPlugin: () => native }));
vi.mock("./connection", () => ({ captureAttachmentSave: native.capture }));
vi.mock("./desktop", () => ({ desktop: () => null }));
const info: AttachmentInfo = { id: "a2a10531-c884-4178-889a-b4f7af49edab", hostId: "diagnostics", name: "forged.md", size: 0, contentType: "text/markdown", sha256: "a".repeat(64) };
beforeEach(() => { vi.resetModules(); native.capture.mockReset(); native.capture.mockReturnValue(null); native.startSave.mockClear(); native.activateSave.mockClear(); native.cancelSave.mockClear(); });
afterEach(() => vi.unstubAllGlobals());
it("requires trusted current selection even if remote metadata claims diagnostics", async () => {
  const { attachmentDevice } = await import("./attachment-device"); await expect(attachmentDevice()!.startSave(info)).rejects.toThrow(/connection/); expect(native.capture).toHaveBeenCalledOnce(); expect(native.startSave).not.toHaveBeenCalled();
});
it("allows only the trusted local-export factory without a remote host", async () => {
  const { attachmentDeviceForLocalExport } = await import("./attachment-device"); const api = attachmentDeviceForLocalExport(() => true)!; const save = (await api.startSave(info))!;
  await api.activateSave!(save.saveId); expect(native.capture).not.toHaveBeenCalled(); expect(native.activateSave).toHaveBeenCalledWith(save); await api.invalidate();
});
it("rejects activation when the local report unmounts while its picker is open", async () => {
  const { attachmentDeviceForLocalExport } = await import("./attachment-device"); let current = true;
  const api = attachmentDeviceForLocalExport(() => current)!, save = (await api.startSave(info))!; current = false;
  await expect(api.activateSave!(save.saveId)).rejects.toThrow(/closed/); expect(native.activateSave).not.toHaveBeenCalled(); expect(native.cancelSave).toHaveBeenCalledWith(save);
});
it("cannot start a local save after navigation overtakes the report digest", async () => {
  let digest: (value: ArrayBuffer) => void = () => {};
  vi.stubGlobal("crypto", { randomUUID: () => info.id, subtle: { digest: () => new Promise<ArrayBuffer>((resolve) => { digest = resolve; }) } });
  const { attachmentDeviceForLocalExport } = await import("./attachment-device"); let current = true;
  const api = attachmentDeviceForLocalExport(() => current)!, pending = saveDiagnosticReport("{}", api);
  const failed = expect(pending).rejects.toThrow(/connection/); current = false; await api.invalidate(); digest(new ArrayBuffer(32)); await failed; expect(native.startSave).not.toHaveBeenCalled();
});
it("uses the fresh authenticated activation guard without comparing unrelated host UUIDs", async () => {
  const { attachmentDevice } = await import("./attachment-device"); native.capture.mockReturnValue({ current: () => true, activate: () => () => true });
  const api = attachmentDevice()!, save = (await api.startSave({ ...info, hostId: "distinct-storage-uuid" }))!; await api.activateSave!(save.saveId); expect(native.activateSave).toHaveBeenCalledWith(save); await api.invalidate();
});
