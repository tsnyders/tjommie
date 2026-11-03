import * as pdfjs from "/assets/build/pdf.mjs";
globalThis.pdfjsLib = pdfjs;
const assert = (condition, message) => { if (!condition) throw new Error(message); };
const once = (target, event) => new Promise((accept, reject) => {
  const timer = setTimeout(() => reject(new Error(`Timed out: ${event}`)), 20000);
  target.addEventListener(event, () => { clearTimeout(timer); accept(); }, { once: true });
  target.addEventListener("error", () => { clearTimeout(timer); reject(new Error(`Media error ${target.error?.code}`)); }, { once: true });
});

window.finished = (async () => {
  async function pdfProbe() {
  window.stage = "viewer module";
  const { EventBus, PDFViewer, PDFLinkService, PDFFindController } = await import("/assets/web/pdf_viewer.mjs");
  pdfjs.GlobalWorkerOptions.workerSrc = "/assets/build/pdf.worker.mjs";
  const head = await fetch("/attachment/pdf", { method: "HEAD" }), length = Number(head.headers.get("content-length"));
  let rangeCalls = 0, rangeBytes = 0;
  class Ranges extends pdfjs.PDFDataRangeTransport {
    requestDataRange(begin, end) {
      (async () => {
        if (end - begin > 2 * 1024 * 1024) throw new Error("PDF requested more than the provisional 2 MiB range budget");
        const range = new Uint8Array(end - begin);
        for (let offset = begin; offset < end; offset += 262144) {
          const response = await fetch("/attachment/pdf", { headers: { Range: `bytes=${offset}-${Math.min(end, offset + 262144) - 1}` } });
          assert(response.status === 206, "Missing PDF range response");
          const bytes = await response.arrayBuffer(); rangeCalls++; rangeBytes += bytes.byteLength; range.set(new Uint8Array(bytes), offset - begin);
        }
        this.onDataRange(begin, range.buffer);
      })().catch((error) => { window.pdfRangeError = String(error); });
    }
  }
  const range = new Ranges(length, new Uint8Array(0), true);
  const loading = pdfjs.getDocument({ range, length, rangeChunkSize: 262144, disableStream: true, disableAutoFetch: true, enableXfa: false, disableWorkerRendering: true, standardFontDataUrl: "/assets/standard_fonts/", cMapUrl: "/assets/cmaps/", wasmUrl: "/assets/wasm/" });
  window.stage = "PDF loading";
  const document = await loading.promise, eventBus = new EventBus(), link = new PDFLinkService({ eventBus }), find = new PDFFindController({ eventBus, linkService: link });
  const container = globalThis.document.getElementById("container"), viewer = new PDFViewer({ container, eventBus, linkService: link, findController: find, annotationMode: pdfjs.AnnotationMode.DISABLE, annotationEditorMode: pdfjs.AnnotationEditorType.DISABLE, maxCanvasPixels: 8 * 1024 * 1024 });
  link.setViewer(viewer); link.setDocument(document); find.setDocument(document);
  eventBus.on("pagesinit", () => { viewer.currentScale = 1; viewer.update(); });
  const pdf = { version: pdfjs.version, pages: document.numPages, length, worker: Boolean(loading._worker?.port), scriptingManager: "not constructed", annotations: "disabled" };
  const parsedTexts = [];
  for (let page = 1; page <= document.numPages; page++) parsedTexts.push((await (await document.getPage(page)).getTextContent()).items.map((item) => item.str || "").join(" "));
  pdf.parsedText = parsedTexts[0]; pdf.parsedSearchMatches = parsedTexts.filter((text) => text.includes("Searchable")).length;
  try {
    window.stage = "PDF text layer";
    const rendered = new Promise((accept, reject) => { const timer = setTimeout(() => reject(new Error("PDF text layer timed out")), 15000); eventBus.on("textlayerrendered", () => { clearTimeout(timer); accept(); }); }); viewer.setDocument(document); await rendered;
    const textNode = globalThis.document.querySelector(".textLayer span"); assert(textNode, "Missing selectable PDF text");
    const selection = getSelection(); selection.selectAllChildren(textNode); pdf.selectedText = selection.toString(); selection.removeAllRanges();
    const matches = new Promise((accept, reject) => {
      const timer = setTimeout(() => reject(new Error("PDF search timed out")), 15000);
      eventBus.on("updatefindmatchescount", (event) => { if (event.matchesCount.total === 3) { clearTimeout(timer); accept(3); } });
    });
    window.stage = "PDF search";
    eventBus.dispatch("find", { type: "", query: "Searchable", caseSensitive: false, entireWord: false, highlightAll: true, findPrevious: false, matchDiacritics: false });
    pdf.searchMatches = await matches; viewer.currentPageNumber = 3; viewer.currentScale = 1.25;
    pdf.currentPage = viewer.currentPageNumber; pdf.scale = viewer.currentScale;
  } catch (error) { pdf.viewerError = String(error); }
  pdf.rangeCalls = rangeCalls; pdf.rangeBytes = rangeBytes;
  viewer.setDocument(null); await loading.destroy();
  const bad = pdfjs.getDocument({ url: "/attachment/invalid", disableWorkerRendering: true }); let corruptRejected = false;
  try { await bad.promise; } catch { corruptRejected = true; } finally { await bad.destroy(); } assert(corruptRejected, "Corrupt PDF accepted"); pdf.corruptRejected = corruptRejected;
  return pdf;
  }
  const audio = globalThis.document.createElement("audio"), video = globalThis.document.createElement("video");
  window.stage = "audio metadata";
  audio.controls = true; audio.preload = "metadata"; audio.src = "/attachment/audio"; globalThis.document.body.append(audio); await once(audio, "loadedmetadata");
  const audioSeek = once(audio, "seeked"); audio.currentTime = 20; await audioSeek;
  const audioResult = { canPlay: audio.canPlayType('audio/wav; codecs="1"'), duration: audio.duration, currentTime: audio.currentTime, seeked: !audio.seeking && audio.currentTime >= 19.9 };
  window.stage = "video metadata";
  video.controls = true; video.preload = "metadata"; video.src = "/attachment/video"; globalThis.document.body.append(video); await once(video, "loadedmetadata");
  const videoSeek = once(video, "seeked"); video.currentTime = 9; await videoSeek;
  const videoResult = { canPlay: video.canPlayType('video/mp4; codecs="avc1.42E01E,mp4a.40.2"'), duration: video.duration, currentTime: video.currentTime, seeked: !video.seeking && video.currentTime >= 8.9, videoWidth: video.videoWidth, videoHeight: video.videoHeight };
  audio.removeAttribute("src"); audio.load(); video.removeAttribute("src"); video.load();
  const suffix = await fetch("/attachment/audio", { headers: { Range: "bytes=-9" } }); assert(suffix.status === 206 && (await suffix.arrayBuffer()).byteLength === 9, "Suffix range failed");
  const invalidRange = await fetch("/attachment/audio", { headers: { Range: "bytes=999999999-" } }); assert(invalidRange.status === 416, "Invalid range accepted");
  const unknown = await fetch("/attachment/missing"); assert(unknown.status === 404, "Unknown attachment accepted");
  let pdfTimer;
  const pdf = await Promise.race([pdfProbe(), new Promise((accept) => { pdfTimer = setTimeout(() => accept({ viewerError: `P0 PDF native gate timed out at ${window.stage}; ${window.pdfRangeError || "no range exception recorded"}` }), 22000); })]); clearTimeout(pdfTimer);
  return { pdf, audio: audioResult, video: videoResult, suffixRange: suffix.status, invalidRange: invalidRange.status, unknownAttachment: unknown.status, userAgent: navigator.userAgent };
})().catch((error) => ({ error: String(error.stack || error) }));
