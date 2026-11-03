const { app, BrowserWindow, protocol } = require("electron");
const { createReadStream, readFileSync, statSync, writeFileSync } = require("node:fs");
const { open } = require("node:fs/promises");
const { join, resolve } = require("node:path");
const { Readable } = require("node:stream");
const { httpRange } = require("./range.cjs");
const root = process.env.TJOMMIE_LARGE_DOCUMENT_ROOT, assets = process.env.TJOMMIE_LARGE_DOCUMENT_ASSETS;
if (!root || !assets) throw new Error("A disposable fixture root and local PDF assets are required.");
app.setPath("userData", join(root, "profile")); app.setPath("sessionData", join(root, "session")); app.disableHardwareAcceleration();
protocol.registerSchemesAsPrivileged([{ scheme: "app", privileges: { standard: true, secure: true, supportFetchAPI: true, stream: true } }]);
const ids = { pdf: "41f8bac7-358c-48cf-9f48-a3a358804da6", markdown: "83c70656-e163-478e-a934-6a5801f5ef18", image: "624d43d7-1a49-4d79-8d1d-85d846cfbd8e" };
const records = new Map(Object.entries(ids).map(([kind, id]) => [id, { kind, path: join(root, kind === "pdf" ? "large.pdf" : kind === "markdown" ? "large.md" : "chart.png"), mime: kind === "pdf" ? "application/pdf" : kind === "markdown" ? "text/markdown" : "image/png" }]));
const requests = [], samples = [];
let maxReadBytes = 0, activeReads = 0, maxActiveReads = 0;
async function serve(request) {
  const url = new URL(request.url), headers = { "cache-control": "no-store", "x-content-type-options": "nosniff" };
  if (url.pathname === "/") return new Response('<!doctype html><link rel="stylesheet" href="/pdfjs/web/pdf_viewer.css"><style>html,body,#root{height:100%;margin:0}#root>div{height:100%;display:flex;flex-direction:column}canvas{display:block}.textLayer{position:absolute;inset:0}.markdown{overflow-wrap:anywhere}</style><div id="root"></div><script type="module" src="/renderer.mjs"></script>', { headers: { ...headers, "content-type": "text/html", "content-security-policy": "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' blob: data:; worker-src 'self'; font-src 'self' blob:; object-src 'none'; base-uri 'none'" } });
  if (url.pathname === "/fixture.json") {
    const item = kind => ({ url: `app://tjommie/_attachments/${ids[kind]}`, size: statSync(records.get(ids[kind]).path).size });
    return new Response(JSON.stringify({ pdf: item("pdf"), markdown: item("markdown"), image: { ...item("image"), id: ids.image, info: { id: ids.image, hostId: "fixture", name: "chart.png", size: item("image").size, contentType: "image/png", sha256: "a".repeat(64) } } }), { headers: { ...headers, "content-type": "application/json" } });
  }
  if (url.pathname === "/renderer.mjs") return new Response(readFileSync(join(root, "renderer.mjs")), { headers: { ...headers, "content-type": "text/javascript" } });
  if (url.pathname.startsWith("/pdfjs/")) {
    const path = resolve(assets, url.pathname.slice(7));
    if (!path.startsWith(resolve(assets) + "\\")) return new Response(null, { status: 404 });
    return new Response(Readable.toWeb(createReadStream(path)), { headers: { ...headers, "content-type": path.endsWith(".mjs") ? "text/javascript" : path.endsWith(".css") ? "text/css" : "application/octet-stream" } });
  }
  const record = records.get(url.pathname.slice("/_attachments/".length)); if (!record || !url.pathname.startsWith("/_attachments/")) return new Response(null, { status: 404 });
  const size = statSync(record.path).size; let range;
  try { range = httpRange(request.headers.get("range"), size); } catch { return new Response(null, { status: 416 }); }
  const begin = range?.start ?? 0, end = range?.end ?? size - 1, entry = { kind: record.kind, begin, end, bytes: 0 }; requests.push(entry);
  const file = await open(record.path, "r"); let at = begin, closed = false;
  const close = async () => { if (!closed) { closed = true; await file.close(); } };
  const stream = new ReadableStream({ async pull(controller) {
    if (closed) return;
    if (at > end) { await close(); controller.close(); return; }
    const bytes = Buffer.alloc(Math.min(262144, end - at + 1)); maxReadBytes = Math.max(maxReadBytes, bytes.length); activeReads++; maxActiveReads = Math.max(maxActiveReads, activeReads);
    try { const result = await file.read(bytes, 0, bytes.length, at); if (result.bytesRead !== bytes.length) throw new Error("Incomplete fixture read"); at += bytes.length; entry.bytes += bytes.length; controller.enqueue(bytes); }
    catch (error) { await close(); controller.error(error); } finally { activeReads--; }
  }, cancel: close }, { highWaterMark: 0 });
  return new Response(stream, { status: range ? 206 : 200, headers: { ...headers, "content-type": record.mime, "content-length": String(end - begin + 1), ...(range ? { "content-range": `bytes ${begin}-${end}/${size}` } : {}) } });
}
app.whenReady().then(async () => {
  protocol.handle("app", serve); const win = new BrowserWindow({ show: false, width: 1000, height: 800, webPreferences: { sandbox: true, contextIsolation: true, nodeIntegration: false, backgroundThrottling: false, offscreen: true } });
  win.webContents.setFrameRate(30); win.webContents.startPainting();
  win.webContents.session.setPermissionRequestHandler((_contents, _permission, accept) => accept(false));
  const sample = () => samples.push({ at: Date.now(), processes: app.getAppMetrics().map(({ type, memory }) => ({ type, memory })) }); sample(); const timer = setInterval(sample, 50);
  await win.loadURL("app://tjommie/"); const renderer = await win.webContents.executeJavaScript("window.finished"); clearInterval(timer); sample();
  const result = { electron: process.versions.electron, chromium: process.versions.chrome, renderer, maxReadBytes, maxActiveReads, requests, samples };
  writeFileSync(join(root, "result.json"), JSON.stringify(result)); win.destroy(); app.exit(renderer.error ? 1 : 0);
}).catch(error => { console.error(error); app.exit(1); });
