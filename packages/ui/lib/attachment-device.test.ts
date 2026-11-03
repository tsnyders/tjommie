import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { saveDiagnosticReport } from "../components/settings/save-report";
import type { AttachmentInfo } from "../../protocol/src/attachments";

const native = vi.hoisted(() => ({
  capture: vi.fn(), startSave: vi.fn(async (options: { heartbeatId: string }) => { expect(options.heartbeatId).toMatch(/^[0-9a-f-]{36}$/); return { saveId: "save" }; }),
  callbacks: new Map<string, (value: unknown) => void>(),
  addListener: vi.fn(), ackSaveHeartbeat: vi.fn(async () => {}), invalidate: vi.fn(async () => {}),
}));
vi.mock("@capacitor/core", () => ({ Capacitor: { isNativePlatform: () => true }, registerPlugin: () => native }));
vi.mock("./connection", () => ({ captureAttachmentSaveHeartbeat: native.capture }));
vi.mock("./desktop", () => ({ desktop: () => null }));
const info: AttachmentInfo = { id: "a2a10531-c884-4178-889a-b4f7af49edab", hostId: "diagnostics", name: "forged.md", size: 0, contentType: "text/markdown", sha256: "a".repeat(64) };
beforeEach(() => {
  vi.resetModules(); native.capture.mockReset(); native.capture.mockReturnValue(null); native.startSave.mockClear(); native.callbacks.clear(); native.ackSaveHeartbeat.mockClear();
  native.addListener.mockImplementation(async (event: string, listener: (value: unknown) => void) => { native.callbacks.set(event, listener); return { remove: async () => {} }; });
});
afterEach(() => vi.unstubAllGlobals());

it("requires the current authenticated link even if shared metadata claims to be diagnostics", async () => {
  const { attachmentDevice } = await import("./attachment-device");
  await expect(attachmentDevice()!.startSave(info)).rejects.toThrow(/connection/i);
  expect(native.capture).toHaveBeenCalledWith("diagnostics"); expect(native.startSave).not.toHaveBeenCalled();
});
it("allows a trusted local report export without selecting a remote host", async () => {
  const { attachmentDeviceForLocalExport } = await import("./attachment-device");
  const api = attachmentDeviceForLocalExport(() => true)!; await expect(api.startSave(info)).resolves.toEqual({ saveId: "save" });
  expect(native.capture).not.toHaveBeenCalled(); expect(native.startSave).toHaveBeenCalledOnce(); await api.invalidate();
});
it("does not renew a local export after its trusted page lifecycle ends", async () => {
  const { attachmentDeviceForLocalExport } = await import("./attachment-device"); let current = true;
  const api = attachmentDeviceForLocalExport(() => current)!; await api.startSave(info); current = false;
  const heartbeatId = native.startSave.mock.calls[0][0].heartbeatId;
  native.callbacks.get("saveHeartbeat")?.({ heartbeatId, challenge: "a2a10531-c884-4178-889a-b4f7af49edab" });
  for (let turn = 0; turn < 8; turn++) await Promise.resolve();
  expect(native.ackSaveHeartbeat).toHaveBeenCalledWith({ heartbeatId, challenge: "a2a10531-c884-4178-889a-b4f7af49edab", alive: false }); await api.invalidate();
});
it("cannot start a local save after navigation overtakes the report digest", async () => {
  let digest: (value: ArrayBuffer) => void = () => {};
  vi.stubGlobal("crypto", { randomUUID: () => info.id, subtle: { digest: () => new Promise<ArrayBuffer>((resolve) => { digest = resolve; }) } });
  const { attachmentDeviceForLocalExport } = await import("./attachment-device"); let current = true;
  const api = attachmentDeviceForLocalExport(() => current)!, pending = saveDiagnosticReport("{}", api);
  const failed = expect(pending).rejects.toThrow(/connection/i); current = false; await api.invalidate(); digest(new ArrayBuffer(32)); await failed;
  expect(native.startSave).not.toHaveBeenCalled();
});
