"use client";

import { useEffect, useRef, useState } from "react";
import { Maximize, Minus, Plus } from "lucide-react";
import type { AttachmentCompanion, AttachmentInfo, AttachmentNativeApi, AttachmentRef } from "../../../protocol/src/attachments";
import { attachmentError, previewAttachment } from "@/lib/attachment-transfer";
import { daemon } from "@/lib/connection";
import { pauseThumbnailWork } from "@/lib/file-thumbnails";
import { usePhone } from "@/lib/use-phone";
import { Dialog } from "../dialog";
import { IconButton } from "../icon-button";
import { Pill } from "../pill";
import { MarkdownFileViewer } from "./markdown-file-viewer";
import { PdfFileViewer } from "./pdf-file-viewer";

export type PreviewKind = "markdown" | "image" | "pdf" | "audio" | "video" | "unsupported";
export function previewKind(type: string): PreviewKind {
  if (type === "text/markdown") return "markdown";
  if (/^image\/(png|jpeg|webp|gif)$/.test(type)) return "image";
  if (type === "application/pdf") return "pdf";
  if (/^audio\/(wav|mpeg|mp4|ogg)$/.test(type)) return "audio";
  if (/^video\/(mp4|webm)$/.test(type)) return "video";
  return "unsupported";
}

export function FileViewer({ reference, name, companions, native, onClose, onDownload }: { reference: AttachmentRef; name: string; companions?: AttachmentCompanion[]; native: AttachmentNativeApi; onClose: () => void; onDownload: () => void }) {
  const [ready, setReady] = useState<{ info: AttachmentInfo; url: string } | null>(null), [error, setError] = useState<string | null>(null), [expanded, setExpanded] = useState(false);
  const phone = usePhone();
  const closePreview = useRef<(() => Promise<void>) | undefined>(undefined), closeImage = useRef<() => Promise<void>>(async () => {}), [preparingDownload, setPreparingDownload] = useState(false);
  const { messageId, attachmentIndex, attachmentId } = reference;
  useEffect(() => {
    const abort = new AbortController(), thumbnails = pauseThumbnailWork(abort.signal); let release: (() => Promise<void>) | undefined;
    const opening = thumbnails.ready.then(() => { abort.signal.throwIfAborted(); return previewAttachment(daemon(), { messageId, attachmentIndex, ...(attachmentId ? { attachmentId } : {}) }, native, abort.signal, (failure) => { if (!abort.signal.aborted) { setError(attachmentError(failure)); void release?.().catch(() => {}); } }); }).then((preview) => {
      release = preview.close; closePreview.current = preview.close;
      if (!abort.signal.aborted) setReady({ info: preview.attachment, url: preview.url });
    }).catch((failure: unknown) => { if (!abort.signal.aborted) setError(failure instanceof RangeError ? failure.message : attachmentError(failure)); void release?.().catch(() => {}); });
    return () => {
      // This is the companion's latest resource closer, deliberately captured at teardown.
      // eslint-disable-next-line react-hooks/exhaustive-deps
      const closeCompanion = closeImage.current;
      abort.abort(); void opening.then(async () => { try { await closeCompanion(); } finally { await release?.(); } }).catch(() => {}).finally(thumbnails.release);
    };
  }, [messageId, attachmentIndex, attachmentId, native]);
  return (
    <Dialog open onOpenChange={(open) => { if (!open) onClose(); }} title={name} width={expanded ? 1280 : 960} closable className={expanded ? "h-[calc(100dvh-32px)]" : "h-[min(760px,calc(100dvh-48px))]"} actions={<span className="flex items-center gap-2">
      {!phone && <Pill onClick={() => setExpanded(!expanded)}>{expanded ? "Restore" : "Expand"}</Pill>}
      <Pill primary className="max-sm:min-h-11" disabled={preparingDownload} onClick={() => { setPreparingDownload(true); void closeImage.current().then(() => closePreview.current?.()).then(() => { onClose(); onDownload(); }).catch(() => { setError("The preview could not close. Try again."); setPreparingDownload(false); }); }}>Download</Pill>
    </span>}>
      <div className="mt-3 min-h-0 flex-1 overflow-auto" aria-busy={!ready && !error}>
        {error ? <p role="alert" className="text-sm text-danger">{error}</p> : !ready ? <p role="status" className="text-sm text-secondary">Opening file…</p> : ready.info.contentType === "text/markdown" ? <MarkdownFileViewer url={ready.url} size={ready.info.size} reference={reference} companions={companions} native={native} closeImageRef={closeImage} /> : <Preview info={ready.info} url={ready.url} />}
      </div>
    </Dialog>
  );
}
function Preview({ info, url }: { info: AttachmentInfo; url: string }) {
  switch (previewKind(info.contentType)) {
    case "image": return <ImagePreview url={url} name={info.name} />;
    case "pdf": return <PdfFileViewer url={url} size={info.size} />;
    case "audio": return <MediaPreview url={url} video={false} />;
    case "video": return <MediaPreview url={url} video />;
    default: return <p className="text-sm text-secondary">This file type has no preview. Use Download to save it on this device.</p>;
  }
}
function ImagePreview({ url, name }: { url: string; name: string }) {
  const [zoom, setZoom] = useState(1), [natural, setNatural] = useState(0), [failed, setFailed] = useState(false), scroll = useRef<HTMLDivElement>(null);
  if (failed) return <p role="alert" className="text-sm text-danger">This image is corrupt or unsupported. Download is still available.</p>;
  return <div className="flex h-full min-h-0 flex-col gap-2">
    <div className="flex items-center gap-2"><IconButton label="Zoom image out" className="max-sm:size-11 disabled:opacity-40" disabled={zoom <= 1} onClick={() => setZoom(Math.max(1, zoom / 1.5))}><Minus aria-hidden /></IconButton><IconButton label="Zoom image in" className="max-sm:size-11 disabled:opacity-40" disabled={zoom >= 6} onClick={() => setZoom(Math.min(6, zoom * 1.5))}><Plus aria-hidden /></IconButton><Pill className="max-sm:min-h-11" onClick={() => { setZoom(1); scroll.current?.scrollTo(0, 0); }}>Reset</Pill></div>
    <div ref={scroll} className="min-h-0 flex-1 overflow-auto" tabIndex={0} aria-label="Image; scroll to pan when zoomed">
      {/* eslint-disable-next-line @next/next/no-img-element -- Scoped native handles must not be fetched by the image optimizer. */}
      <img src={url} alt={name} onLoad={(event) => setNatural(event.currentTarget.naturalWidth)} onError={() => setFailed(true)} style={zoom === 1 ? { maxWidth: "100%", maxHeight: "100%", objectFit: "contain" } : { width: natural * zoom, maxWidth: "none" }} />
    </div>
  </div>;
}
function MediaPreview({ url, video }: { url: string; video: boolean }) {
  const [failed, setFailed] = useState(false), [fullscreenError, setFullscreenError] = useState(false), element = useRef<HTMLVideoElement>(null);
  if (failed) return <p role="alert" className="text-sm text-danger">This media is corrupt or its codec is unsupported on this device. Download is still available.</p>;
  return video ? <div><video ref={element} src={url} controls playsInline preload="metadata" onError={() => setFailed(true)} className="max-h-[65dvh] w-full" /><Pill className="mt-2 max-sm:min-h-11" onClick={() => { void element.current?.requestFullscreen().catch(() => setFullscreenError(true)); }}><Maximize className="size-3.5" aria-hidden />Fullscreen video</Pill>{fullscreenError && <p role="status" className="text-xs text-secondary">Fullscreen is unavailable on this device.</p>}</div> : <audio src={url} controls preload="metadata" onError={() => setFailed(true)} className="w-full" />;
}
