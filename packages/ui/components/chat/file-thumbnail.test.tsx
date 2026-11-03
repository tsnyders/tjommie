// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { AttachmentNativeApi, AttachmentRef } from "../../../protocol/src/attachments";
import type { FileThumbnailContent } from "@/lib/file-thumbnails";
import { pauseThumbnailWork } from "@/lib/file-thumbnails";
import { FileThumbnail } from "./file-thumbnail";

const fixture = vi.hoisted(() => ({ read: vi.fn(), current: true, capture: vi.fn() }));
vi.mock("@/lib/connection", () => ({ captureAttachmentSave: () => { fixture.capture(); return { activate: () => () => fixture.current }; }, daemon: () => ({}) }));
vi.mock("@/lib/file-thumbnails", async (original) => ({ ...await original<typeof import("@/lib/file-thumbnails")>(), readFileThumbnail: fixture.read }));
Reflect.set(globalThis, "IS_REACT_ACT_ENVIRONMENT", true);
const native = {} as AttachmentNativeApi, reference = { messageId: "message", attachmentIndex: 0 };
let root: Root | null = null, visibility = "visible", intersect: (visible: boolean) => void;
const settle = () => act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
async function render(options: { reference?: AttachmentRef; type?: string; size?: number; active?: boolean; host?: string } = {}) {
  root ??= createRoot(document.body.appendChild(document.createElement("div")));
  await act(async () => root!.render(<FileThumbnail reference={options.reference ?? reference} native={native} type={options.type ?? "text/markdown"} size={options.size ?? 100} active={options.active ?? true} host={options.host ?? "selected-pc"} />));
}
beforeEach(() => {
  fixture.current = true; fixture.read.mockReset(); fixture.capture.mockClear(); visibility = "visible";
  vi.spyOn(document, "visibilityState", "get").mockImplementation(() => visibility as DocumentVisibilityState);
  vi.stubGlobal("IntersectionObserver", class {
    constructor(callback: (entries: { isIntersecting: boolean }[]) => void) { intersect = (visible) => callback([{ isIntersecting: visible }]); }
    observe() {} disconnect() {}
  });
});
afterEach(async () => { if (root) await act(async () => root!.unmount()); root = null; document.body.replaceChildren(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });

it("loads only intersecting visible cards and removes actual content immediately when hidden", async () => {
  fixture.read.mockResolvedValue({ text: "# Actual attached report\n<img src='https://invalid.example'>" });
  await render(); expect(fixture.read).not.toHaveBeenCalled();
  await act(async () => intersect(true)); await settle(); expect(document.body.textContent).toContain("Actual attached report"); expect(document.querySelector("img")).toBeNull();
  await act(async () => { visibility = "hidden"; document.dispatchEvent(new Event("visibilitychange")); });
  expect(document.body.textContent).not.toContain("Actual attached report"); expect(document.querySelector("canvas")).toBeNull();
  await act(async () => { visibility = "visible"; document.dispatchEvent(new Event("visibilitychange")); }); await settle(); expect(fixture.read).toHaveBeenCalledTimes(2);
});
it.each(["host", "reference", "disconnect", "unmount"] as const)("aborts stale %s work and never paints its late result", async (change) => {
  let finish!: (value: FileThumbnailContent) => void;
  fixture.read.mockImplementationOnce((_client, _ref, _native, signal: AbortSignal) => new Promise<FileThumbnailContent>((resolve) => { finish = resolve; expect(signal.aborted).toBe(false); })).mockResolvedValue({ text: "Fresh document" });
  await render(); await act(async () => intersect(true)); await settle(); const signal = fixture.read.mock.calls[0][3] as AbortSignal;
  if (change === "host") await render({ host: "other-pc" });
  if (change === "reference") await render({ reference: { messageId: "different", attachmentIndex: 1 } });
  if (change === "disconnect") { fixture.current = false; await render({ active: false }); }
  if (change === "unmount") { await act(async () => root!.unmount()); root = null; }
  expect(signal.aborted).toBe(true); await act(async () => finish({ text: "Stale private document" })); await settle();
  expect(document.body.textContent).not.toContain("Stale private document");
  if (change === "host" || change === "reference") expect(document.body.textContent).toContain("Fresh document");
});
it("never opens large PDF/video or audio handles just to decorate a card", async () => {
  await render({ type: "application/pdf", size: 200 * 1024 * 1024 }); await act(async () => intersect(true)); await settle();
  expect(document.body.textContent).toContain("PDF preview"); expect(fixture.capture).not.toHaveBeenCalled();
  await render({ type: "video/mp4", size: 200 * 1024 * 1024 }); await settle(); expect(fixture.read).not.toHaveBeenCalled();
  await render({ type: "audio/wav", size: 1024 }); await settle(); expect(document.body.textContent).toContain("Open audio player"); expect(document.querySelector("audio")).toBeNull(); expect(fixture.read).not.toHaveBeenCalled();
});
it("retires background content for an explicit viewer/save and resumes only after release", async () => {
  let finish!: (value: FileThumbnailContent) => void;
  fixture.read.mockImplementationOnce(() => new Promise<FileThumbnailContent>((resolve) => { finish = resolve; })).mockResolvedValue({ text: "Resumed excerpt" });
  await render(); await act(async () => intersect(true)); await settle();
  let pause!: ReturnType<typeof pauseThumbnailWork>;
  await act(async () => { pause = pauseThumbnailWork(); });
  expect((fixture.read.mock.calls[0][3] as AbortSignal).aborted).toBe(true); expect(document.querySelector("canvas")).toBeNull();
  await act(async () => finish({ text: "Retired excerpt" })); await pause.ready; await settle();
  expect(document.body.textContent).not.toContain("Retired excerpt"); expect(fixture.read).toHaveBeenCalledOnce();
  await act(async () => pause.release()); await settle(); expect(document.body.textContent).toContain("Resumed excerpt");
});
