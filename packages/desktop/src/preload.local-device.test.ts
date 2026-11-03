import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { expect, it, vi } from "vitest";
import type { AttachmentNativeApi } from "../../protocol/src/attachments.ts";
import type { SpeechApi } from "../../protocol/src/speech.ts";
import type { VaultNativeApi } from "../../protocol/src/vault.ts";

it("exposes only the typed local file/speech actions, strips IPC events and removes listeners", async () => {
  const listeners = new Map<string, (_event: unknown, value: unknown) => void>();
  const invoke = vi.fn(async (_channel: string, ..._args: unknown[]) => undefined);
  const removeListener = vi.fn((channel: string) => listeners.delete(channel)); let exposed: unknown;
  runInNewContext(readFileSync(new URL("./preload.cjs", import.meta.url), "utf8"), {
    require: () => ({ contextBridge: { exposeInMainWorld: (_name: string, api: unknown) => { exposed = api; } }, ipcRenderer: { invoke, send: vi.fn(), on: (channel: string, listener: (_event: unknown, value: unknown) => void) => listeners.set(channel, listener), removeListener }, webUtils: { getPathForFile: vi.fn() } }),
  });
  const api = exposed as { attachments: AttachmentNativeApi; speech: SpeechApi; vault: VaultNativeApi; diagnostics: { info(): Promise<unknown> } };
  const info = { id: "41f8bac7-358c-48cf-9f48-a3a358804da6", hostId: "host", name: "file.txt", size: 3, contentType: "text/markdown", sha256: "0".repeat(64) };
  await api.attachments.startSave(info); await api.attachments.appendSave("save", 0, "YWJj"); await api.attachments.finishSave("save"); await api.attachments.cancelSave("save");
  await api.attachments.preview(info); await api.attachments.respond("preview", "request", null); await api.attachments.closePreview("preview"); await api.attachments.invalidate();
  await api.speech.status(); await api.speech.downloadModel(); await api.speech.cancelModelDownload(); await api.speech.removeModel();
  await api.speech.start({ sessionId: "speech", language: "en" }); await api.speech.appendPcm({ sessionId: "speech", sequence: 0, pcm: new Uint8Array([1, 0]) }); await api.speech.stop({ sessionId: "speech" }); await api.speech.cancel({ sessionId: "speech" });
  expect(invoke.mock.calls.map(([channel]) => channel)).toEqual(["attachments:start-save", "attachments:append-save", "attachments:finish-save", "attachments:cancel-save", "attachments:preview", "attachments:respond", "attachments:close-preview", "attachments:invalidate", "speech:status", "speech:download-model", "speech:cancel-model-download", "speech:remove-model", "speech:start", "speech:append-pcm", "speech:stop", "speech:cancel"]);
  expect(invoke).toHaveBeenCalledWith("attachments:start-save", info);
  await api.vault.command({ kind: "lock" });
  expect(invoke).toHaveBeenLastCalledWith("vault:command", { kind: "lock" });
  const read = vi.fn(), final = vi.fn(), offRead = api.attachments.onRead(read), offSpeech = api.speech.onEvent(final);
  const request = { previewId: "preview", requestId: "request", offset: 0, length: 3 }, event = { kind: "final", sessionId: "speech", text: "synthetic editable text" };
  listeners.get("attachments:read")?.({ internal: "synthetic IPC event" }, request); listeners.get("speech:event")?.({ internal: "synthetic IPC event" }, event);
  expect(read).toHaveBeenCalledWith(request); expect(final).toHaveBeenCalledWith(event);
  offRead(); offSpeech(); expect(removeListener).toHaveBeenCalledTimes(2); expect(listeners.size).toBe(0);
  await api.diagnostics.info(); expect(invoke).toHaveBeenLastCalledWith("diagnostics:info");
});
