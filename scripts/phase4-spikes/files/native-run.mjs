import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { closeSync, ftruncateSync, mkdtempSync, openSync, readFileSync, rmSync, statSync, writeFileSync, writeSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = mkdtempSync(join(tmpdir(), "tjommie-files-native-"));
const writeAll = (fd, buffer) => { for (let offset = 0; offset < buffer.length;) offset += writeSync(fd, buffer, offset); };

function pdf(file, targetSize) {
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R 5 0 R 7 0 R] /Count 3 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 9 0 R >> >> /Contents 4 0 R >>",
    "", "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 9 0 R >> >> /Contents 6 0 R >>",
    "", "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 9 0 R >> >> /Contents 8 0 R >>", "",
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
  ];
  for (const [index, text] of [[3, "PHASE4 Searchable alpha"], [5, "PHASE4 Searchable beta"], [7, "PHASE4 Searchable gamma"]]) {
    const stream = `BT /F1 18 Tf 72 700 Td (${text}) Tj ET`;
    objects[index] = `<< /Length ${Buffer.byteLength(stream)} >>\nstream\n${stream}\nendstream`;
  }
  let content = "%PDF-1.7\n", offsets = [0];
  for (const [index, object] of objects.entries()) { offsets.push(Buffer.byteLength(content)); content += `${index + 1} 0 obj\n${object}\nendobj\n`; }
  const prefix = Buffer.from(content);
  const xref = `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n${offsets.slice(1).map((offset) => `${String(offset).padStart(10, "0")} 00000 n \n`).join("")}trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n`;
  const suffixLength = Buffer.byteLength(`${xref}${targetSize}\n%%EOF\n`);
  const padding = targetSize - prefix.length - suffixLength;
  const tail = Buffer.from(`${xref}${prefix.length + padding}\n%%EOF\n`);
  const fd = openSync(file, "wx"), block = Buffer.alloc(65536, 32); block[0] = 37; block[block.length - 1] = 10;
  try {
    writeAll(fd, prefix);
    for (let remaining = padding; remaining > 0;) {
      const count = Math.min(remaining, block.length), piece = block.subarray(0, count); piece[count - 1] = 10; writeAll(fd, piece); remaining -= count;
    }
    writeAll(fd, tail);
  } finally { closeSync(fd); }
}

try {
  pdf(join(root, "document.pdf"), 200 * 1024 * 1024);
  const media = spawnSync("ffmpeg", ["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=24", "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=44100", "-t", "12", "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-c:a", "aac", "-movflags", "+faststart", join(root, "movie.mp4")], { encoding: "utf8", timeout: 30000, windowsHide: true });
  assert.equal(media.status, 0, `Synthetic FFmpeg fixture failed: ${media.stderr}`);
  const video = join(root, "movie.mp4"), originalVideoBytes = statSync(video).size;
  const target = 200 * 1024 * 1024, free = Buffer.alloc(8); free.writeUInt32BE(target - originalVideoBytes); free.write("free", 4);
  const fd = openSync(video, "r+"); writeSync(fd, free, 0, free.length, originalVideoBytes); ftruncateSync(fd, target); closeSync(fd);
  const wave = spawnSync("ffmpeg", ["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", "sine=frequency=220:sample_rate=8000", "-t", "30", "-c:a", "pcm_s16le", join(root, "sound.wav")], { encoding: "utf8", timeout: 30000, windowsHide: true });
  assert.equal(wave.status, 0, `Synthetic WAV fixture failed: ${wave.stderr}`);
  writeFileSync(join(root, "invalid.pdf"), "This is not a PDF.");
  const env = { ...process.env, TJOMMIE_FILES_SPIKE_ROOT: root, TJOMMIE_FILES_SPIKE_ASSETS: join(here, "runtime", "node_modules", "pdfjs-dist") };
  delete env.ELECTRON_RUN_AS_NODE;
  const child = spawn(join(here, "..", "..", "..", "node_modules", "electron", "dist", "electron.exe"), [join(here, "native.cjs")], { env, stdio: ["ignore", "pipe", "pipe"], windowsHide: true });
  let stdout = "", stderr = "";
  child.stdout.on("data", (bytes) => { stdout += bytes; }); child.stderr.on("data", (bytes) => { stderr += bytes; });
  const timer = setTimeout(() => child.kill(), 90000);
  const status = await new Promise((accept) => child.on("exit", accept)); clearTimeout(timer);
  assert.equal(status, 0, `Native fixture failed: ${stdout}\n${stderr}`);
  const result = JSON.parse(readFileSync(join(root, "result.json"), "utf8"));
  assert.equal(result.renderer.video.seeked, true); assert.equal(result.renderer.audio.seeked, true);
  if (!result.renderer.pdf.viewerError) {
    assert.equal(result.renderer.pdf.pages, 3); assert.equal(result.renderer.pdf.parsedText, "PHASE4 Searchable alpha");
    assert.equal(result.renderer.pdf.parsedSearchMatches, 3); assert.equal(result.renderer.pdf.searchMatches, 3);
  }
  console.log(JSON.stringify({ ...result, originalVideoBytes, boundary: "Isolated hidden Electron, generated PDF/MP4/WAV, app-owned range streaming and native disk staging; not Tjommie integration, Save As dialog or Android proof" }, null, 2));
  // shortcut: a hidden renderer can block display scheduling; keep that acceptance gate red until a visible disposable app proves it.
  if (result.renderer.pdf.viewerError) process.exitCode = 2;
} finally {
  assert.equal(dirname(resolve(root)), resolve(tmpdir())); assert(root.startsWith(join(resolve(tmpdir()), "tjommie-files-native-")));
  rmSync(root, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
}
