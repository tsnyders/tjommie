"use client";

import { Download, FolderOpen, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { FileData } from "@tjommie/protocol";
import type { AttachmentRef } from "../../../protocol/src/attachments";
import { attachmentDevice } from "@/lib/attachment-device";
import { attachmentError, downloadAttachment } from "@/lib/attachment-transfer";
import { captureAttachmentDownload } from "@/lib/connection";
import { pauseThumbnailWork } from "@/lib/file-thumbnails";
import { fileSize } from "@/lib/format";
import { localFileActions, useLocalFileActions } from "@/lib/native-capability";
import { useApp } from "@/lib/store";
import { IconButton } from "../icon-button";
import { FileViewer, previewKind } from "./file-viewer";
import { FileThumbnail } from "./file-thumbnail";

// Reads use message identity; the legacy path is used only by the separate local reveal action.
export function FileCard({ file, messageId, attachmentIndex = 0 }: { file: Pick<FileData, "path" | "name"> & Partial<Pick<FileData, "size" | "attachmentId" | "contentType" | "companions">>; messageId?: string; attachmentIndex?: number }) {
  const files = useLocalFileActions();
  const host = useApp((state) => state.host), connected = useApp((state) => state.connection.status === "ready");
  const [preview, setPreview] = useState<{ host: string | null } | null>(null), [status, setStatus] = useState<string | null>(null), [saving, setSaving] = useState(false), active = useRef<AbortController | null>(null);
  const transferring = useRef(false);
  const native = attachmentDevice();
  const type = { markdown: "Markdown", image: "Image", pdf: "PDF", audio: "Audio", video: "Video", unsupported: "File" }[previewKind(file.contentType ?? "")];
  useEffect(() => {
    const hidden = () => { if (document.visibilityState === "hidden") setPreview(null); };
    document.addEventListener("visibilitychange", hidden);
    return () => { active.current?.abort(); document.removeEventListener("visibilitychange", hidden); };
  }, [host]);
  useEffect(() => { if (!connected && transferring.current) active.current?.abort(); }, [connected]);
  const reference: AttachmentRef | null = messageId ? { messageId, attachmentIndex, ...(file.attachmentId ? { attachmentId: file.attachmentId } : {}) } : null;
  const download = () => {
    if (saving) { active.current?.abort(); return; }
    if (!native || !reference) return;
    if (!connected) { setStatus("The file host is offline. Reconnect to download it."); return; }
    const abort = new AbortController(), thumbnails = pauseThumbnailWork(abort.signal); active.current = abort; transferring.current = false; setSaving(true); setStatus("Preparing download…");
    let context: ReturnType<typeof captureAttachmentDownload> | undefined;
    let cleanupFailed = false;
    void thumbnails.ready.then(() => {
      abort.signal.throwIfAborted();
      context = captureAttachmentDownload(abort);
      setStatus("Choose where to save…");
      return downloadAttachment(context.client, reference, native, abort.signal, (done, total) => setStatus(`Downloading ${total ? Math.round(done * 100 / total) : 100}%`), {
        reacquire: context.reacquire,
        transferring: (client) => { transferring.current = true; context!.transferring(client); },
      });
    })
      .then((result) => { if (!abort.signal.aborted) setStatus(result === "saved" ? "Saved on this device." : "Download cancelled."); })
      .catch((error: unknown) => { cleanupFailed = typeof error === "object" && error !== null && "code" in error && error.code === "cleanup_failed"; if (!abort.signal.aborted || cleanupFailed) setStatus(attachmentError(error)); })
      .finally(() => { context?.close(); thumbnails.release(); if (active.current === abort) { active.current = null; transferring.current = false; setSaving(false); if (abort.signal.aborted && !cleanupFailed) setStatus("Download cancelled."); } });
  };
  return (
    <div data-slot="file-card" className="w-[264px] max-w-full rounded-[12px] bg-bubble p-3">
      <button type="button" aria-label={`Preview ${file.name}`} title={!connected ? "The file host is offline" : native && reference ? "Open file preview" : "Preview is unavailable on this device"} disabled={!native || !reference || !connected || saving} onClick={() => setPreview({ host })} className="press block min-h-11 w-full rounded-[8px] text-left disabled:opacity-50">
        <FileThumbnail reference={reference} native={native} type={file.contentType ?? ""} size={file.size} active={connected && !saving && !preview} host={host} />
      </button>
      <div className="mt-2 flex flex-wrap items-center gap-2">
        <span className="min-w-0 grow basis-1/2">
          <span title={file.name} className="line-clamp-2 break-words text-[13px] font-semibold leading-[18px] text-title">{file.name}</span>
          <span className="block text-[12px] leading-[18px] text-caption">{type}{file.size !== undefined ? ` · ${fileSize(file.size)}` : ""}</span>
        </span>
        <div className="ml-auto flex shrink-0 items-center">
          <IconButton label={saving ? `Cancel download of ${file.name}` : `Download ${file.name}`} disabled={!native || !reference} onClick={download} className="max-sm:size-11 disabled:opacity-40">{saving ? <X aria-hidden /> : <Download aria-hidden />}</IconButton>
          {files && <IconButton label={`Show ${file.name} in its folder`} onClick={() => localFileActions()?.showFile(file.path)} className="max-sm:size-11"><FolderOpen aria-hidden /></IconButton>}
        </div>
      </div>
      {status && <p role="status" className="mt-2 text-[12px] leading-[18px] text-secondary">{status}</p>}
      {preview?.host === host && connected && native && reference && <FileViewer key={`${messageId}-${attachmentIndex}-${file.attachmentId ?? "legacy"}`} reference={reference} name={file.name} companions={file.companions} native={native} onClose={() => setPreview(null)} onDownload={download} />}
    </div>
  );
}
