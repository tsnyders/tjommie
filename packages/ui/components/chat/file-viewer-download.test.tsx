// @vitest-environment happy-dom
import { act, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { expect, it, vi } from "vitest";
import type { AttachmentNativeApi } from "../../../protocol/src/attachments";
import { FileViewer } from "./file-viewer";
import { queueThumbnail } from "@/lib/file-thumbnails";

const preview = vi.hoisted(() => vi.fn());
vi.mock("@/lib/connection", () => ({ daemon: () => ({}) }));
vi.mock("@/lib/use-phone", () => ({ usePhone: () => false }));
vi.mock("../dialog", () => ({ Dialog: ({ children, actions }: { children: ReactNode; actions: ReactNode }) => <div>{actions}{children}</div> }));
vi.mock("@/lib/attachment-transfer", async (original) => ({ ...await original<typeof import("@/lib/attachment-transfer")>(), previewAttachment: preview, attachmentError: () => "Unavailable" }));
Reflect.set(globalThis, "IS_REACT_ACT_ENVIRONMENT", true);
const settle = () => act(async () => { await new Promise(resolve => setTimeout(resolve, 0)); });
it("closes companion then document before starting a new download under the two-transfer budget", async () => {
  const root = createRoot(document.body.appendChild(document.createElement("div"))), download = vi.fn(), closed = vi.fn();
  const text = new TextEncoder().encode("![Chart](chart.png)");
  vi.stubGlobal("fetch", vi.fn(async () => new Response(text, { status: 206, headers: { "content-range": `bytes 0-${text.length - 1}/${text.length}`, "content-length": String(text.length) } })));
  let imageDone!: () => void, documentDone!: () => void;
  const imageRetired = new Promise<void>(resolve => { imageDone = resolve; }), documentRetired = new Promise<void>(resolve => { documentDone = resolve; });
  const closeImage = vi.fn(() => imageRetired), closeDocument = vi.fn(() => documentRetired);
  preview.mockResolvedValueOnce({ attachment: { contentType: "text/markdown", size: text.length }, url: "app://tjommie/_attachments/main", close: closeDocument }).mockResolvedValueOnce({ attachment: { contentType: "image/png" }, url: "app://tjommie/_attachments/image", close: closeImage });
  const button = (label: string) => [...document.querySelectorAll("button")].find(item => item.textContent === label)!;
  try {
    await act(async () => root.render(<FileViewer name="document.md" reference={{ messageId: "message", attachmentIndex: 0 }} companions={[{ name: "chart.png", attachmentId: "41f8bac7-358c-48cf-9f48-a3a358804da6", attachmentIndex: 1 }]} native={{} as AttachmentNativeApi} onClose={closed} onDownload={download} />)); await settle();
    await act(async () => button("Show Chart").click()); await settle();
    await act(async () => button("Download").click()); await settle();
    expect(closeImage).toHaveBeenCalledOnce(); expect(closeDocument).not.toHaveBeenCalled(); expect(download).not.toHaveBeenCalled();
    await act(async () => imageDone()); await settle(); expect(closeDocument).toHaveBeenCalledOnce(); expect(download).not.toHaveBeenCalled();
    await act(async () => documentDone()); await settle(); expect(closed).toHaveBeenCalledOnce(); expect(download).toHaveBeenCalledOnce();
  } finally { imageDone?.(); documentDone?.(); await act(async () => root.unmount()); document.body.replaceChildren(); vi.unstubAllGlobals(); preview.mockReset(); }
});
it("retires a background thumbnail before opening the explicit viewer", async () => {
  let started!: () => void, close!: () => void;
  const began = new Promise<void>((resolve) => { started = resolve; }), retired = new Promise<void>((resolve) => { close = resolve; });
  const background = queueThumbnail(new AbortController().signal, async (signal) => { started(); await new Promise<void>((resolve) => signal.addEventListener("abort", () => resolve(), { once: true })); await retired; }); await began;
  const root = createRoot(document.body.appendChild(document.createElement("div")));
  preview.mockResolvedValue({ attachment: { contentType: "application/octet-stream", size: 0 }, url: "app://tjommie/_attachments/fixture", close: async () => {} });
  try {
    await act(async () => root.render(<FileViewer name="file.bin" reference={{ messageId: "message", attachmentIndex: 0 }} native={{} as AttachmentNativeApi} onClose={() => {}} onDownload={() => {}} />)); await settle(); expect(preview).not.toHaveBeenCalled();
    await act(async () => close()); await background; await settle(); expect(preview).toHaveBeenCalledOnce();
  } finally { close(); await act(async () => root.unmount()); document.body.replaceChildren(); preview.mockReset(); }
});
