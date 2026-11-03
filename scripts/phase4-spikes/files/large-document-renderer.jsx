import React from "react";
import { createRoot } from "react-dom/client";
import { PdfFileViewer } from "../../../packages/ui/components/chat/pdf-file-viewer.tsx";
import { MarkdownFileViewer } from "../../../packages/ui/components/chat/markdown-file-viewer.tsx";

const wait = async (read, description) => {
  const start = performance.now();
  while (!read()) { if (performance.now() - start > 30000) throw new Error(`Timed out: ${description}; ${document.body.textContent}`); await new Promise(resolve => setTimeout(resolve, 20)); }
};
const click = label => { const button = [...document.querySelectorAll("button")].find(item => item.textContent === label); if (!button) throw new Error(`Missing button ${label}`); button.click(); };
const input = (element, value) => { Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(element, value); element.dispatchEvent(new Event("input", { bubbles: true })); };
window.finished = (async () => {
  const fixture = await (await fetch("/fixture.json")).json(), root = createRoot(document.getElementById("root"));
  window.stage = "PDF render"; root.render(<PdfFileViewer url={fixture.pdf.url} size={fixture.pdf.size} />);
  await wait(() => document.querySelector(".textLayer span")?.textContent.includes("Searchable alpha"), "actual PDF text layer");
  const canvas = document.querySelector("canvas"), textNode = document.querySelector(".textLayer span"), selection = getSelection();
  selection.selectAllChildren(textNode); const selectedText = selection.toString(); selection.removeAllRanges();
  if (!selectedText.includes("Searchable alpha")) throw new Error("PDF selection failed");
  const canvasPixels = canvas.width * canvas.height;
  window.stage = "PDF search"; input(document.querySelector('[aria-label="Search PDF"]'), "beta"); click("Search");
  await wait(() => document.body.textContent.includes("Match on page 2") && document.querySelector(".textLayer span")?.textContent.includes("Searchable beta"), "actual PDF search/navigation");
  document.querySelector('[aria-label="Zoom PDF in"]').click();
  await wait(() => document.querySelector("canvas")?.style.width === "765px", "PDF zoom");
  root.render(null); await wait(() => !document.querySelector("canvas"), "PDF close");
  await new Promise(resolve => setTimeout(resolve, 200));
  window.stage = "Markdown render";
  let imageCloses = 0;
  window.fixtureClient = { call: async (method) => method === "attachments.open" ? { transferId: fixture.image.id, attachment: fixture.image.info, chunkBytes: 262144, expiresAt: "2026-11-03T23:59:59Z" } : {} };
  const native = { preview: async () => ({ previewId: fixture.image.id, url: fixture.image.url }), onRead: () => () => {}, closePreview: async () => { imageCloses++; } };
  root.render(<MarkdownFileViewer url={fixture.markdown.url} size={fixture.markdown.size} native={native} reference={{ messageId: "fixture", attachmentIndex: 0 }} companions={[{ name: "chart.png", attachmentId: fixture.image.info.id, attachmentIndex: 1 }]} />);
  await wait(() => document.querySelector(".markdown h1")?.textContent === "FIRST LARGE MARKDOWN SECTION", "first Markdown section");
  click("Show Chart"); await wait(() => document.querySelector(".markdown img")?.naturalWidth === 1, "explicit inline companion");
  input(document.querySelector('[aria-label="Markdown section"]'), "6400");
  await wait(() => document.querySelector(".markdown")?.textContent.includes("LAST LARGE MARKDOWN SECTION"), "last Markdown section");
  await wait(() => imageCloses === 1, "companion lease close on section change");
  const markdownNodes = document.querySelector(".markdown").querySelectorAll("*").length, markdownCharacters = document.querySelector(".markdown").textContent.length;
  root.unmount(); window.stage = "complete";
  return { selectedText, canvasPixels, pdfSearchPage: 2, pdfZoom: 1.25, markdownSections: 6400, markdownNodes, markdownCharacters, imageCloses, jsHeap: performance.memory?.usedJSHeapSize, userAgent: navigator.userAgent };
})().catch(error => ({ error: String(error.stack || error), stage: window.stage }));
