import type { Client } from "@tjommie/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import { ATTACHMENT_CHUNK_BYTES, type AttachmentNativeApi, type AttachmentNativeRead, type AttachmentOpen } from "../../protocol/src/attachments";
import { attachmentBytes, AttachmentTransfer, downloadAttachment, previewAttachment } from "./attachment-transfer";

const ref = { messageId: "message", attachmentIndex: 0 };
const transfer: AttachmentOpen = { transferId: "41f8bac7-358c-48cf-9f48-a3a358804da6", attachment: { id: "83c70656-e163-478e-a934-6a5801f5ef18", hostId: "host", name: "file.bin", size: ATTACHMENT_CHUNK_BYTES + 7, contentType: "application/octet-stream", sha256: "a".repeat(64) }, chunkBytes: ATTACHMENT_CHUNK_BYTES, expiresAt: "2026-11-03T12:00:00Z" };
afterEach(() => vi.useRealTimers());
function fixture() {
  const call = vi.fn(async (method: string, params: { offset?: number; length?: number }) => {
    if (method === "attachments.metadata") return transfer.attachment;
    if (method === "attachments.open") return transfer;
    if (method === "attachments.close") return {};
    const offset = params.offset!, size = Math.min(params.length!, transfer.attachment.size - offset);
    return { transferId: transfer.transferId, revision: transfer.attachment.sha256, offset, bytes: Buffer.alloc(size, 97).toString("base64"), eof: offset + size === transfer.attachment.size };
  });
  const native: AttachmentNativeApi = { startSave: vi.fn(async () => ({ saveId: "native-save" })), appendSave: vi.fn(async () => {}), finishSave: vi.fn(async () => ({ status: "saved" as const })), cancelSave: vi.fn(async () => {}), preview: vi.fn(async () => ({ previewId: "preview", url: "app://tjommie/_attachments/preview" })), onRead: vi.fn(() => () => {}), respond: vi.fn(async () => {}), closePreview: vi.fn(async () => {}), invalidate: vi.fn(async () => {}) };
  return { client: { call } as unknown as Pick<Client, "call">, call, native };
}
describe("bounded attachment client", () => {
  it.each(["reauth", "revision", "activation"] as const)("reports failed URI cleanup after %s fails and still closes its transfer", async (phase) => {
    const { client, native, call } = fixture(), failure = Object.assign(new Error("Synthetic provider retained document"), { code: "cleanup_failed" });
    native.cancelSave = vi.fn(async () => { throw failure; });
    native.activateSave = vi.fn(async () => { if (phase === "activation") throw new Error("Activation refused"); });
    if (phase === "revision") call.mockImplementationOnce(async () => transfer.attachment).mockImplementationOnce(async () => ({ ...transfer, attachment: { ...transfer.attachment, sha256: "b".repeat(64) } }));
    const options = phase === "reauth" ? { reacquire: async (): Promise<typeof client> => { throw new Error("Authorization refused"); } } : {};
    await expect(downloadAttachment(client, ref, native, new AbortController().signal, () => {}, options)).rejects.toMatchObject({ code: "cleanup_failed" });
    expect(native.appendSave).not.toHaveBeenCalled(); if (phase !== "reauth") expect(call).toHaveBeenLastCalledWith("attachments.close", { transferId: transfer.transferId });
  });
  it("holds no transfer while a ten-minute picker waits, then opens on the reacquired client", async () => {
    vi.useFakeTimers(); const first = fixture(), fresh = fixture();
    let choose: (value: { saveId: string }) => void = () => {};
    first.native.startSave = vi.fn(() => new Promise<{ saveId: string }>((resolve) => { choose = resolve; }));
    first.native.activateSave = vi.fn(async () => {});
    const phase = vi.fn(), reacquire = vi.fn(async () => fresh.client);
    const pending = downloadAttachment(first.client, ref, first.native, new AbortController().signal, () => {}, { reacquire, transferring: phase });
    await vi.advanceTimersByTimeAsync(600_000);
    expect(first.call.mock.calls.map(([method]) => method)).toEqual(["attachments.metadata"]);
    expect(first.native.appendSave).not.toHaveBeenCalled(); expect(first.native.activateSave).not.toHaveBeenCalled();
    choose({ saveId: "chosen" }); expect(await pending).toBe("saved");
    expect(reacquire).toHaveBeenCalledOnce(); expect(phase).toHaveBeenCalledWith(fresh.client);
    expect(fresh.call).toHaveBeenCalledWith("attachments.open", ref); expect(first.native.activateSave).toHaveBeenCalledWith("chosen");
  });
  it.each(["id", "hostId", "name", "size", "contentType", "sha256"] as const)("refuses changed %s after the picker before activating or reading", async (field) => {
    const first = fixture(), fresh = fixture(); first.native.activateSave = vi.fn(async () => {});
    const changed = { ...transfer.attachment, [field]: field === "size" ? 12 : field === "id" ? transfer.transferId : field === "sha256" ? "b".repeat(64) : "changed" };
    fresh.call.mockImplementationOnce(async () => ({ ...transfer, attachment: changed }));
    await expect(downloadAttachment(first.client, ref, first.native, new AbortController().signal, () => {}, { reacquire: async () => fresh.client })).rejects.toThrow(/changed/);
    expect(first.native.activateSave).not.toHaveBeenCalled(); expect(first.native.appendSave).not.toHaveBeenCalled();
    expect(fresh.call).toHaveBeenLastCalledWith("attachments.close", { transferId: transfer.transferId }); expect(first.native.cancelSave).toHaveBeenCalled();
  });
  it("enters transfer cancellation before open and closes a late open after genuine loss", async () => {
    const { client, native, call } = fixture(), abort = new AbortController(); let opened: (value: AttachmentOpen) => void = () => {};
    call.mockImplementation(async (method) => method === "attachments.metadata" ? transfer.attachment : method === "attachments.open" ? new Promise((resolve) => { opened = resolve; }) : {});
    const phases: string[] = [];
    const pending = downloadAttachment(client, ref, native, abort.signal, () => {}, { transferring: () => { phases.push("transferring"); } });
    await vi.waitFor(() => expect(call).toHaveBeenCalledWith("attachments.open", ref)); expect(phases).toEqual(["transferring"]);
    abort.abort(); await expect(pending).rejects.toMatchObject({ name: "AbortError" }); opened(transfer); await Promise.resolve(); await Promise.resolve();
    expect(native.appendSave).not.toHaveBeenCalled(); expect(native.cancelSave).toHaveBeenCalledWith("native-save"); expect(call).toHaveBeenCalledWith("attachments.close", { transferId: transfer.transferId });
  });
  it.each(["start", "finish"] as const)("lets a native %s picker wait beyond the host RPC deadline", async (phase) => {
    vi.useFakeTimers();
    const { client, native } = fixture();
    let release = () => {};
    const waiting = new Promise<void>((resolve) => { release = resolve; });
    if (phase === "start") native.startSave = async () => { await waiting; return { saveId: "chosen" }; };
    else native.finishSave = async () => { await waiting; return { status: "saved" }; };
    let settled = false;
    const result = downloadAttachment(client, ref, native, new AbortController().signal, () => {});
    void result.then(() => { settled = true; }, () => { settled = true; });
    await vi.advanceTimersByTimeAsync(40_000);
    expect(settled).toBe(false);
    release();
    expect(await result).toBe("saved");
  });
  it("streams downloads into the native destination without assembling the file", async () => {
    const { client, native, call } = fixture(), progress = vi.fn();
    expect(await downloadAttachment(client, ref, native, new AbortController().signal, progress)).toBe("saved");
    expect(native.appendSave).toHaveBeenCalledTimes(2); expect(progress).toHaveBeenLastCalledWith(transfer.attachment.size, transfer.attachment.size);
    expect(call.mock.calls.filter(([method]) => method === "attachments.read").map(([, params]) => params.length)).toEqual([ATTACHMENT_CHUNK_BYTES, ATTACHMENT_CHUNK_BYTES]);
    expect(call).toHaveBeenLastCalledWith("attachments.close", { transferId: transfer.transferId });
  });
  it("does not write bytes after a cancelled picker or report a cancelled finish as saved", async () => {
    const { client, native } = fixture(); native.startSave = vi.fn(async () => null);
    expect(await downloadAttachment(client, ref, native, new AbortController().signal, () => {})).toBe("cancelled"); expect(native.appendSave).not.toHaveBeenCalled();
    native.startSave = vi.fn(async () => ({ saveId: "save" })); native.finishSave = vi.fn(async () => ({ status: "cancelled" as const }));
    expect(await downloadAttachment(client, ref, native, new AbortController().signal, () => {})).toBe("cancelled");
  });
  it("rejects revision, count, offset, transfer and base64 corruption", () => {
    const chunk = { transferId: transfer.transferId, revision: transfer.attachment.sha256, offset: 0, bytes: "YQ==", eof: false };
    expect(attachmentBytes(chunk, transfer, 0, 1)).toEqual(Uint8Array.of(97));
    for (const patch of [{ revision: "b".repeat(64) }, { offset: 1 }, { transferId: transfer.attachment.id }, { bytes: "YQ==\n" }, { bytes: "Yg==", eof: true }, { bytes: "Zh==" }, { bytes: "" }]) expect(() => attachmentBytes({ ...chunk, ...patch }, transfer, 0, 1)).toThrow();
  });
  it("refuses a network preview URL and closes its lease/native handle", async () => {
    const { client, native } = fixture(); native.preview = vi.fn(async () => ({ previewId: "preview", url: "https://example.com/file" }));
    await expect(previewAttachment(client, ref, native, new AbortController().signal)).rejects.toThrow(/invalid preview/); expect(native.closePreview).toHaveBeenCalledWith("preview");
  });
  it("serves only the matching native preview and disposes its listener once", async () => {
    const { client, native } = fixture(), previewId = "624d43d7-1a49-4d79-8d1d-85d846cfbd8e", requestId = "8172071e-bd79-4d10-8b8a-a9db09353cba", remove = vi.fn(); let listener: (request: AttachmentNativeRead) => void = () => {};
    native.preview = vi.fn(async () => ({ previewId, url: `app://tjommie/_attachments/${previewId}` })); native.onRead = (next) => { listener = next; return remove; };
    const preview = await previewAttachment(client, ref, native, new AbortController().signal);
    listener({ previewId: "another", requestId, offset: 0, length: 3 }); expect(native.respond).not.toHaveBeenCalled();
    listener({ previewId, requestId, offset: 0, length: 3 }); await vi.waitFor(() => expect(native.respond).toHaveBeenCalledWith(previewId, requestId, "YWFh"));
    await preview.close(); await preview.close(); expect(remove).toHaveBeenCalledTimes(1); expect(native.closePreview).toHaveBeenCalledTimes(1);
  });
  it("reports a host read failure separately from an unsupported media codec", async () => {
    const { client, native, call } = fixture(), previewId = "624d43d7-1a49-4d79-8d1d-85d846cfbd8e", failure = Object.assign(new Error("Synthetic corrupt file"), { code: "corrupt" }), onError = vi.fn(); let listener: (request: AttachmentNativeRead) => void = () => {};
    native.preview = async () => ({ previewId, url: `app://tjommie/_attachments/${previewId}` }); native.onRead = (next) => { listener = next; return () => {}; };
    const preview = await previewAttachment(client, ref, native, new AbortController().signal, onError); call.mockRejectedValueOnce(failure);
    listener({ previewId, requestId: "request", offset: 0, length: 3 }); await vi.waitFor(() => expect(onError).toHaveBeenCalledWith(failure)); expect(native.respond).toHaveBeenCalledWith(previewId, "request", null); await preview.close();
  });
  it("closes an open reply that arrives after cancellation", async () => {
    const { client, call } = fixture(), abort = new AbortController(); let finish: (value: AttachmentOpen) => void = () => {};
    call.mockImplementationOnce(() => new Promise((accept) => { finish = accept as (value: AttachmentOpen) => void; }));
    const opening = AttachmentTransfer.open(client, ref, abort.signal); abort.abort(); await expect(opening).rejects.toMatchObject({ name: "AbortError" }); finish(transfer); await Promise.resolve(); await Promise.resolve();
    expect(call).toHaveBeenCalledWith("attachments.close", { transferId: transfer.transferId });
  });
  it("never finalizes a native save when cancellation wins just before its start reply resumes", async () => {
    const { client, native } = fixture(), abort = new AbortController();
    native.startSave = () => { const result = Promise.resolve({ saveId: "late-save" }); void result.then(() => queueMicrotask(() => abort.abort())); return result; };
    await expect(downloadAttachment(client, ref, native, abort.signal, () => {})).rejects.toMatchObject({ name: "AbortError" });
    expect(native.finishSave).not.toHaveBeenCalled(); expect(native.appendSave).not.toHaveBeenCalled(); expect(native.cancelSave).toHaveBeenCalledWith("late-save");
  });
});
