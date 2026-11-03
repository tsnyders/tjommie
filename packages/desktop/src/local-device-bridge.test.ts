import { createHash, randomUUID } from "node:crypto";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it, vi } from "vitest";
import type { AttachmentNativeRead } from "../../protocol/src/attachments.ts";
import type { SpeechStatus } from "../../protocol/src/speech.ts";
import { tempRoot } from "../../daemon/src/testing.ts";
import { DesktopDeviceBridge, attachmentSaveName } from "./local-device-bridge.ts";

function fixture(destination: string | null = null) {
  const frame = { url: "app://tjommie/chat/" }, send = vi.fn<(channel: string, value: unknown) => void>();
  let visible = true;
  const contents = { id: 17, mainFrame: frame, getURL: () => frame.url, isDestroyed: () => false, send };
  const window = { webContents: contents, isDestroyed: () => false, isVisible: () => visible, isFocused: () => true };
  const event = { sender: contents, senderFrame: frame };
  const status: SpeechStatus = { engine: "whisper.cpp", capture: "pcm", available: false, permission: "unknown", languages: [], model: { state: "missing", bytes: 1, received: 0 } };
  const speech = { status: vi.fn(async () => status), isCapturing: vi.fn(() => true), cancelAll: vi.fn(async () => {}), start: vi.fn(async (_input: unknown) => {}), stop: vi.fn(async (_input: unknown) => {}), cancel: vi.fn(async (_input: unknown) => {}), appendPcm: vi.fn(async (_input: unknown) => {}), downloadModel: vi.fn(async () => {}), cancelModelDownload: vi.fn(async () => {}), removeModel: vi.fn(async () => {}) };
  const chooseSave = vi.fn(async () => destination);
  const bridge = new DesktopDeviceBridge({ window: () => window, ready: () => true, production: true, speech, chooseSave });
  const bytes = Buffer.from("synthetic shared file");
  const info = { id: randomUUID(), hostId: "host", name: "shared.txt", size: bytes.length, contentType: "text/markdown", sha256: createHash("sha256").update(bytes).digest("hex") };
  return { bridge, event, window, contents, frame, send, speech, chooseSave, bytes, info, hide: () => { visible = false; } };
}

