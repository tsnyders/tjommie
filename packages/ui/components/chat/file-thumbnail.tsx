"use client";

import { File, FileText, Image, Music2, Play, Video } from "lucide-react";
import { useEffect, useRef, useState, useSyncExternalStore } from "react";
import type { AttachmentNativeApi, AttachmentRef } from "../../../protocol/src/attachments";
import { captureAttachmentSave, daemon } from "@/lib/connection";
import { onThumbnailPauseChange, queueThumbnail, readFileThumbnail, thumbnailEligible, thumbnailKind, thumbnailsPaused, type FileThumbnailContent } from "@/lib/file-thumbnails";

const LABEL = { image: "Image preview", pdf: "PDF preview", video: "Video preview", text: "Text preview", audio: "Open audio player", file: "Open file" };
const ICON = { image: Image, pdf: FileText, video: Video, text: FileText, audio: Music2, file: File };
export function FileThumbnail({ reference, native, type, size, active, host }: { reference: AttachmentRef | null; native: AttachmentNativeApi | null; type: string; size?: number; active: boolean; host: string | null }) {
  const element = useRef<HTMLSpanElement>(null), [visible, setVisible] = useState(false);
  const paused = useSyncExternalStore(onThumbnailPauseChange, thumbnailsPaused, () => false);
  const kind = thumbnailKind(type), visual = kind !== "audio" && kind !== "file";
  const identity = JSON.stringify([host, reference?.messageId, reference?.attachmentIndex, reference?.attachmentId, type, size]);
  useEffect(() => {
    let intersects = typeof IntersectionObserver === "undefined";
    const changed = () => setVisible(intersects && document.visibilityState !== "hidden");
    const observer = typeof IntersectionObserver === "undefined" ? null : new IntersectionObserver(([entry]) => { intersects = entry.isIntersecting; changed(); });
    if (element.current) observer?.observe(element.current);
    document.addEventListener("visibilitychange", changed); changed();
    return () => { observer?.disconnect(); document.removeEventListener("visibilitychange", changed); };
  }, []);
  const eligible = size === undefined || thumbnailEligible(type, size);
  return <span ref={element} data-slot="file-thumbnail" data-thumbnail-kind={kind} aria-hidden="true" className={`relative flex w-full items-center justify-center overflow-hidden bg-bubble ${visual ? "h-36" : "h-20"}`}>
    {active && visible && !paused && native && reference && visual && eligible ? <ThumbnailContent key={identity} reference={reference} native={native} kind={kind} /> : <Fallback kind={kind} />}
  </span>;
}
function Fallback({ kind }: { kind: ReturnType<typeof thumbnailKind> }) {
  const Icon = ICON[kind]; return <span className="flex items-center gap-2 text-caption"><Icon className="size-6 shrink-0" /><span className="text-xs">{LABEL[kind]}</span></span>;
}
function ThumbnailContent({ reference, native, kind }: { reference: AttachmentRef; native: AttachmentNativeApi; kind: ReturnType<typeof thumbnailKind> }) {
  const target = useRef<HTMLCanvasElement>(null), [content, setContent] = useState<FileThumbnailContent | null>(null);
  const { messageId, attachmentIndex, attachmentId } = reference;
  useEffect(() => {
    const canvas = target.current;
    const abort = new AbortController(), current = captureAttachmentSave()?.activate();
    if (!current?.()) return;
    const client = daemon();
    void queueThumbnail(abort.signal, async (signal, invalidated) => {
      if (!current()) throw new DOMException("File host changed", "AbortError");
      const value = await readFileThumbnail(client, { messageId, attachmentIndex, ...(attachmentId ? { attachmentId } : {}) }, native, signal, invalidated);
      signal.throwIfAborted(); if (!current()) throw new DOMException("File host changed", "AbortError"); return value;
    }).then((value) => {
      if (abort.signal.aborted || !current()) return;
      if ("canvas" in value && canvas) { canvas.width = value.canvas.width; canvas.height = value.canvas.height; canvas.getContext("2d")?.drawImage(value.canvas, 0, 0); value.canvas.width = 0; value.canvas.height = 0; }
      setContent(value);
    }).catch(() => {});
    return () => { abort.abort(); if (canvas) { canvas.width = 0; canvas.height = 0; } };
  }, [messageId, attachmentIndex, attachmentId, native]);
  return <>
    <canvas ref={target} className={`size-full object-contain ${content && "canvas" in content ? "" : "hidden"}`} />
    {content && "text" in content ? <span className="block size-full overflow-hidden px-4 py-3 text-left font-mono text-[11px] leading-[18px] whitespace-pre-wrap break-words text-secondary">{content.text || "Empty document"}</span> : !content && <Fallback kind={kind} />}
    {kind === "video" && content && <span className="absolute bottom-2 left-2 flex size-8 items-center justify-center rounded-full bg-card text-title"><Play className="size-4" /></span>}
    {(kind === "pdf" || kind === "text") && content && <span className="absolute right-2 bottom-2 rounded bg-card px-1.5 py-0.5 text-[12px] font-medium text-caption">{kind === "pdf" ? "Page 1" : "Text excerpt"}</span>}
  </>;
}
