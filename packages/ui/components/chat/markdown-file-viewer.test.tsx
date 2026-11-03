// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, expect, it, vi } from "vitest";
import type { AttachmentNativeApi } from "../../../protocol/src/attachments";
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
const bytes = new TextEncoder().encode(text);
const settle = () => act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
async function mount(): Promise<void> {
  vi.stubGlobal("fetch", vi.fn(async () => new Response(bytes, { status: 206, headers: { "content-range": `bytes 0-${bytes.length - 1}/${bytes.length}`, "content-length": String(bytes.length) } })));
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
  await act(async () => button("Show Chart").click()); await settle();
  expect(preview).toHaveBeenCalledWith({}, { messageId: "parent", attachmentIndex: 2, attachmentId: companions[0].attachmentId }, native, expect.any(AbortSignal), expect.any(Function));
  expect(document.querySelector("img")?.src).toContain("/_attachments/"); expect(document.querySelector('img[src^="https:"]')).toBeNull();
  await act(async () => button("Hide Chart").click()); await settle(); expect(close).toHaveBeenCalledOnce(); expect(document.querySelector("img")).toBeNull();
});
it("discards a companion result arriving after hide, and waits for cleanup before granting another handle", async () => {
  let complete!: (value: { attachment: { contentType: string }; url: string; close: () => Promise<void> }) => void;
  const close = vi.fn(async () => {}); preview.mockImplementationOnce(() => new Promise(resolve => { complete = resolve; }));
  await mount(); await act(async () => button("Show Chart").click()); await settle(); await act(async () => button("Hide Chart").click());
  await act(async () => complete({ attachment: { contentType: "image/png" }, url: "app://tjommie/_attachments/late", close })); await settle();
  expect(close).toHaveBeenCalledOnce(); expect(document.querySelector("img")).toBeNull();
});
