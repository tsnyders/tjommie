"use client";

import { Download, Eye, FileText, FolderOpen } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import type { FileData } from "@tjommie/protocol";
import type { AttachmentRef } from "../../../protocol/src/attachments";
import { attachmentDevice } from "@/lib/attachment-device";
import { attachmentError, downloadAttachment } from "@/lib/attachment-transfer";
import { daemon } from "@/lib/connection";
import { fileSize } from "@/lib/format";
import { localFileActions, useLocalFileActions } from "@/lib/native-capability";
import { useApp } from "@/lib/store";
import { FileViewer, previewKind } from "./file-viewer";

// Reads use message identity; the legacy path is used only by the separate local reveal action.
export function FileCard({ file, messageId, attachmentIndex = 0 }: { file: Pick<FileData, "path" | "name"> & Partial<Pick<FileData, "size" | "attachmentId" | "contentType" | "companions">>; messageId?: string; attachmentIndex?: number }) {
  const files = useLocalFileActions();
  const host = useApp((state) => state.host), connected = useApp((state) => state.connection.status === "ready");
  const [preview, setPreview] = useState<{ host: string | null } | null>(null), [status, setStatus] = useState<string | null>(null), [saving, setSaving] = useState(false), active = useRef<AbortController | null>(null);
  const native = attachmentDevice();
  const type = { markdown: "Markdown", image: "Image", pdf: "PDF", audio: "Audio", video: "Video", unsupported: "File" }[previewKind(file.contentType ?? "")];
  useEffect(() => {
    const hidden = () => { if (document.visibilityState === "hidden") setPreview(null); };
    document.addEventListener("visibilitychange", hidden);
    return () => { active.current?.abort(); document.removeEventListener("visibilitychange", hidden); };
  }, [host, connected]);
  const reference: AttachmentRef | null = messageId ? { messageId, attachmentIndex, ...(file.attachmentId ? { attachmentId: file.attachmentId } : {}) } : null;
  const download = () => {
    if (saving) { active.current?.abort(); return; }
    if (!native || !reference) return;
    if (!connected) { setStatus("The file host is offline. Reconnect to download it."); return; }
    const abort = new AbortController(); active.current = abort; setSaving(true); setStatus("Preparing download…");
    void Promise.resolve().then(() => downloadAttachment(daemon(), reference, native, abort.signal, (done, total) => setStatus(`Downloading ${total ? Math.round(done * 100 / total) : 100}%`)))
      .then((result) => { if (!abort.signal.aborted) setStatus(result === "saved" ? "Saved on this device." : "Download cancelled."); })
      .catch((error: unknown) => { if (!abort.signal.aborted) setStatus(attachmentError(error)); })
      .finally(() => { if (active.current === abort) { active.current = null; setSaving(false); if (abort.signal.aborted) setStatus("Download cancelled."); } });
  };
  return (
    <div data-slot="file-card" className="w-[340px] max-w-full rounded-[12px] bg-bubble p-3">
    <div className="flex items-center gap-2">
      <span className="flex size-8 shrink-0 items-center justify-center rounded-[8px] bg-file-tile">
        <FileText className="size-4 text-danger" aria-hidden />
      </span>
      <button type="button" aria-label={`Preview ${file.name}`} title={!connected ? "The file host is offline" : native && reference ? "Preview" : "Preview is unavailable on this device"} disabled={!native || !reference || !connected} onClick={() => setPreview({ host })} className="press flex size-11 shrink-0 items-center justify-center rounded-full text-icon hover:bg-pill disabled:opacity-40"><Eye className="size-4" aria-hidden /></button>
      <button type="button" aria-label={saving ? `Cancel download of ${file.name}` : `Download ${file.name}`} title={saving ? "Cancel download" : "Download on this device"} disabled={!native || !reference} onClick={download} className="press flex size-11 shrink-0 items-center justify-center rounded-full text-icon hover:bg-pill disabled:opacity-40"><Download className="size-4" aria-hidden /></button>
      <span className="min-w-0 flex-1">
        <span className="block truncate text-[13px] font-semibold leading-[18px] text-title">{file.name}</span>
        <span className="block text-[12px] leading-[18px] text-caption">{type}{file.size !== undefined ? ` · ${fileSize(file.size)}` : ""}</span>
      </span>
      {files && <button
        type="button"
        aria-label={`Show ${file.name} in its folder`}
        title="Show in folder"
        onClick={() => localFileActions()?.showFile(file.path)}
        className="press flex size-11 shrink-0 items-center justify-center rounded-full text-icon hover:bg-pill"
      >
        <FolderOpen className="size-4" aria-hidden />
      </button>}
    </div>
      {status && <p role="status" className="mt-1 text-xs text-secondary">{status}</p>}
      {preview?.host === host && connected && native && reference && <FileViewer key={`${messageId}-${attachmentIndex}-${file.attachmentId ?? "legacy"}`} reference={reference} name={file.name} companions={file.companions} native={native} onClose={() => setPreview(null)} onDownload={download} />}
    </div>
  );
}
