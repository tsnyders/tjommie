"use client";

import { useEffect, useId, useRef, useState, type FormEvent } from "react";
import { ChevronLeft, ChevronRight } from "lucide-react";
import { attachmentCompanionName, type AttachmentCompanion, type AttachmentNativeApi, type AttachmentRef } from "../../../protocol/src/attachments";
import { attachmentError, previewAttachment } from "@/lib/attachment-transfer";
import { daemon } from "@/lib/connection";
import { findMarkdownText, MARKDOWN_SECTION_BYTES, markdownSection } from "@/lib/markdown-document";
import { FIELD, Label } from "../controls";
import { IconButton } from "../icon-button";
import { Pill } from "../pill";
import { MarkdownText } from "./markdown";

const NO_COMPANIONS: AttachmentCompanion[] = [];
type Selection = { section: number; name: string };
function CompanionRaster({ url, alt, onError }: { url: string; alt: string; onError: () => void }) {
  // eslint-disable-next-line @next/next/no-img-element -- App-owned scoped resource, never a remote image.
  return <img src={url} alt={alt} className="max-w-full" onError={onError} />;
}
export function MarkdownFileViewer({ url, size, reference, companions = NO_COMPANIONS, native, closeImageRef }: { url: string; size: number; reference: AttachmentRef; companions?: AttachmentCompanion[]; native: AttachmentNativeApi; closeImageRef?: { current: () => Promise<void> } }) {
  const sectionId = useId(), searchId = useId();
  const [section, setSection] = useState(0), [content, setContent] = useState<{ section: number; text: string } | null>(null), [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState(""), [searchNote, setSearchNote] = useState(""), [searching, setSearching] = useState(false), [match, setMatch] = useState<{ query: string; offset: number } | null>(null);
  const [selected, setSelected] = useState<Selection | null>(null), [image, setImage] = useState<{ selection: Selection; url: string } | null>(null), [imageError, setImageError] = useState<{ selection: Selection; text: string } | null>(null);
  const searchAbort = useRef<AbortController | null>(null), previousImage = useRef(Promise.resolve()), scroll = useRef<HTMLDivElement>(null);
  const sections = Math.max(1, Math.ceil(size / MARKDOWN_SECTION_BYTES)), selectedName = selected?.section === section ? selected.name : null;
  useEffect(() => {
    const abort = new AbortController();
    void markdownSection(url, size, section, abort.signal).then((value) => { if (!abort.signal.aborted) { setContent({ section, text: value.text }); setError(null); scroll.current?.scrollTo(0, 0); } }).catch((failure: unknown) => { if (!abort.signal.aborted) setError(failure instanceof TypeError ? "This document is not valid UTF-8 Markdown. Download is still available." : attachmentError(failure)); });
    return () => abort.abort();
  }, [url, size, section]);
  useEffect(() => () => searchAbort.current?.abort(), [url]);
  useEffect(() => {
    const abort = new AbortController(), companion = companions.find((item) => item.name === selectedName);
    const opening = previousImage.current.then(async () => {
      if (!companion || !selected || abort.signal.aborted) return;
      const preview = await previewAttachment(daemon(), { messageId: reference.messageId, attachmentIndex: companion.attachmentIndex, attachmentId: companion.attachmentId }, native, abort.signal, (failure) => { if (!abort.signal.aborted) setImageError({ selection: selected, text: attachmentError(failure) }); });
      if (!/^image\/(png|jpeg|webp|gif)$/.test(preview.attachment.contentType)) { await preview.close(); throw new Error("The companion is not a supported image."); }
      if (!abort.signal.aborted) { setImage({ selection: selected, url: preview.url }); setImageError(null); }
      return preview;
    }).catch((failure: unknown) => { if (!abort.signal.aborted && selected) setImageError({ selection: selected, text: attachmentError(failure) }); });
    const close = () => { abort.abort(); return previousImage.current = opening.then(async (preview) => { await preview?.close(); }); };
    if (closeImageRef) closeImageRef.current = close;
    return () => { void close().catch(() => {}); };
  }, [selected, selectedName, companions, reference.messageId, native, closeImageRef]);
  async function search(event: FormEvent): Promise<void> {
    event.preventDefault(); if (!query.trim() || searching) return;
    const abort = new AbortController(); searchAbort.current = abort; setSearching(true); setSearchNote("");
    try {
      let found = await findMarkdownText(url, size, query, match?.query === query ? Math.min(size, match.offset + 1) : 0, abort.signal);
      if (!found && match?.query === query) found = await findMarkdownText(url, size, query, 0, abort.signal);
      if (!abort.signal.aborted) { setMatch(found ? { query, offset: found.offset } : null); setSearchNote(found ? `Match in section ${found.section + 1}` : "No match found"); if (found) setSection(found.section); }
    } catch (failure: unknown) { if (!abort.signal.aborted) setSearchNote(attachmentError(failure)); }
    finally { if (searchAbort.current === abort) { searchAbort.current = null; setSearching(false); } }
  }
  return <div className="flex h-full min-h-0 flex-col gap-2">
    <div className="flex flex-wrap items-center gap-2">
      <IconButton label="Previous section" className="max-sm:size-11 disabled:opacity-40" disabled={section === 0} onClick={() => setSection(section - 1)}><ChevronLeft aria-hidden /></IconButton>
      <Label htmlFor={sectionId}>Section</Label><span className="w-20"><input id={sectionId} aria-label="Markdown section" type="number" min={1} max={sections} value={section + 1} onChange={(event) => { const value = Number(event.target.value); if (Number.isInteger(value) && value >= 1 && value <= sections) setSection(value - 1); }} className={`${FIELD} h-11`} /></span><span className="text-[13px] leading-[18px] text-secondary">/ {sections}</span>
      <IconButton label="Next section" className="max-sm:size-11 disabled:opacity-40" disabled={section + 1 >= sections} onClick={() => setSection(section + 1)}><ChevronRight aria-hidden /></IconButton>
    </div>
    {sections > 1 && <p className="text-[12px] leading-[18px] text-secondary">Large Markdown is read in sections. A block may continue in the next section. Search reads through the complete file.</p>}
    <form onSubmit={(event) => { void search(event); }} className="flex flex-wrap items-end gap-2">
      <div className="w-full min-w-0 sm:flex-1"><Label htmlFor={searchId}>Search Markdown</Label><input id={searchId} aria-label="Search Markdown" maxLength={256} value={query} onChange={(event) => setQuery(event.target.value)} className={`${FIELD} mt-1 h-11`} /></div>
      <Pill type="submit" className="max-sm:min-h-11" disabled={searching}>{match?.query === query ? "Find next" : "Search"}</Pill>
      {searching && <Pill className="max-sm:min-h-11" onClick={() => searchAbort.current?.abort()}>Cancel search</Pill>}
      <span role="status" className="w-full text-[12px] leading-[18px] text-secondary">{searching ? "Searching…" : searchNote}</span>
    </form>
    <div ref={scroll} className="min-h-0 flex-1 overflow-auto" role="region" tabIndex={0} aria-label="Markdown document; scroll to read this section" aria-busy={content?.section !== section && !error}>
      {error ? <p role="alert" className="text-sm text-danger">{error}</p> : content?.section !== section ? <p role="status">Reading section…</p> : <MarkdownText text={content.text} fileImages={(source, alt) => {
        const name = attachmentCompanionName(source), companion = companions.find((item) => item.name === name);
        if (!companion) return alt;
        return <span className="inline-block max-w-full"><Pill className="h-auto! min-h-8 max-w-full whitespace-normal! [overflow-wrap:anywhere] max-sm:min-h-11" onClick={() => setSelected(selectedName === companion.name ? null : { section, name: companion.name })}>{selectedName === companion.name ? "Hide" : "Show"} {alt || companion.name}</Pill>{selectedName === companion.name && (imageError?.selection === selected ? <span role="alert">{imageError.text}</span> : image?.selection === selected ? <CompanionRaster url={image.url} alt={alt} onError={() => { if (selected) setImageError({ selection: selected, text: "The shared image is corrupt or unsupported." }); }} /> : <span role="status">Opening image…</span>)}</span>;
      }} />}
    </div>
  </div>;
}
