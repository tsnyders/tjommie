import { ATTACHMENT_CHUNK_BYTES, AttachmentInfo, type AttachmentNativeApi, type AttachmentNativeRead } from "../../protocol/src/attachments.ts";

export type AttachmentPlugin = {
  startSave(options: { info: AttachmentInfo; requestId: string }): Promise<{ saveId?: string }>;
  activateSave(options: { saveId: string }): Promise<void>;
  appendSave(options: { saveId: string; offset: number; bytes: string }): Promise<void>;
  finishSave(options: { saveId: string }): Promise<{ status: "saved" | "cancelled" }>;
  cancelSave(options: { saveId: string }): Promise<void>;
  preview(options: { info: AttachmentInfo }): Promise<{ previewId: string; url: string }>;
  respond(options: { previewId: string; requestId: string; bytes: string | null }): Promise<void>;
  closePreview(options: { previewId: string }): Promise<void>;
  invalidate(options?: { scope: "transfers" }): Promise<void>;
  addListener(event: "read", listener: (request: AttachmentNativeRead) => void): Promise<{ remove(): Promise<void> }>;
};
export type AttachmentSaveGuard = { current(): boolean; activate(): (() => boolean) | null };
export type CaptureAttachmentSave = () => AttachmentSaveGuard | null;

export function attachmentApi(plugin: AttachmentPlugin, captureSave?: CaptureAttachmentSave): AttachmentNativeApi {
  type Save = { id: string; guard: AttachmentSaveGuard; active?: () => boolean; closed: boolean; cleanup?: Promise<void>; close(): Promise<void>; cancelled: Promise<{ status: "cancelled" }> };
  const saves = new Map<string, Save>(), retiring = new Map<string, Promise<void>>();
  const current = (id: string, active: boolean): Save => {
    const save = saves.get(id);
    if (!save || save.closed || !save.guard.current() || active && (!save.active || !save.active())) { void save?.close().catch(() => {}); throw new Error("Download closed."); }
    return save;
  };
  return {
    startSave: async (value) => {
      const info = AttachmentInfo.parse(value), guard = captureSave?.();
      if (!guard?.current()) throw new Error("A current connection is required to save this file.");
      const id = crypto.randomUUID(); let cancel: (value: { status: "cancelled" }) => void = () => {};
      const cancelled = new Promise<{ status: "cancelled" }>((resolve) => { cancel = resolve; });
      const save: Save = { id, guard, closed: false, cancelled, close: () => {
        if (save.cleanup) return save.cleanup; save.closed = true; saves.delete(id); cancel({ status: "cancelled" });
        save.cleanup = plugin.cancelSave({ saveId: id }); retiring.set(id, save.cleanup);
        void save.cleanup.then(() => retiring.delete(id), () => {}); return save.cleanup;
      } };
      saves.set(id, save);
      try {
        const chosen = await Promise.race([plugin.startSave({ info, requestId: id }), cancelled]);
        if (!("saveId" in chosen) || !chosen.saveId) { await save.close(); return null; }
        if (chosen.saveId !== id || save.closed || !guard.current()) { await save.close(); throw new Error("Download closed."); }
        return { saveId: id };
      } catch (error) { try { await save.close(); } finally { retiring.delete(id); } throw error; }
    },
    activateSave: async (id) => {
      const save = current(id, false), active = save.guard.activate();
      if (!active || !active()) { await save.close(); throw new Error("Download closed."); }
      save.active = active; // A concurrent disconnect now cancels this operation, including a late activation.
      try { await plugin.activateSave({ saveId: id }); current(id, true); } catch (error) { await save.close(); throw error; }
    },
    appendSave: (saveId, offset, bytes) => {
      if (!Number.isSafeInteger(offset) || offset < 0 || bytes.length > 4 * Math.ceil(ATTACHMENT_CHUNK_BYTES / 3)) return Promise.reject(new Error("Invalid download chunk."));
      try { current(saveId, true); return plugin.appendSave({ saveId, offset, bytes }); } catch (error) { return Promise.reject(error); }
    },
    finishSave: async (saveId) => {
      const save = current(saveId, true); let published = false;
      try { const result = await Promise.race([plugin.finishSave({ saveId }), save.cancelled]); published = result.status === "saved"; return result; }
      finally { if (published) await save.close().catch(() => {}); else await save.close(); }
    },
    cancelSave: async (saveId) => { try { await (saves.get(saveId)?.close() ?? retiring.get(saveId) ?? plugin.cancelSave({ saveId })); } finally { retiring.delete(saveId); } },
    preview: (info) => plugin.preview({ info: AttachmentInfo.parse(info) }),
    respond: (previewId, requestId, bytes) => plugin.respond({ previewId, requestId, bytes }), closePreview: (previewId) => plugin.closePreview({ previewId }),
    invalidate: (options) => { for (const save of saves.values()) if (!options || save.active) void save.close().catch(() => {}); return plugin.invalidate(options); },
    onRead: (listener) => {
      let closed = false;
      const pending = plugin.addListener("read", (request) => { if (!closed) listener(request); });
      return () => { closed = true; void pending.then((handle) => handle.remove()).catch(() => {}); };
    },
  };
}
