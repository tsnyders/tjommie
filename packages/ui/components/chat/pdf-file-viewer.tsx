"use client";

import { useEffect, useId, useRef, useState, type FormEvent } from "react";
import { ChevronLeft, ChevronRight, Minus, Plus } from "lucide-react";
import { PdfRangeQueue } from "@/lib/document-ranges";
import { FIELD, Label } from "../controls";
import { IconButton } from "../icon-button";
import { Pill } from "../pill";

type PdfText = { items: { str?: string }[] };
type PdfPage = { getViewport(options: { scale: number }): { width: number; height: number; userUnit: number }; render(options: Record<string, unknown>): { promise: Promise<void>; cancel(): void }; getTextContent(): Promise<PdfText>; cleanup(): boolean };
type PdfDocument = { numPages: number; getPage(page: number): Promise<PdfPage>; cleanup(): Promise<void> };
type PdfLoading = { promise: Promise<PdfDocument>; onPassword?: (submit: (value: string) => void, reason: number) => void; destroy(): Promise<void> };
type Range = { onDataRange(begin: number, bytes: ArrayBuffer): void; requestDataRange(begin: number, end: number): void; abort(): void };
type PdfApi = {
  GlobalWorkerOptions: { workerSrc: string };
  PDFDataRangeTransport: new (length: number, initial: Uint8Array, done: boolean) => Range;
  getDocument(options: Record<string, unknown>): PdfLoading;
  TextLayer: new (options: Record<string, unknown>) => { render(): Promise<void>; cancel(): void };
};
export function PdfFileViewer({ url, size }: { url: string; size: number }) {
  const passwordId = useId(), searchId = useId();
  const [loaded, setLoaded] = useState<{ api: PdfApi; document: PdfDocument } | null>(null), [page, setPage] = useState(1), [zoom, setZoom] = useState(1), [error, setError] = useState<string | null>(null);
  const [password, setPassword] = useState<{ submit: (value: string) => void; incorrect: boolean } | null>(null), [query, setQuery] = useState(""), [found, setFound] = useState<{ query: string; page: number } | null>(null), [searching, setSearching] = useState(false), [searchNote, setSearchNote] = useState("");
  const input = useRef<HTMLInputElement>(null), canvas = useRef<HTMLCanvasElement>(null), textLayer = useRef<HTMLDivElement>(null), alive = useRef<AbortController | null>(null);
  useEffect(() => {
    const abort = new AbortController(); alive.current = abort; let loading: PdfLoading | undefined;
    void (async () => {
      const moduleUrl = new URL("/pdfjs/build/pdf.mjs", window.location.href).href;
      const api = await import(/* webpackIgnore: true */ moduleUrl) as PdfApi; if (abort.signal.aborted) return;
      api.GlobalWorkerOptions.workerSrc = new URL("/pdfjs/build/pdf.worker.mjs", window.location.href).href;
      class Ranges extends api.PDFDataRangeTransport {
        #queue = new PdfRangeQueue({ url, size, signal: abort.signal, deliver: (begin, bytes) => this.onDataRange(begin, bytes), fail: () => { if (!abort.signal.aborted) { setError("The PDF could not be read. Download is still available."); void loading?.destroy().catch(() => {}); } } });
        requestDataRange(begin: number, end: number) { this.#queue.request(begin, end); }
        abort() { abort.abort(); }
      }
      loading = api.getDocument({ range: new Ranges(size, new Uint8Array(0), true), length: size, rangeChunkSize: 262144, disableStream: true, disableAutoFetch: true, enableXfa: false, disableWorkerRendering: true, canvasMaxAreaInBytes: 32 * 1024 * 1024, standardFontDataUrl: "/pdfjs/standard_fonts/", cMapUrl: "/pdfjs/cmaps/", wasmUrl: "/pdfjs/wasm/" });
      loading.onPassword = (submit, reason) => { if (!abort.signal.aborted) setPassword({ submit, incorrect: reason === 2 }); };
      const document = await loading.promise; if (!abort.signal.aborted) { setPassword(null); setLoaded({ api, document }); }
    })().catch(() => { if (!abort.signal.aborted) setError("This PDF is corrupt, unsupported, or its local reader is unavailable. Download is still available."); });
    return () => { abort.abort(); alive.current = null; void loading?.destroy().catch(() => {}); };
  }, [url, size]);
  useEffect(() => {
    if (!loaded || !canvas.current || !textLayer.current) return;
    const target = canvas.current, layer = textLayer.current, abort = new AbortController(); let render: ReturnType<PdfPage["render"]> | undefined, text: InstanceType<PdfApi["TextLayer"]> | undefined, current: PdfPage | undefined;
    layer.replaceChildren();
    void (async () => {
      current = await loaded.document.getPage(page); if (abort.signal.aborted) return;
      const viewport = current.getViewport({ scale: zoom }), ratio = Math.min(devicePixelRatio || 1, Math.sqrt(8 * 1024 * 1024 / (viewport.width * viewport.height)));
      if (![viewport.width, viewport.height, ratio].every((value) => Number.isFinite(value) && value > 0)) throw new Error("Invalid PDF page dimensions.");
      target.width = Math.max(1, Math.floor(viewport.width * ratio)); target.height = Math.max(1, Math.floor(viewport.height * ratio)); target.style.width = `${viewport.width}px`; target.style.height = `${viewport.height}px`;
      layer.style.width = `${viewport.width}px`; layer.style.height = `${viewport.height}px`; layer.style.setProperty("--scale-factor", String(zoom)); layer.style.setProperty("--total-scale-factor", String(zoom * viewport.userUnit));
      render = current.render({ canvasContext: target.getContext("2d"), viewport, transform: [ratio, 0, 0, ratio, 0, 0], annotationMode: 0 }); await render.promise;
      if (abort.signal.aborted) return;
      const textContentSource = await current.getTextContent();
      if (abort.signal.aborted) return;
      text = new loaded.api.TextLayer({ textContentSource, container: layer, viewport }); await text.render();
    })().catch(() => { if (!abort.signal.aborted) setError("The PDF page could not be rendered. Download is still available."); }).finally(() => { current?.cleanup(); });
    return () => { abort.abort(); render?.cancel(); text?.cancel(); layer.replaceChildren(); target.width = 0; target.height = 0; };
  }, [loaded, page, zoom]);
  const search = async (event: FormEvent) => {
    event.preventDefault(); if (!loaded || !query.trim() || searching) return; setSearching(true); setSearchNote(""); const term = query.toLocaleLowerCase(), start = found?.query === query ? found.page % loaded.document.numPages + 1 : 1;
    try {
      for (let at = 0; at < loaded.document.numPages; at++) {
        if (alive.current?.signal.aborted ?? true) return;
        const index = (start - 1 + at) % loaded.document.numPages + 1, current = await loaded.document.getPage(index);
        try { const content = await current.getTextContent(); if (content.items.map((item) => item.str ?? "").join(" ").toLocaleLowerCase().includes(term)) { if (alive.current && !alive.current.signal.aborted) { setFound({ query, page: index }); setPage(index); setSearchNote(`Match on page ${index}`); } return; } }
        finally { if (index !== page) current.cleanup(); }
      }
      if (alive.current && !alive.current.signal.aborted) { setFound(null); setSearchNote("No matching pages"); }
    } catch { if (alive.current && !alive.current.signal.aborted) setSearchNote("PDF search could not finish."); } finally { if (alive.current && !alive.current.signal.aborted) setSearching(false); }
  };
  if (error) return <p role="alert" className="text-sm text-danger">{error}</p>;
  if (password) return <form onSubmit={(event) => { event.preventDefault(); const value = input.current?.value ?? ""; if (input.current) input.current.value = ""; password.submit(value); }} className="flex flex-wrap items-end gap-2">
    <div className="w-full min-w-0 sm:flex-1"><Label htmlFor={passwordId}>{password.incorrect ? "Incorrect password. Try again" : "PDF password"}</Label><input id={passwordId} ref={(element) => { if (input.current && input.current !== element) input.current.value = ""; input.current = element; }} type="password" autoComplete="off" className={`${FIELD} mt-1 h-11`} /></div>
    <Pill primary type="submit" className="max-sm:min-h-11">Open PDF</Pill><p className="w-full text-[12px] leading-[18px] text-secondary">Used only for this open document.</p>
  </form>;
  return <div className="flex h-full min-h-0 flex-col gap-2">
    <div className="flex flex-wrap items-center gap-2">
      <IconButton label="Previous page" className="max-sm:size-11 disabled:opacity-40" disabled={!loaded || page <= 1} onClick={() => setPage(page - 1)}><ChevronLeft aria-hidden /></IconButton><span role="status" className="text-[13px] leading-[18px] text-secondary">{loaded ? `${page} / ${loaded.document.numPages}` : "Loading PDF…"}</span><IconButton label="Next page" className="max-sm:size-11 disabled:opacity-40" disabled={!loaded || page >= loaded.document.numPages} onClick={() => setPage(page + 1)}><ChevronRight aria-hidden /></IconButton>
      <IconButton label="Zoom PDF out" className="max-sm:size-11 disabled:opacity-40" disabled={zoom <= 0.5} onClick={() => setZoom(Math.max(0.5, zoom / 1.25))}><Minus aria-hidden /></IconButton><IconButton label="Zoom PDF in" className="max-sm:size-11 disabled:opacity-40" disabled={zoom >= 4} onClick={() => setZoom(Math.min(4, zoom * 1.25))}><Plus aria-hidden /></IconButton>
    </div>
    <form onSubmit={(event) => { void search(event); }} className="flex flex-wrap items-end gap-2">
      <div className="w-full min-w-0 sm:flex-1"><Label htmlFor={searchId}>Search PDF</Label><input id={searchId} aria-label="Search PDF" maxLength={256} value={query} onChange={(event) => setQuery(event.target.value)} className={`${FIELD} mt-1 h-11`} /></div><Pill type="submit" className="max-sm:min-h-11" disabled={!loaded || searching}>{searching ? "Searching…" : found?.query === query ? "Next matching page" : "Search"}</Pill><span role="status" className="w-full text-[12px] leading-[18px] text-secondary">{searchNote}</span>
    </form>
    <div className="min-h-0 flex-1 overflow-auto" tabIndex={0} aria-label="PDF page"><div className="relative w-fit bg-white"><canvas ref={canvas} aria-label={`PDF page ${page}`} /><div ref={textLayer} className="textLayer absolute inset-0" /></div></div>
  </div>;
}
