import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { createRequire } from "node:module";
import { expect, it } from "vitest";
import { prepareUiAssets } from "./prepare-ui.ts";

it("prepares local PDF module, worker, resources and notices alongside secure transport", () => {
  const repo = mkdtempSync(join(tmpdir(), "tjommie-ui-assets-"));
  const write = (path: string, value: string) => { mkdirSync(dirname(path), { recursive: true }); writeFileSync(path, value); };
  try {
    write(join(repo, "packages/transport/wasm/tjommie_noise_bg.wasm"), "noise");
    write(join(repo, "packages/ui/package.json"), "{}");
    const pdf = join(repo, "node_modules/pdfjs-dist");
    write(join(pdf, "package.json"), '{"name":"pdfjs-dist"}');
    const files = ["build/pdf.mjs", "build/pdf.worker.mjs", "web/pdf_viewer.css", "web/images/image.svg", "cmaps/map.bcmap", "standard_fonts/font.pfb", "wasm/decode.wasm", "LICENSE"];
    for (const file of files) write(join(pdf, file), file === "build/pdf.worker.mjs" ? readFileSync(join(dirname(createRequire(import.meta.url).resolve("pdfjs-dist/package.json")), file), "utf8") : file);
    prepareUiAssets(repo);
    const output = join(repo, "packages/ui/public");
    expect(readFileSync(join(output, "tjommie_noise_bg.wasm"), "utf8")).toBe("noise");
    for (const file of files.filter((file) => file !== "build/pdf.worker.mjs")) expect(readFileSync(join(output, "pdfjs", file), "utf8")).toBe(file);
    expect(readFileSync(join(output, "pdfjs/build/pdf.worker.mjs"), "utf8")).toContain("Modified by Tjommie");
    expect(readFileSync(join(output, "third-party-notices.txt"), "utf8")).toContain("Mozilla Foundation and contributors");
    expect(readFileSync(join(output, "third-party-notices.txt"), "utf8")).toContain("Tjommie modifies PDF.js");
  } finally { rmSync(repo, { recursive: true, force: true }); }
});