describe("trusted current-device bridge", () => {
  it("rejects other windows, subframes and foreign origins before touching native services", async () => {
    const test = fixture();
    for (const event of [{ ...test.event, sender: { ...test.contents } }, { ...test.event, senderFrame: { url: test.frame.url } }, { ...test.event, senderFrame: null }]) {
      await expect(test.bridge.startSave(event, test.info)).rejects.toThrow("Untrusted");
      expect(() => test.bridge.status(event)).toThrow("Untrusted");
    }
    test.frame.url = "https://attacker.invalid/";
    await expect(test.bridge.startSpeech(test.event, { sessionId: "bad", language: "en" })).rejects.toThrow("Untrusted");
    expect(test.chooseSave).not.toHaveBeenCalled(); expect(test.speech.status).not.toHaveBeenCalled(); expect(test.speech.start).not.toHaveBeenCalled();
  });
  it("binds a range response and its replies to the live main frame and generation", async () => {
    const test = fixture(), preview = await test.bridge.preview(test.event, test.info);
    const request = { webContentsId: 17, frame: test.frame, resourceType: "media" };
    expect(test.bridge.permitsAttachmentRequest(request)).toBe(true);
    for (const change of [{ webContentsId: 18 }, { frame: { url: test.frame.url } }, { frame: null }, { resourceType: "mainFrame" }, { resourceType: "subFrame" }, { resourceType: "script" }]) expect(test.bridge.permitsAttachmentRequest({ ...request, ...change })).toBe(false);
    const response = await test.bridge.serve(new Request(preview.url, { headers: { range: "bytes=1-3" } })), body = response.arrayBuffer();
    await vi.waitFor(() => expect(test.send).toHaveBeenCalledWith("attachments:read", expect.objectContaining({ previewId: preview.previewId, offset: 1, length: 3 })));
    const read = test.send.mock.lastCall![1] as AttachmentNativeRead;
    test.bridge.respond(test.event, preview.previewId, read.requestId, test.bytes.subarray(1, 4).toString("base64"));
    expect(Buffer.from(await body)).toEqual(test.bytes.subarray(1, 4)); expect(response.status).toBe(206);
    await test.bridge.invalidate(); expect(test.bridge.permitsAttachmentRequest(request)).toBe(false); expect((await test.bridge.serve(new Request(preview.url))).status).toBe(404);
    expect(() => test.bridge.respond(test.event, preview.previewId, read.requestId, "YWJj")).toThrow("closed");
  });
  it("cancels an outstanding native read inline before asynchronous cleanup finishes", async () => {
    const test = fixture(), preview = await test.bridge.preview(test.event, test.info);
    const response = await test.bridge.serve(new Request(preview.url)), body = response.arrayBuffer(), failed = expect(body).rejects.toThrow("closed");
    await vi.waitFor(() => expect(test.send).toHaveBeenCalled());
    let finish: (() => void) | undefined; test.speech.cancelAll.mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
    const cleanup = test.bridge.invalidate(); await failed;
    const next = test.bridge.preview(test.event, test.info); expect(test.bridge.permitsAttachmentRequest({ webContentsId: 17, frame: test.frame, resourceType: "media" })).toBe(false);
    finish?.(); await cleanup; await next; await test.bridge.invalidate();
  });
  it("saves verified bytes only to the system-selected destination and clears a partial save", async () => {
    const root = tempRoot(), destination = join(root, "chosen.txt"), test = fixture(destination);
    const save = await test.bridge.startSave(test.event, test.info); expect(save).not.toBeNull();
    await test.bridge.appendSave(test.event, save!.saveId, 0, test.bytes.toString("base64"));
    expect(await test.bridge.finishSave(test.event, save!.saveId)).toEqual({ status: "saved" }); expect(readFileSync(destination)).toEqual(test.bytes);
    await test.bridge.startSave(test.event, test.info); await test.bridge.invalidate(); expect(readdirSync(root)).toEqual(["chosen.txt"]);
    expect(attachmentSaveName("C:\\outside/../report.json")).toBe("C__outside_.._report.json");
  });
  it("discards a late picker result after invalidation and refuses hidden-window grants", async () => {
    const destination = join(tempRoot(), "late.txt"), test = fixture(destination);
    let finish: ((path: string | null) => void) | undefined;
    test.chooseSave.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    const saving = test.bridge.startSave(test.event, test.info); await vi.waitFor(() => expect(test.chooseSave).toHaveBeenCalled());
    await test.bridge.invalidate(); finish?.(destination); expect(await saving).toBeNull(); expect(existsSync(destination)).toBe(false);
    test.hide(); await expect(test.bridge.preview(test.event, test.info)).rejects.toThrow("Show the app");
  });
  it("does not grant a call that crossed invalidation while awaiting authorization", async () => {
    const test = fixture(), pending = test.bridge.preview(test.event, test.info);
    const failed = expect(pending).rejects.toThrow("closed");
    await test.bridge.invalidate(); await failed;
  });
  it("keeps repeated clear requests failed while a native cleanup remains unresolved", async () => {
    const test = fixture(); let blocked = true;
    test.speech.cancelAll.mockImplementation(async () => { if (blocked) throw new Error("Synthetic retained raw audio"); });
    await expect(test.bridge.invalidate()).rejects.toThrow("could not be cleared");
    await expect(test.bridge.invalidate()).rejects.toThrow("could not be cleared");
    await expect(test.bridge.preview(test.event, test.info)).rejects.toThrow("could not be cleared");
    blocked = false; await test.bridge.invalidate(); await test.bridge.preview(test.event, test.info); await test.bridge.invalidate();
  });
  it("fences speech final events and session commands across cancellation and page changes", async () => {
    const test = fixture(); await test.bridge.startSpeech(test.event, { sessionId: "first", language: "en" });
    expect(test.bridge.isCapturing()).toBe(true);
    test.bridge.emitSpeech({ kind: "session", sessionId: "first", phase: "listening" }); expect(test.send).toHaveBeenCalledWith("speech:event", expect.objectContaining({ sessionId: "first" }));
    await test.bridge.appendPcm(test.event, { sessionId: "first", sequence: 0, pcm: new Uint8Array([1, 0]) });
    await test.bridge.invalidate(); test.send.mockClear();
    test.bridge.emitSpeech({ kind: "final", sessionId: "first", text: "late synthetic transcript" }); expect(test.send).not.toHaveBeenCalled();
    await test.bridge.startSpeech(test.event, { sessionId: "second", language: "en" });
    await expect(test.bridge.stopSpeech(test.event, { sessionId: "first" })).rejects.toThrow("no longer active");
    test.bridge.emitSpeech({ kind: "final", sessionId: "second", text: "editable synthetic text" }); expect(test.send).toHaveBeenCalledWith("speech:event", expect.objectContaining({ text: "editable synthetic text" }));
    test.contents.mainFrame = { url: "app://tjommie/other/" }; expect(test.bridge.isCapturing()).toBe(false); await test.bridge.invalidate();
  });
  it("revokes microphone permission and late finals during blur cleanup without closing a file preview", async () => {
    const test = fixture(); await test.bridge.startSpeech(test.event, { sessionId: "blurred", language: "en" }); await test.bridge.preview(test.event, test.info);
    let finish: (() => void) | undefined; test.speech.cancelAll.mockImplementationOnce(() => new Promise<void>((resolve) => { finish = resolve; }));
    const cancelled = test.bridge.cancelCapture(); expect(test.bridge.isCapturing()).toBe(false);
    test.bridge.emitSpeech({ kind: "final", sessionId: "blurred", text: "late synthetic text" }); expect(test.send).not.toHaveBeenCalled();
    expect(test.bridge.permitsAttachmentRequest({ webContentsId: 17, frame: test.frame, resourceType: "image" })).toBe(true);
    finish?.(); await cancelled; await test.bridge.invalidate();
  });
});
