import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { runInNewContext } from "node:vm";
import { expect, it } from "vitest";
import { boundedPdfWorker } from "./pdfjs-worker.ts";

const directory = dirname(createRequire(import.meta.url).resolve("pdfjs-dist/package.json"));
it("adapts the exact pinned worker so even a whole 200MiB request uses individual chunk callbacks", () => {
  const source = readFileSync(join(directory, "build/pdf.worker.mjs"), "utf8"), patched = boundedPdfWorker(source);
  const method = patched.slice(patched.indexOf("  groupChunks(chunks) {"), patched.indexOf("  onReceiveData(args) {", patched.indexOf("  groupChunks(chunks) {")));
  const grouping = runInNewContext(`({${method}})`) as { groupChunks(chunks: number[]): { beginChunk: number; endChunk: number }[] };
  const groups = grouping.groupChunks(Array.from({ length: 800 }, (_, index) => index));
  expect(groups).toHaveLength(800); expect(groups.every((group, index) => group.beginChunk === index && group.endChunk === index + 1)).toBe(true);
  expect(patched).toContain("super(new Uint8Array(length)"); expect(patched).toContain("Modified by Tjommie");
  expect(() => boundedPdfWorker(source.replace("6.5.438", "6.5.439"))).toThrow("pinned");
  expect(() => boundedPdfWorker(patched)).toThrow("pinned");
});
