import { createHash } from "node:crypto";

export const PDF_WORKER_MODIFICATION = "Tjommie modifies PDF.js 6.5.438 range grouping to request one 256 KiB chunk at a time; upstream document-length worker allocation remains.";
const ORIGINAL_SHA256 = "43710009ea26bfbcb615524946e46e3aaf8ad927514f5e465e8ff6d18044ffe9";

/** Preserve the pinned reader contract without assembling a large merged range in the renderer. */
export function boundedPdfWorker(source: string): string {
  if (createHash("sha256").update(source).digest("hex") !== ORIGINAL_SHA256) throw new Error("The pinned PDF.js worker changed; review its range transport before preparing assets.");
  const start = source.indexOf("  groupChunks(chunks) {"), end = source.indexOf("  onReceiveData(args) {", start);
  if (start < 0 || end < start || source.indexOf("  groupChunks(chunks) {", start + 1) !== -1) throw new Error("The pinned PDF.js range grouping could not be identified.");
  return source.slice(0, start) + `  // Modified by Tjommie: bounded range grouping; see third-party-notices.txt.\n  groupChunks(chunks) {\n    return chunks.map(chunk => ({ beginChunk: chunk, endChunk: chunk + 1 }));\n  }\n` + source.slice(end);
}
