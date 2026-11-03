import { describe, expect, it, vi } from "vitest";
import { attachmentApi, type AttachmentPlugin } from "./attachments.ts";
import type { AttachmentInfo } from "../../protocol/src/attachments.ts";

const info: AttachmentInfo = { id: "b410d5e0-556d-4b93-b7e4-6b85d26b835d", hostId: "host", name: "file.md", size: 0, contentType: "text/markdown", sha256: "a".repeat(64) };
const challenge = "d9bf348d-a28c-4eb8-bbd1-1bf4515d26bd";
const settle = async () => { for (let turn = 0; turn < 8; turn++) await Promise.resolve(); };
function savingPlugin() {
  const listeners = new Map<string, (value: unknown) => void>(), order: string[] = [];
  const remove = vi.fn(async () => {});
  const plugin = {
    addListener: vi.fn(async (event: string, listener: (value: unknown) => void): Promise<{ remove(): Promise<void> }> => { order.push("listen"); listeners.set(event, listener); return { remove }; }),
    startSave: vi.fn(async (_options: { info: AttachmentInfo; heartbeatId?: string }) => { order.push("start"); return { saveId: "save" }; }),
    ackSaveHeartbeat: vi.fn(async (_options: { heartbeatId: string; challenge: string; alive: boolean }) => {}),
    finishSave: vi.fn(async (_options: { saveId: string }): Promise<{ status: "saved" | "cancelled" }> => ({ status: "saved" })),
    cancelSave: vi.fn(async () => {}), invalidate: vi.fn(async () => {}),
  };
  return { plugin, native: plugin as unknown as AttachmentPlugin, order, remove, emit: (value: unknown) => listeners.get("saveHeartbeat")?.(value) };
}

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
  it("registers heartbeat handling before starting and acknowledges only the save's authenticated challenge", async () => {
    const { native, plugin, order, emit } = savingPlugin(), health = vi.fn(async () => true);
    const api = attachmentApi(native, () => health);
    await api.startSave(info);
    health.mockClear();
    expect(order).toEqual(["listen", "start"]);
    const heartbeatId = plugin.startSave.mock.calls[0][0].heartbeatId;
    expect(heartbeatId).toMatch(/^[0-9a-f-]{36}$/);
    emit({ heartbeatId: "another-save", challenge }); emit({ heartbeatId, challenge: "invalid" }); await settle();
    expect(health).not.toHaveBeenCalled();
    emit({ heartbeatId, challenge }); await settle();
    expect(health).toHaveBeenCalledOnce();
    expect(plugin.ackSaveHeartbeat).toHaveBeenCalledExactlyOnceWith({ heartbeatId, challenge, alive: true });
  });
  it("reports failed authentication to native without keeping the lease alive", async () => {
    const { native, plugin, emit } = savingPlugin(); let checks = 0; const api = attachmentApi(native, () => async () => ++checks === 1);
    await api.startSave(info); const heartbeatId = plugin.startSave.mock.calls[0][0].heartbeatId;
    emit({ heartbeatId, challenge }); await settle();
    expect(plugin.ackSaveHeartbeat).toHaveBeenCalledWith({ heartbeatId, challenge, alive: false });
  });
  it("fences an authenticated reply arriving after invalidation and removes its listener", async () => {
    const { native, plugin, remove, emit } = savingPlugin(); let answer: (alive: boolean) => void = () => {};
    let checks = 0; const api = attachmentApi(native, () => () => ++checks === 1 ? Promise.resolve(true) : new Promise<boolean>((resolve) => { answer = resolve; }));
    await api.startSave(info); const heartbeatId = plugin.startSave.mock.calls[0][0].heartbeatId;
    emit({ heartbeatId, challenge }); await settle(); await api.invalidate(); answer(true); await settle();
    expect(plugin.ackSaveHeartbeat).not.toHaveBeenCalled(); expect(remove).toHaveBeenCalledOnce();
  });
  it("refuses shared saves without the current host even when metadata claims to be diagnostics", async () => {
    const { native, plugin, emit } = savingPlugin(); const api = attachmentApi(native, () => null);
    await expect(api.startSave(info)).rejects.toThrow(/connection/i); expect(plugin.startSave).not.toHaveBeenCalled();
    const localInfo = { ...info, hostId: "diagnostics", name: "tjommie-diagnostics.json", contentType: "application/json" };
    await expect(api.startSave(localInfo)).rejects.toThrow(/connection/i); expect(plugin.startSave).not.toHaveBeenCalled();
    const local = attachmentApi(native, () => async () => true); await local.startSave(localInfo);
    const heartbeatId = plugin.startSave.mock.calls[0][0].heartbeatId; emit({ heartbeatId, challenge }); await settle();
    expect(plugin.ackSaveHeartbeat).toHaveBeenCalledWith({ heartbeatId, challenge, alive: true });
  });
  it("removes heartbeat subscriptions on picker completion, failure and cancellation", async () => {
    for (const outcome of ["saved", "failed", "cancel"] as const) {
      const { native, plugin, remove, emit } = savingPlugin(); const health = vi.fn(async () => true), api = attachmentApi(native, () => health);
      await api.startSave(info); const heartbeatId = plugin.startSave.mock.calls[0][0].heartbeatId;
      health.mockClear();
      if (outcome === "failed") { plugin.finishSave.mockRejectedValueOnce(new Error("provider failed")); await expect(api.finishSave("save")).rejects.toThrow("provider failed"); }
      else if (outcome === "cancel") await api.cancelSave("save");
      else await api.finishSave("save");
      emit({ heartbeatId, challenge }); await settle(); expect(health).not.toHaveBeenCalled(); expect(remove).toHaveBeenCalledOnce();
    }
  });
  it("does not start native work if invalidation overtakes listener registration", async () => {
    const { native, plugin, remove } = savingPlugin(); let registered: (value: { remove(): Promise<void> }) => void = () => {};
    plugin.addListener.mockImplementationOnce(() => new Promise((resolve) => { registered = resolve; }));
    const api = attachmentApi(native, () => async () => true), pending = api.startSave(info);
    const failed = expect(pending).rejects.toThrow(/closed/i); await api.invalidate(); registered({ remove }); await failed; await settle();
    expect(plugin.startSave).not.toHaveBeenCalled(); expect(remove).toHaveBeenCalledOnce();
  });
  it("settles an open picker when the native Cancel action or watchdog ends its lease", async () => {
    const { native, plugin, remove, emit } = savingPlugin(); plugin.finishSave.mockImplementationOnce(() => new Promise(() => {}));
    const api = attachmentApi(native, () => async () => true); await api.startSave(info);
    const pending = api.finishSave("save"), heartbeatId = plugin.startSave.mock.calls[0][0].heartbeatId;
    emit({ heartbeatId, challenge: null }); expect(await pending).toEqual({ status: "cancelled" }); expect(remove).toHaveBeenCalledOnce();
  });
  it("rechecks the captured page/connection after listener registration before starting native work", async () => {
    const { native, plugin, remove } = savingPlugin(); let current = true, registered: (value: { remove(): Promise<void> }) => void = () => {};
    plugin.addListener.mockImplementationOnce(() => new Promise((resolve) => { registered = resolve; }));
    const pending = attachmentApi(native, () => async () => current).startSave(info);
    const failed = expect(pending).rejects.toThrow(/closed/i); current = false; registered({ remove }); await failed;
    expect(plugin.startSave).not.toHaveBeenCalled(); expect(remove).toHaveBeenCalledOnce();
  });
  it("answers a challenge emitted before native promotion has returned the save handle", async () => {
    const { native, plugin, emit } = savingPlugin(); let promoted: (value: { saveId: string }) => void = () => {};
    plugin.startSave.mockImplementationOnce(() => new Promise((resolve) => { promoted = resolve; }));
    const api = attachmentApi(native, () => async () => true), pending = api.startSave(info); await settle();
    const heartbeatId = plugin.startSave.mock.calls[0][0].heartbeatId; emit({ heartbeatId, challenge }); await settle();
    expect(plugin.ackSaveHeartbeat).toHaveBeenCalledWith({ heartbeatId, challenge, alive: true }); promoted({ saveId: "save" }); await pending;
  });
});
