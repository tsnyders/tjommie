// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";
import type { AttachmentNativeApi } from "../../../protocol/src/attachments";
import { MARKDOWN_SECTION_BYTES } from "@/lib/markdown-document";
import { MarkdownFileViewer } from "./markdown-file-viewer";

const preview = vi.hoisted(() => vi.fn());
vi.mock("@/lib/connection", () => ({ daemon: () => ({}) }));
vi.mock("@/lib/attachment-transfer", () => ({ previewAttachment: preview, attachmentError: () => "Unavailable" }));
Reflect.set(globalThis, "IS_REACT_ACT_ENVIRONMENT", true);
let root: Root | undefined;
const native = {} as AttachmentNativeApi;
const companions = [{ name: "chart.png", attachmentId: "41f8bac7-358c-48cf-9f48-a3a358804da6", attachmentIndex: 2 }];
const reference = { messageId: "parent", attachmentIndex: 0 };
const text = "# Document\n\n![Chart](chart.png) ![remote](https://attacker.invalid/private.png) ![escape](../chart.png)";
const settle = () => act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
async function mount(markdown = text): Promise<void> {
  const bytes = new TextEncoder().encode(markdown);
  vi.stubGlobal("fetch", vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
    const range = /^bytes=(\d+)-(\d+)$/.exec(new Headers(init?.headers).get("Range") ?? "");
    if (!range) throw new Error("Expected a bounded document read");
    const begin = Number(range[1]), end = Number(range[2]);
    return new Response(bytes.slice(begin, end + 1), { status: 206, headers: { "content-range": `bytes ${begin}-${end}/${bytes.length}`, "content-length": String(end - begin + 1) } });
  }));
  root = createRoot(document.body.appendChild(document.createElement("div")));
  await act(async () => root!.render(<MarkdownFileViewer url="app://tjommie/_attachments/main" size={bytes.length} reference={reference} companions={companions} native={native} />)); await settle();
}
function button(label: string): HTMLButtonElement { const found = [...document.querySelectorAll("button")].find(item => item.textContent === label); if (!found) throw new Error(`Missing ${label}`); return found; }
afterEach(async () => { if (root) await act(async () => root!.unmount()); root = undefined; document.body.replaceChildren(); preview.mockReset(); vi.unstubAllGlobals(); });
it("opens only an explicitly associated image using its parent message/index and closes it when hidden", async () => {
  const close = vi.fn(async () => {}); preview.mockResolvedValue({ attachment: { contentType: "image/png" }, url: "app://tjommie/_attachments/41f8bac7-358c-48cf-9f48-a3a358804da6", close });
  await mount(); expect(document.querySelector("img")).toBeNull(); expect(document.body.textContent).toContain("remote");
  const region = document.querySelector<HTMLElement>('[role="region"][aria-label="Markdown document; scroll to read this section"]');
  expect(region?.tabIndex).toBe(0); region?.focus(); expect(document.activeElement).toBe(region);
  const search = document.querySelector<HTMLInputElement>('[aria-label="Search Markdown"]'), section = document.querySelector<HTMLInputElement>('[aria-label="Markdown section"]');
  expect(search?.labels?.[0].textContent).toBe("Search Markdown"); expect(section).toBeNull();
  expect(document.querySelector('[aria-label="Next section"]')).toBeNull();
  await act(async () => button("Show Chart").click()); await settle();
  expect(preview).toHaveBeenCalledWith({}, { messageId: "parent", attachmentIndex: 2, attachmentId: companions[0].attachmentId }, native, expect.any(AbortSignal), expect.any(Function));
  expect(document.querySelector("img")?.src).toContain("/_attachments/"); expect(document.querySelector('img[src^="https:"]')).toBeNull();
  await act(async () => button("Hide Chart").click()); await settle(); expect(close).toHaveBeenCalledOnce(); expect(document.querySelector("img")).toBeNull();
});
it("keeps section navigation and full-file search for a large document", async () => {
  const first = "# First section\n\n";
  await mount(first + " ".repeat(MARKDOWN_SECTION_BYTES - first.length) + "# Last section\n\nNeedle");
  const section = document.querySelector<HTMLInputElement>('[aria-label="Markdown section"]')!;
  expect(section.labels?.[0].textContent).toBe("Section"); expect(section.value).toBe("1");
  await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Next section"]')!.click()); await settle();
  expect(section.value).toBe("2"); expect(document.querySelector("h1")?.textContent).toBe("Last section");
  await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Previous section"]')!.click()); await settle();
  expect(section.value).toBe("1");
  const search = document.querySelector<HTMLInputElement>('[aria-label="Search Markdown"]')!;
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!.call(search, "Needle");
    search.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await act(async () => search.form!.dispatchEvent(new Event("submit", { bubbles: true, cancelable: true }))); await settle();
  expect(section.value).toBe("2"); expect(document.querySelector('[role="status"]')?.textContent).toBe("Match in section 2");
});
it("discards a companion result arriving after hide, and waits for cleanup before granting another handle", async () => {
  let complete!: (value: { attachment: { contentType: string }; url: string; close: () => Promise<void> }) => void;
  const close = vi.fn(async () => {}); preview.mockImplementationOnce(() => new Promise(resolve => { complete = resolve; }));
  await mount(); await act(async () => button("Show Chart").click()); await settle(); await act(async () => button("Hide Chart").click());
  await act(async () => complete({ attachment: { contentType: "image/png" }, url: "app://tjommie/_attachments/late", close })); await settle();
  expect(close).toHaveBeenCalledOnce(); expect(document.querySelector("img")).toBeNull();
});
