import { MAX_ATTACHMENT_BYTES } from "../../protocol/src/attachments";
import { documentRange } from "./document-ranges";

export const MARKDOWN_SECTION_BYTES = 32 * 1024;
export type MarkdownSection = { text: string; begin: number; end: number };
export async function markdownSection(url: string, size: number, section: number, signal: AbortSignal): Promise<MarkdownSection> {
  signal.throwIfAborted();
  if (!Number.isSafeInteger(size) || size < 0 || size > MAX_ATTACHMENT_BYTES || !Number.isSafeInteger(section) || section < 0 || section >= Math.max(1, Math.ceil(size / MARKDOWN_SECTION_BYTES))) throw new RangeError("Invalid Markdown section.");
  if (!size) return { text: "", begin: 0, end: 0 };
  const start = section * MARKDOWN_SECTION_BYTES, end = Math.min(size, start + MARKDOWN_SECTION_BYTES), first = Math.max(0, start - 3);
  const bytes = await documentRange(url, size, first, end, signal);
  let beginAt = start - first, endAt = bytes.length;
  while (beginAt > 0 && (bytes[beginAt] & 0xc0) === 0x80) beginAt--;
  if (end < size) {
    // A final incomplete UTF-8 character belongs to the next section.
    let last = endAt - 1;
    while (last > 0 && (bytes[last] & 0xc0) === 0x80) last--;
    const lead = bytes[last], width = lead < 0x80 ? 1 : lead < 0xe0 ? 2 : lead < 0xf0 ? 3 : 4;
    if (endAt - last < width) endAt = last;
  }
  return { text: new TextDecoder("utf-8", { fatal: true }).decode(bytes.subarray(beginAt, endAt)), begin: first + beginAt, end: first + endAt };
}

export async function findMarkdownText(url: string, size: number, query: string, from: number, signal: AbortSignal): Promise<{ offset: number; section: number } | null> {
  if (!query || query.length > 256 || !Number.isSafeInteger(from) || from < 0 || from > size) throw new RangeError("Invalid Markdown search.");
  let carry = "", carryBytes = 0;
  for (let section = Math.floor(from / MARKDOWN_SECTION_BYTES); section < Math.ceil(size / MARKDOWN_SECTION_BYTES); section++) {
    signal.throwIfAborted();
    const current = await markdownSection(url, size, section, signal), combined = carry + current.text;
    let index = combined.toLocaleLowerCase().indexOf(query.toLocaleLowerCase());
    while (index !== -1) {
      const offset = current.begin - carryBytes + new TextEncoder().encode(combined.slice(0, index)).length;
      if (offset >= from) return { offset, section: Math.floor(offset / MARKDOWN_SECTION_BYTES) };
      index = combined.toLocaleLowerCase().indexOf(query.toLocaleLowerCase(), index + 1);
    }
    carry = query.length > 1 ? combined.slice(-(query.length - 1)) : ""; carryBytes = new TextEncoder().encode(carry).length;
  }
  return null;
}
