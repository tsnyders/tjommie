// @vitest-environment happy-dom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import type { Client } from "@tjommie/client";
import type { AttachmentInfo, AttachmentNativeApi } from "../../../protocol/src/attachments";
import { initialState } from "@/lib/state";
import { update } from "@/lib/store";
import { FileCard } from "./file-card";
const fixture = vi.hoisted(() => ({ native: null as AttachmentNativeApi | null, client: null as Pick<Client, "call"> | null, abort: null as AbortController | null, close: vi.fn() }));
vi.mock("@/lib/attachment-device", () => ({ attachmentDevice: () => fixture.native }));
vi.mock("@/lib/native-capability", () => ({ useLocalFileActions: () => null, localFileActions: () => null }));
vi.mock("./file-viewer", () => ({ previewKind: () => "markdown", FileViewer: () => null }));
vi.mock("@/lib/connection", () => ({ captureAttachmentDownload: (abort: AbortController) => { fixture.abort = abort; return { client: fixture.client!, reacquire: async () => fixture.client!, transferring: () => {}, close: fixture.close }; } }));
Reflect.set(globalThis, "IS_REACT_ACT_ENVIRONMENT", true);
const info: AttachmentInfo = { id: "83c70656-e163-478e-a934-6a5801f5ef18", hostId: "distinct-storage-host", name: "file.md", size: 0, contentType: "text/markdown", sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" };
let root: Root | null = null, choose: (value: { saveId: string }) => void = () => {};
const settle = () => act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });
beforeEach(() => {
  fixture.close.mockClear(); fixture.abort = null;
  fixture.client = { call: vi.fn(async (method: string) => method === "attachments.metadata" ? info : method === "attachments.open" ? { transferId: "41f8bac7-358c-48cf-9f48-a3a358804da6", attachment: info, chunkBytes: 262144, expiresAt: "2026-11-03T12:00:00Z" } : {}) } as unknown as Pick<Client, "call">;
  fixture.native = { startSave: vi.fn(() => new Promise<{ saveId: string }>((resolve) => { choose = resolve; })), activateSave: vi.fn(async () => {}), appendSave: vi.fn(async () => {}), finishSave: vi.fn(async () => ({ status: "saved" as const })), cancelSave: vi.fn(async () => {}), preview: async () => { throw new Error("unused"); }, onRead: () => () => {}, respond: async () => {}, closePreview: async () => {}, invalidate: async () => {} };
  update(() => ({ ...initialState, host: "selected-pc", connection: { status: "ready" } }));
});
afterEach(async () => { if (root) await act(async () => root!.unmount()); root = null; document.body.replaceChildren(); update(() => initialState); });
async function start() {
  root = createRoot(document.body.appendChild(document.createElement("div")));
  await act(async () => root!.render(<FileCard file={{ path: "unused", name: info.name, attachmentId: info.id, contentType: info.contentType }} messageId="message" />));
  await act(async () => document.querySelector<HTMLButtonElement>('[aria-label="Download file.md"]')!.click()); await settle();
}
it("keeps the byte-free picker through an offline render and saves after reconnect", async () => {
  await start(); await act(async () => update((state) => ({ ...state, connection: { status: "reconnecting" } }))); await settle();
  expect(fixture.abort?.signal.aborted).toBe(false); expect(fixture.native!.activateSave).not.toHaveBeenCalled();
  await act(async () => update((state) => ({ ...state, connection: { status: "ready" } }))); await act(async () => choose({ saveId: "chosen" })); await settle();
  expect(document.body.textContent).toContain("Saved on this device."); expect(fixture.native!.activateSave).toHaveBeenCalledWith("chosen"); expect(fixture.close).toHaveBeenCalledOnce();
});
it.each(["switch", "unmount"] as const)("cancels a pending picker on %s and cancels its late URI handle", async (reason) => {
  await start();
  if (reason === "switch") await act(async () => update((state) => ({ ...state, host: "another-pc" })));
  else { await act(async () => root!.unmount()); root = null; }
  await settle(); expect(fixture.abort?.signal.aborted).toBe(true);
  await act(async () => choose({ saveId: "late" })); await settle();
  expect(fixture.native!.activateSave).not.toHaveBeenCalled(); expect(fixture.native!.finishSave).not.toHaveBeenCalled(); expect(fixture.native!.cancelSave).toHaveBeenCalledWith("late");
});
it("cancels a genuine disconnect after transfer activation and ignores a late finish", async () => {
  let finish: (value: { status: "saved" }) => void = () => {};
  fixture.native!.finishSave = vi.fn(() => new Promise<{ status: "saved" }>((resolve) => { finish = resolve; }));
  await start(); await act(async () => choose({ saveId: "active" })); await settle(); expect(fixture.native!.activateSave).toHaveBeenCalled();
  await act(async () => update((state) => ({ ...state, connection: { status: "reconnecting" } }))); await settle();
  expect(fixture.abort?.signal.aborted).toBe(true); expect(fixture.native!.cancelSave).toHaveBeenCalledWith("active");
  await act(async () => finish({ status: "saved" })); await settle(); expect(document.body.textContent).toContain("Download cancelled."); expect(document.body.textContent).not.toContain("Saved on this device.");
});
it("shows provider cleanup failure even when a genuine disconnect also cancels the operation", async () => {
  fixture.native!.finishSave = vi.fn(() => new Promise<{ status: "saved" }>(() => {}));
  fixture.native!.cancelSave = vi.fn(async () => { throw Object.assign(new Error("Synthetic private provider error"), { code: "cleanup_failed" }); });
  await start(); await act(async () => choose({ saveId: "active" })); await settle();
  await act(async () => update((state) => ({ ...state, connection: { status: "reconnecting" } }))); await settle();
  expect(document.body.textContent).toContain("The save provider kept an incomplete file."); expect(document.body.textContent).not.toContain("Download cancelled."); expect(document.body.textContent).not.toContain("Synthetic private provider error");
});
