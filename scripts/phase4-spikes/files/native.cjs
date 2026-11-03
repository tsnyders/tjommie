const { app, BrowserWindow, protocol } = require("electron");
const assert = require("node:assert/strict");
const { createHash } = require("node:crypto");
const { createReadStream, existsSync, readFileSync, statSync, writeFileSync } = require("node:fs");
const { open, rename, rm } = require("node:fs/promises");
const { join, resolve } = require("node:path");
const { Readable } = require("node:stream");
const { httpRange } = require("./range.cjs");
const root = process.env.TJOMMIE_FILES_SPIKE_ROOT, assets = process.env.TJOMMIE_FILES_SPIKE_ASSETS;
if (!root || !assets) throw new Error("P0 native fixture needs disposable paths");
app.setPath("userData", join(root, "profile")); app.setPath("sessionData", join(root, "session"));
app.disableHardwareAcceleration();
protocol.registerSchemesAsPrivileged([{ scheme: "filespike", privileges: { standard: true, secure: true, supportFetchAPI: true, stream: true } }]);
const CHUNK = 256 * 1024;
const csp = "default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' blob: data:; worker-src 'self'; font-src 'self' blob:; media-src 'self'; object-src 'none'; base-uri 'none'";
const registry = new Map([["pdf", { file: join(root, "document.pdf"), mime: "application/pdf" }], ["video", { file: join(root, "movie.mp4"), mime: "video/mp4" }], ["audio", { file: join(root, "sound.wav"), mime: "audio/wav" }], ["invalid", { file: join(root, "invalid.pdf"), mime: "application/pdf" }]]);
const requests = [], serving = { activeBytes: 0, maxActiveBytes: 0, maxChunkBytes: 0 };
let revoked = false;

async function serve(request) {
  const url = new URL(request.url), headers = { "cache-control": "no-store", "x-content-type-options": "nosniff" };
  if (url.host !== "app") return new Response(null, { status: 404, headers });
  if (url.pathname === "/index.html") return new Response('<!doctype html><link rel="stylesheet" href="/assets/web/pdf_viewer.css"><div id="container" style="position:absolute;inset:0;overflow:auto"><div id="viewer" class="pdfViewer"></div></div><script type="module" src="/probe.mjs"></script>', { headers: { ...headers, "content-type": "text/html", "content-security-policy": csp } });
  if (url.pathname === "/probe.mjs") return new Response(readFileSync(join(__dirname, "renderer.mjs")), { headers: { ...headers, "content-type": "text/javascript" } });
  if (url.pathname.startsWith("/assets/")) {
    const name = url.pathname.slice(8), file = resolve(assets, name);
    const allowed = ["build/pdf.mjs", "build/pdf.worker.mjs", "web/pdf_viewer.mjs", "web/pdf_viewer.css"].includes(name) || /^(standard_fonts|cmaps|wasm)\/[A-Za-z0-9_.-]+$/.test(name);
    if (!allowed || !file.startsWith(`${resolve(assets)}\\`) || !existsSync(file)) return new Response(null, { status: 404, headers });
    const mime = name.endsWith(".mjs") ? "text/javascript" : name.endsWith(".css") ? "text/css" : "application/octet-stream";
    return new Response(Readable.toWeb(createReadStream(file)), { headers: { ...headers, "content-type": mime } });
  }
  const id = /^\/attachment\/([a-z]+)$/.exec(url.pathname)?.[1], item = registry.get(id);
  if (!item) return new Response(null, { status: 404, headers });
  if (revoked) return new Response(null, { status: 403, headers });
  if (request.method !== "GET" && request.method !== "HEAD") return new Response(null, { status: 405, headers });
  const size = statSync(item.file).size;
  let range;
  try { range = request.method === "HEAD" ? null : httpRange(request.headers.get("range"), size); }
  catch { return new Response(null, { status: 416, headers: { ...headers, "content-range": `bytes */${size}` } }); }
  const start = range?.start ?? 0, end = range?.end ?? size - 1;
  const entry = { id, method: request.method, range: request.headers.get("range"), start, end, served: 0 }; requests.push(entry);
  const responseHeaders = { ...headers, "accept-ranges": "bytes", "content-length": String(end - start + 1), "content-type": item.mime };
  if (range) responseHeaders["content-range"] = `bytes ${start}-${end}/${size}`;
  if (request.method === "HEAD") return new Response(null, { headers: responseHeaders });
  const fd = await open(item.file, "r"); let at = start, closed = false;
  const close = async () => { if (!closed) { closed = true; await fd.close(); } };
  const stream = new ReadableStream({
    async pull(controller) {
      if (closed) return;
      if (revoked) { await close(); controller.error(new Error("revoked")); return; }
      if (at > end) { await close(); controller.close(); return; }
      const bytes = Buffer.alloc(Math.min(CHUNK, end - at + 1)); serving.activeBytes += bytes.length;
      serving.maxChunkBytes = Math.max(serving.maxChunkBytes, bytes.length); serving.maxActiveBytes = Math.max(serving.maxActiveBytes, serving.activeBytes);
      try { const { bytesRead } = await fd.read(bytes, 0, bytes.length, at); if (bytesRead !== bytes.length) throw new Error("short read"); at += bytesRead; entry.served += bytesRead; controller.enqueue(bytes); }
      catch (error) { await close(); controller.error(error); }
      finally { serving.activeBytes -= bytes.length; }
    },
    cancel: close,
  }, { highWaterMark: 0 });
  return new Response(stream, { status: range ? 206 : 200, headers: responseHeaders });
}

