import { ATTACHMENT_CHUNK_BYTES, AttachmentInfo, type AttachmentNativeApi, type AttachmentNativeRead } from "../../protocol/src/attachments.ts";

export type AttachmentPlugin = {
  startSave(options: { info: AttachmentInfo; heartbeatId: string }): Promise<{ saveId: string }>;
  ackSaveHeartbeat(options: { heartbeatId: string; challenge: string; alive: boolean }): Promise<void>;
  appendSave(options: { saveId: string; offset: number; bytes: string }): Promise<void>;
  finishSave(options: { saveId: string }): Promise<{ status: "saved" | "cancelled" }>;
  cancelSave(options: { saveId: string }): Promise<void>;
  preview(options: { info: AttachmentInfo }): Promise<{ previewId: string; url: string }>;
  respond(options: { previewId: string; requestId: string; bytes: string | null }): Promise<void>;
  closePreview(options: { previewId: string }): Promise<void>;
  invalidate(): Promise<void>;
  addListener(event: "read", listener: (request: AttachmentNativeRead) => void): Promise<{ remove(): Promise<void> }>;
  addListener(event: "saveHeartbeat", listener: (request: { heartbeatId: string; challenge: string | null }) => void): Promise<{ remove(): Promise<void> }>;
};
export type CaptureSaveHeartbeat = (hostId: string) => ((signal: AbortSignal) => Promise<boolean>) | null;
export function attachmentApi(plugin: AttachmentPlugin, captureHeartbeat?: CaptureSaveHeartbeat): AttachmentNativeApi {
  type Lease = { close(): void; cancelled: Promise<{ status: "cancelled" }>; saveId?: string };
  const active = new Set<Lease>(), saves = new Map<string, Lease>();
  return {
    startSave: async (value) => {
      const info = AttachmentInfo.parse(value), health = captureHeartbeat?.(info.hostId);
      if (!health) throw new Error("A current connection is required to save this file.");
      const heartbeatId = crypto.randomUUID(), stopped = new AbortController(); let closed = false, checking = false;
      let registration: Promise<{ remove(): Promise<void> }> | undefined;
      let cancel: (value: { status: "cancelled" }) => void = () => {};
      const cancelled = new Promise<{ status: "cancelled" }>((resolve) => { cancel = resolve; });
      const lease: Lease = { cancelled, close: () => {
        if (closed) return; closed = true; stopped.abort(); cancel({ status: "cancelled" }); active.delete(lease); if (lease.saveId) saves.delete(lease.saveId);
        void registration?.then((handle) => handle.remove()).catch(() => {});
      } };
      active.add(lease);
      try {
        registration = plugin.addListener("saveHeartbeat", (request) => {
          if (closed || request?.heartbeatId !== heartbeatId) return;
          if (request.challenge === null) { lease.close(); return; }
          if (checking || typeof request.challenge !== "string" || !/^[0-9a-f-]{36}$/.test(request.challenge)) return;
          const challenge = request.challenge;
          checking = true;
          void (async () => {
            let alive = false; try { alive = await health(stopped.signal); } catch { /* A failed authenticated check cannot renew a native save. */ }
            if (!closed) await plugin.ackSaveHeartbeat({ heartbeatId, challenge, alive });
          })().catch(() => {}).finally(() => { checking = false; });
        });
        await registration;
        if (closed || !await health(stopped.signal) || closed) throw new Error("Download closed.");
        const save = await plugin.startSave({ info, heartbeatId });
        if (closed) { await plugin.cancelSave({ saveId: save.saveId }); throw new Error("Download closed."); }
        lease.saveId = save.saveId; saves.set(save.saveId, lease); return save;
      } catch (error) { lease.close(); throw error; }
    },
    appendSave: (saveId, offset, bytes) => {
      if (!Number.isSafeInteger(offset) || offset < 0 || bytes.length > 4 * Math.ceil(ATTACHMENT_CHUNK_BYTES / 3)) return Promise.reject(new Error("Invalid download chunk."));
      return plugin.appendSave({ saveId, offset, bytes });
    },
    finishSave: async (saveId) => {
      const lease = saves.get(saveId); if (!lease) throw new Error("Download closed.");
      try { return await Promise.race([plugin.finishSave({ saveId }), lease.cancelled]); } finally { lease.close(); }
    },
    cancelSave: (saveId) => { saves.get(saveId)?.close(); return plugin.cancelSave({ saveId }); },
    preview: (info) => plugin.preview({ info: AttachmentInfo.parse(info) }),
    respond: (previewId, requestId, bytes) => plugin.respond({ previewId, requestId, bytes }), closePreview: (previewId) => plugin.closePreview({ previewId }),
    invalidate: () => { for (const lease of active) lease.close(); return plugin.invalidate(); },
    onRead: (listener) => {
      let closed = false;
      const pending = plugin.addListener("read", (request) => { if (!closed) listener(request); });
      return () => { closed = true; void pending.then((handle) => handle.remove()).catch(() => {}); };
    },
  };
}
