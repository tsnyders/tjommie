import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { closeSync, cpSync, existsSync, ftruncateSync, mkdirSync, mkdtempSync, openSync, readFileSync, rmSync, writeFileSync, writeSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { rolldown } from "rolldown";
import { boundedPdfWorker } from "../../pdfjs-worker.ts";

const here = dirname(fileURLToPath(import.meta.url)), repo = resolve(here, "../../.."), root = mkdtempSync(join(tmpdir(), "tjommie-large-documents-")), size = 200 * 1024 * 1024;
function pdf() {
  const objects = ["<< /Type /Catalog /Pages 2 0 R >>", "<< /Type /Pages /Kids [3 0 R 5 0 R 7 0 R] /Count 3 >>", "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 9 0 R >> >> /Contents 4 0 R >>", "", "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 9 0 R >> >> /Contents 6 0 R >>", "", "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 9 0 R >> >> /Contents 8 0 R >>", "", "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"];
  for (const [index, word] of [[3, "alpha"], [5, "beta"], [7, "gamma"]]) { const stream = (index === 3 ? ("%" + "x".repeat(1022) + "\n").repeat(3072) : "") + `BT /F1 18 Tf 72 700 Td (PHASE4 Searchable ${word}) Tj ET`; objects[index] = `<< /Length ${Buffer.byteLength(stream)} >>\nstream\n${stream}\nendstream`; }
  let content = "%PDF-1.7\n"; const offsets = [0];
  for (const [index, object] of objects.entries()) { offsets.push(Buffer.byteLength(content)); content += `${index + 1} 0 obj\n${object}\nendobj\n`; }
  offsets.push(Buffer.byteLength(content));
  const xref = `xref\n0 11\n0000000000 65535 f \n${offsets.slice(1).map(offset => `${String(offset).padStart(10, "0")} 00000 n \n`).join("")}trailer\n<< /Size 11 /Root 1 0 R >>\nstartxref\n`;
  const xrefAt = size - Buffer.byteLength(`${xref}${size}\n%%EOF\n`), closing = "\nendstream\nendobj\n", dataEnd = xrefAt - Buffer.byteLength(closing);
  let padding = size;
  for (let at = 0; at < 3; at++) padding = dataEnd - Buffer.byteLength(`${content}10 0 obj\n<< /Length ${padding} >>\nstream\n`);
  const prefix = Buffer.from(`${content}10 0 obj\n<< /Length ${padding} >>\nstream\n`), tail = Buffer.from(`${closing}${xref}${xrefAt}\n%%EOF\n`), file = openSync(join(root, "large.pdf"), "wx");
  try { writeSync(file, prefix); ftruncateSync(file, size); writeSync(file, tail, 0, tail.length, dataEnd); } finally { closeSync(file); }
}
try {
  pdf();
  const file = openSync(join(root, "large.md"), "wx"), block = Buffer.from("Synthetic offline content.\n".repeat(2600).slice(0, 65536));
  try { for (let at = 0; at < size; at += block.length) writeSync(file, block, 0, Math.min(block.length, size - at), at); const first = Buffer.from("# FIRST LARGE MARKDOWN SECTION\n\n![Chart](chart.png)\n\n"), last = Buffer.from("# LAST LARGE MARKDOWN SECTION\n\n"); writeSync(file, first, 0, first.length, 0); writeSync(file, last, 0, last.length, size - 32768); } finally { closeSync(file); }
  writeFileSync(join(root, "chart.png"), Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aR5sAAAAASUVORK5CYII=", "base64"));
  const assets = join(root, "pdfjs"), source = join(repo, "node_modules/pdfjs-dist"); mkdirSync(assets);
  for (const name of ["build", "web", "standard_fonts", "cmaps", "wasm"]) cpSync(join(source, name), join(assets, name), { recursive: true });
  writeFileSync(join(assets, "build/pdf.worker.mjs"), boundedPdfWorker(readFileSync(join(source, "build/pdf.worker.mjs"), "utf8")));
  const connection = join(root, "fixture-connection.mjs"); writeFileSync(connection, "export const daemon = () => window.fixtureClient;\n");
  const build = await rolldown({ input: join(here, "large-document-renderer.jsx"), platform: "browser", transform: { define: { "process.env.NODE_ENV": '"production"' }, jsx: { runtime: "automatic" } }, onwarn(warning, forward) { if (warning.code !== "MODULE_LEVEL_DIRECTIVE") forward(warning); }, plugins: [{ name: "fixture-ui-alias", resolveId(name) { if (name === "@/lib/connection") return connection; if (name.startsWith("@/")) { const path = join(repo, "packages/ui", name.slice(2)); for (const suffix of ["", ".ts", ".tsx"]) if (existsSync(path + suffix)) return path + suffix; } } }] });
  try { await build.write({ file: join(root, "renderer.mjs"), format: "esm" }); } finally { await build.close(); }
  const env = { ...process.env, TJOMMIE_LARGE_DOCUMENT_ROOT: root, TJOMMIE_LARGE_DOCUMENT_ASSETS: assets }; delete env.ELECTRON_RUN_AS_NODE;
  const child = spawn(join(repo, "node_modules/electron/dist/electron.exe"), [join(here, "large-document-native.cjs")], { env, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] }); let logs = "";
  child.stdout.on("data", bytes => { logs += bytes; }); child.stderr.on("data", bytes => { logs += bytes; }); const timer = setTimeout(() => child.kill(), 90000);
  const status = await new Promise(resolve => child.once("exit", resolve)); clearTimeout(timer);
  const result = existsSync(join(root, "result.json")) ? JSON.parse(readFileSync(join(root, "result.json"), "utf8")) : { logs };
  assert.equal(status, 0, JSON.stringify({ ...result.renderer, requests: result.requests?.length, pdfBytes: result.requests?.filter(item => item.kind === "pdf").reduce((sum, item) => sum + item.bytes, 0), logs }));
  assert.equal(result.renderer.pdfSearchPage, 2); assert.equal(result.renderer.imageCloses, 1); assert(result.renderer.markdownCharacters <= 32771); assert(result.renderer.canvasPixels <= 8 * 1024 * 1024);
  const pdfReads = result.requests.filter(request => request.kind === "pdf"), mdReads = result.requests.filter(request => request.kind === "markdown"), rendererSamples = result.samples.flatMap(sample => sample.processes.filter(process => process.type === "Tab").map(process => process.memory.privateBytes));
  assert(pdfReads.length > 8); assert(pdfReads.every(request => request.end - request.begin + 1 <= 262144)); assert.equal(mdReads.length, 2); assert.equal(result.maxReadBytes, 262144);
  console.log(JSON.stringify({ ...result.renderer, electron: result.electron, chromium: result.chromium, fileBytes: size, pdfRequests: pdfReads.length, pdfBytes: pdfReads.reduce((sum, request) => sum + request.bytes, 0), markdownRequests: mdReads.length, markdownBytes: mdReads.reduce((sum, request) => sum + request.bytes, 0), maxReadBytes: result.maxReadBytes, maxActiveReads: result.maxActiveReads, rendererPrivateKiB: { first: rendererSamples[0], peak: Math.max(...rendererSamples), final: rendererSamples.at(-1) }, boundary: "Actual React components and patched pinned PDF.js worker in a hidden isolated Electron profile; generated 200MiB documents and a 3MiB PDF content stream. Not installed app, remote Noise, picker or Android hardware acceptance." }, null, 2));
} finally {
  assert.equal(dirname(resolve(root)), resolve(tmpdir())); assert(root.startsWith(join(resolve(tmpdir()), "tjommie-large-documents-"))); rmSync(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
}