async function hash(file) { const digest = createHash("sha256"); for await (const bytes of createReadStream(file)) digest.update(bytes); return digest.digest("hex"); }
async function save(source, destination, expected, cancel = false) {
  const temporary = `${destination}.part`, input = await open(source, "r"), output = await open(temporary, "wx"), digest = createHash("sha256");
  let complete = false;
  try {
    for (let at = 0;;) {
      const buffer = Buffer.alloc(CHUNK), { bytesRead } = await input.read(buffer, 0, buffer.length, at); if (!bytesRead) break;
      if (cancel) throw new Error("cancelled");
      const bytes = buffer.subarray(0, bytesRead); digest.update(bytes);
      for (let written = 0; written < bytes.length;) written += (await output.write(bytes, written)).bytesWritten;
      at += bytesRead;
    }
    assert.equal(digest.digest("hex"), expected, "corrupt"); await output.sync(); complete = true;
  } finally { await input.close(); await output.close(); if (!complete) await rm(temporary, { force: true }); }
  await rename(temporary, destination);
}

app.whenReady().then(async () => {
  protocol.handle("filespike", serve);
  const win = new BrowserWindow({ show: false, width: 900, height: 700, webPreferences: { sandbox: true, contextIsolation: true, nodeIntegration: false, backgroundThrottling: false } });
  win.webContents.session.setPermissionRequestHandler((_contents, _permission, accept) => accept(false));
  const metrics = () => app.getAppMetrics().map(({ type, memory }) => ({ type, memory }));
  const before = metrics();
  await win.loadURL("filespike://app/index.html");
  let watchdog;
  const renderer = await Promise.race([win.webContents.executeJavaScript("window.finished"), new Promise((_accept, reject) => { watchdog = setTimeout(async () => reject(new Error(await win.webContents.executeJavaScript("JSON.stringify({stage:window.stage,rangeError:window.pdfRangeError})"))), 40000); })]); clearTimeout(watchdog);
  assert(!renderer.error, renderer.error);
  const memory = metrics();
  const source = join(root, "movie.mp4"), destination = join(root, "saved.mp4"), expected = await hash(source);
  await save(source, destination, expected); assert.equal(await hash(destination), expected);
  const protectedDestination = join(root, "keep.bin"); writeFileSync(protectedDestination, "unchanged");
  await assert.rejects(save(source, protectedDestination, expected, true), /cancelled/);
  assert.equal(readFileSync(protectedDestination, "utf8"), "unchanged"); assert(!existsSync(`${protectedDestination}.part`));
  await assert.rejects(save(source, protectedDestination, "invalid-digest"), /corrupt/);
  assert.equal(readFileSync(protectedDestination, "utf8"), "unchanged"); assert(!existsSync(`${protectedDestination}.part`));
  revoked = true;
  const afterRevoke = await win.webContents.executeJavaScript("fetch('/attachment/pdf', { headers: { Range: 'bytes=0-9' } }).then(response => response.status)"); assert.equal(afterRevoke, 403);
  const totals = Object.fromEntries([...registry.keys()].map((id) => [id, { size: statSync(registry.get(id).file).size, requests: requests.filter((entry) => entry.id === id).length, served: requests.filter((entry) => entry.id === id).reduce((sum, entry) => sum + entry.served, 0) }]));
  writeFileSync(join(root, "result.json"), JSON.stringify({ electron: process.versions.electron, chrome: process.versions.chrome, os: process.platform, renderer, memoryKiB: { before, after: memory }, serving, totals, requests, nativeSave: { bytes: statSync(destination).size, sha256: expected, cancellationKeptExisting: true, corruptKeptExisting: true, picker: "not invoked" }, afterRevoke }, null, 2));
  win.destroy(); app.quit();
}).catch((error) => { console.error(error); app.exit(1); });
