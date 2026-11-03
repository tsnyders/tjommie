import { ATTACHMENT_CHUNK_BYTES, AttachmentInfo, type AttachmentNativeApi, type AttachmentNativeRead } from "../../protocol/src/attachments.ts";

export type AttachmentPlugin = {
  startSave(options: { info: AttachmentInfo }): Promise<{ saveId: string }>;
  appendSave(options: { saveId: string; offset: number; bytes: string }): Promise<void>;
  finishSave(options: { saveId: string }): Promise<{ status: "saved" | "cancelled" }>;
  cancelSave(options: { saveId: string }): Promise<void>;
  preview(options: { info: AttachmentInfo }): Promise<{ previewId: string; url: string }>;
  respond(options: { previewId: string; requestId: string; bytes: string | null }): Promise<void>;
  closePreview(options: { previewId: string }): Promise<void>;
  invalidate(): Promise<void>;
  addListener(event: "read", listener: (request: AttachmentNativeRead) => void): Promise<{ remove(): Promise<void> }>;
};
export function attachmentApi(plugin: AttachmentPlugin): AttachmentNativeApi {
  return {
    startSave: (info) => plugin.startSave({ info: AttachmentInfo.parse(info) }),
    appendSave: (saveId, offset, bytes) => {
      if (!Number.isSafeInteger(offset) || offset < 0 || bytes.length > 4 * Math.ceil(ATTACHMENT_CHUNK_BYTES / 3)) return Promise.reject(new Error("Invalid download chunk."));
      return plugin.appendSave({ saveId, offset, bytes });
    },
    finishSave: (saveId) => plugin.finishSave({ saveId }), cancelSave: (saveId) => plugin.cancelSave({ saveId }),
    preview: (info) => plugin.preview({ info: AttachmentInfo.parse(info) }),
    respond: (previewId, requestId, bytes) => plugin.respond({ previewId, requestId, bytes }), closePreview: (previewId) => plugin.closePreview({ previewId }),
    invalidate: () => plugin.invalidate(),
    onRead: (listener) => {
      let closed = false;
      const pending = plugin.addListener("read", (request) => { if (!closed) listener(request); });
      return () => { closed = true; void pending.then((handle) => handle.remove()).catch(() => {}); };
    },
  };
}
