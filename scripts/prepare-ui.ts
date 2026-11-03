import { cpSync, existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { boundedPdfWorker, PDF_WORKER_MODIFICATION } from "./pdfjs-worker.ts";

export function prepareUiAssets(repo = resolve(fileURLToPath(new URL("..", import.meta.url)))): void {
  const wasm = join(repo, "packages", "transport", "wasm", "tjommie_noise_bg.wasm");
  if (!existsSync(wasm)) throw new Error("Build the local secure transport first: npm run build -w @tjommie/transport");
  const target = join(repo, "packages", "ui", "public");
  mkdirSync(target, { recursive: true });
  cpSync(wasm, join(target, "tjommie_noise_bg.wasm"));
  const pdf = dirname(createRequire(join(repo, "packages", "ui", "package.json")).resolve("pdfjs-dist/package.json"));
  for (const entry of ["build/pdf.mjs", "build/pdf.worker.mjs", "web/pdf_viewer.css", "web/images", "cmaps", "standard_fonts", "wasm", "LICENSE"]) {
    const destination = join(target, "pdfjs", entry);
    mkdirSync(dirname(destination), { recursive: true });
    cpSync(join(pdf, entry), destination, { recursive: true });
  }
  writeFileSync(join(target, "pdfjs/build/pdf.worker.mjs"), boundedPdfWorker(readFileSync(join(pdf, "build/pdf.worker.mjs"), "utf8")), "utf8");
  writeFileSync(join(target, "third-party-notices.txt"), `PDF.js by Mozilla Foundation and contributors\n${PDF_WORKER_MODIFICATION}\n\n${readFileSync(join(pdf, "LICENSE"), "utf8")}`, "utf8");
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) prepareUiAssets();
