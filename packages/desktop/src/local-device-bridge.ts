import { randomUUID } from "node:crypto";
import { AttachmentInfo, type AttachmentNativeRead } from "../../protocol/src/attachments.ts";
import { SpeechEvent, SpeechPcm, SpeechSession, SpeechStart, type SpeechApi } from "../../protocol/src/speech.ts";
import { AttachmentProtocol, AttachmentReadRequests } from "./attachment-protocol.ts";
import { AttachmentSaves } from "./attachment-save.ts";
import { speechOrigin } from "./speech-permissions.ts";

type Frame = { url: string };
type Contents = { id: number; mainFrame: Frame; getURL(): string; isDestroyed(): boolean; send(channel: string, value: unknown): void };
type Window = { webContents: Contents; isDestroyed(): boolean; isVisible(): boolean; isFocused(): boolean };
type Event = { sender: Contents; senderFrame: Frame | null };
type Speech = Omit<SpeechApi, "onEvent"> & { cancelAll(): Promise<void>; isCapturing(): boolean };
type Options = { window(): Window | null; ready(): boolean; production: boolean; speech: Speech; chooseSave(info: AttachmentInfo): Promise<string | null> };
type Lease = { owner: string; window: Window; frame: Frame; controller: AbortController };

export const isAttachmentUrl = (value: string): boolean => { try { const url = new URL(value); return url.protocol === "app:" && url.host === "tjommie" && url.pathname.startsWith("/_attachments/"); } catch { return false; } };
export const attachmentSaveName = (name: string): string => name.replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").replace(/[. ]+$/g, "") || "file";

/** Only the current app main frame receives local files, microphone access or final text. */
export class DesktopDeviceBridge {
  private readonly options: Options;
  private readonly protocol = new AttachmentProtocol();
  private readonly reads = new AttachmentReadRequests();
  private readonly saves = new AttachmentSaves();
  private lease?: Lease;
  private cleanup: Promise<void> = Promise.resolve();
  private readonly owners = new Set<string>();
  private ticket?: { id: string; lease: Lease };
  private picking = false;
  private generation = 0;

  constructor(options: Options) { this.options = options; }
  private trusted(event: Event): Window {
    const window = this.options.window();
    if (!this.options.ready() || !window || window.isDestroyed() || event.sender !== window.webContents || event.sender.isDestroyed() || !event.senderFrame || event.senderFrame !== window.webContents.mainFrame || !speechOrigin(event.senderFrame.url, this.options.production) || !speechOrigin(event.sender.getURL(), this.options.production)) throw new Error("Untrusted local device request.");
    return window;
  }
  private valid(lease: Lease): boolean {
    const window = this.options.window();
    return this.options.ready() && this.lease === lease && !lease.controller.signal.aborted && window === lease.window && !window.isDestroyed() && window.isVisible() && !window.webContents.isDestroyed() && window.webContents.mainFrame === lease.frame && speechOrigin(lease.frame.url, this.options.production) && speechOrigin(window.webContents.getURL(), this.options.production);
  }
  private current(event: Event): Lease { this.trusted(event); if (!this.lease || !this.valid(this.lease)) throw new Error("The local device connection is closed."); return this.lease; }
  private async authorize(event: Event): Promise<Lease> {
    this.trusted(event);
    const generation = this.generation;
    if (this.lease && !this.valid(this.lease)) { void this.invalidate().catch(() => {}); throw new Error("The local device connection is closed."); }
    await this.cleanup;
    if (generation !== this.generation) throw new Error("The local device connection is closed.");
    const window = this.trusted(event);
    if (!window.isVisible()) throw new Error("Show the app before opening a local file or microphone.");
    if (!this.lease) { this.lease = { owner: randomUUID(), window, frame: window.webContents.mainFrame, controller: new AbortController() }; this.owners.add(this.lease.owner); }
    return this.current(event);
  }
  invalidate(): Promise<void> {
    this.generation++;
    const lease = this.lease; this.lease = undefined; this.ticket = undefined;
    lease?.controller.abort();
    for (const owner of this.owners) { this.protocol.closeOwner(owner); this.reads.cancel(owner); }
    const previous = this.cleanup;
    const speech = this.options.speech.cancelAll();
    const saves = [...this.owners].map(async (owner) => { await this.saves.cancelOwner(owner); this.owners.delete(owner); });
    this.cleanup = Promise.allSettled([previous, speech, ...saves]).then((results) => { if (results.slice(1).some((result) => result.status === "rejected")) throw new Error("Temporary local device data could not be cleared. Try again or reopen the app."); });
    void this.cleanup.catch(() => {});
    return this.cleanup;
  }
  async invalidateFrom(event: Event): Promise<void> { this.trusted(event); await this.invalidate(); }
  async startSave(event: Event, input: unknown): Promise<{ saveId: string } | null> {
    const lease = await this.authorize(event), info = AttachmentInfo.parse(input);
    if (this.picking) throw new Error("Another save picker is already open.");
    this.picking = true;
    try {
      const destination = await this.options.chooseSave(info);
      if (!destination || !this.valid(lease)) return null;
      const save = await this.saves.start(lease.owner, info, destination, lease.controller.signal);
      if (!this.valid(lease)) { await this.saves.cancelOwner(lease.owner); return null; }
      return save;
    } finally { this.picking = false; }
  }
  async appendSave(event: Event, id: unknown, offset: unknown, bytes: unknown): Promise<void> {
    const lease = this.current(event);
    if (typeof offset !== "number") throw new Error("Invalid download offset.");
    await this.saves.append(lease.owner, AttachmentInfo.shape.id.parse(id), offset, bytes);
  }
  async finishSave(event: Event, id: unknown): Promise<{ status: "saved" | "cancelled" }> {
    const lease = this.current(event), result = await this.saves.finish(lease.owner, AttachmentInfo.shape.id.parse(id));
    return this.valid(lease) ? result : { status: "cancelled" };
  }
  async cancelSave(event: Event, id: unknown): Promise<void> { const lease = this.current(event); await this.saves.cancel(lease.owner, AttachmentInfo.shape.id.parse(id)); }
  async preview(event: Event, input: unknown): Promise<{ previewId: string; url: string }> {
    const lease = await this.authorize(event); let previewId = "";
    const preview = this.protocol.create(lease.owner, AttachmentInfo.parse(input), (offset, length) => this.reads.request(lease.owner, previewId, offset, length, (request: AttachmentNativeRead) => { if (!this.valid(lease)) throw new Error("Preview closed."); lease.window.webContents.send("attachments:read", request); }), lease.controller.signal);
    previewId = preview.handle; return { previewId, url: preview.url };
  }
  respond(event: Event, previewId: unknown, requestId: unknown, bytes: unknown): void { const lease = this.current(event); this.reads.respond(lease.owner, AttachmentInfo.shape.id.parse(previewId), AttachmentInfo.shape.id.parse(requestId), bytes); }
  closePreview(event: Event, value: unknown): void { const lease = this.current(event), id = AttachmentInfo.shape.id.parse(value); this.protocol.close(lease.owner, id); this.reads.cancel(lease.owner, id); }
  permitsAttachmentRequest(request: { webContentsId?: number; frame?: Frame | null; resourceType: string }): boolean {
    const lease = this.lease;
    return !!lease && this.valid(lease) && request.webContentsId === lease.window.webContents.id && request.frame === lease.frame && ["xhr", "image", "media", "other"].includes(request.resourceType);
  }
  serve(request: Request): Promise<Response> { const lease = this.lease; return lease && this.valid(lease) ? this.protocol.serve(request, lease.owner) : Promise.resolve(new Response(null, { status: 404, headers: { "cache-control": "no-store" } })); }
  isCapturing(): boolean { return !!this.lease && this.ticket?.lease === this.lease && this.valid(this.lease) && this.options.speech.isCapturing(); }
  async cancelCapture(): Promise<void> { this.ticket = undefined; await this.options.speech.cancelAll(); }
  status(event: Event) { this.trusted(event); return this.options.speech.status(); }
  async downloadModel(event: Event): Promise<void> { await this.authorize(event); await this.options.speech.downloadModel(); }
  async cancelModelDownload(event: Event): Promise<void> { this.trusted(event); await this.options.speech.cancelModelDownload(); }
  async removeModel(event: Event): Promise<void> { this.trusted(event); this.ticket = undefined; await this.options.speech.removeModel(); }
  async startSpeech(event: Event, input: unknown): Promise<void> {
    const lease = await this.authorize(event), request = SpeechStart.parse(input);
    if (!lease.window.isFocused()) throw new Error("Focus the app before starting the microphone.");
    if (this.ticket) throw new Error("Another dictation is active.");
    const ticket = { id: request.sessionId, lease }; this.ticket = ticket;
    try { await this.options.speech.start(request); if (!this.valid(lease)) throw new Error("Dictation cancelled."); }
    catch (error) { if (this.ticket === ticket) this.ticket = undefined; throw error; }
  }
  private speechTicket(event: Event, input: unknown): void { const lease = this.current(event), request = SpeechSession.parse(input); if (this.ticket?.id !== request.sessionId || this.ticket.lease !== lease) throw new Error("This recording is no longer active."); }
  async appendPcm(event: Event, input: unknown): Promise<void> { this.trusted(event); const chunk = SpeechPcm.parse(input); this.speechTicket(event, { sessionId: chunk.sessionId }); await this.options.speech.appendPcm(chunk); }
  async stopSpeech(event: Event, input: unknown): Promise<void> { this.speechTicket(event, input); await this.options.speech.stop(SpeechSession.parse(input)); }
  async cancelSpeech(event: Event, input: unknown): Promise<void> { this.trusted(event); const request = SpeechSession.parse(input); if (this.ticket?.id === request.sessionId) { this.ticket = undefined; await this.options.speech.cancel(request); } }
  emitSpeech(input: unknown): void {
    const event = SpeechEvent.parse(input), window = this.options.window();
    if (!this.options.ready() || !window || window.isDestroyed() || window.webContents.isDestroyed() || !speechOrigin(window.webContents.mainFrame.url, this.options.production) || !speechOrigin(window.webContents.getURL(), this.options.production)) return;
    if (event.kind !== "status") {
      if (!this.ticket || this.ticket.id !== event.sessionId || !this.valid(this.ticket.lease)) return;
      if (event.kind === "final" || event.phase === "cancelled" || event.phase === "error") this.ticket = undefined;
    }
    window.webContents.send("speech:event", event);
  }
}
