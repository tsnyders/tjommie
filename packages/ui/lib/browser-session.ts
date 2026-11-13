import { ClientRpcError } from "@tjommie/client";
import type { BrowserFrame, BrowserInput, BrowserState, BrowserStreamFrame, MethodName, ParamsIn, Result, StreamedBrowserFrame } from "@tjommie/protocol";
import { errorText } from "./labels";
export { addressToUrl, keyRoute, mouseButton, wheelPixels } from "./browser-geometry";

export type BrowserCall = <M extends MethodName>(method: M, params: ParamsIn<M>) => Promise<Result<M>>;
// Subscribes to the live stream's frames on the connection the calls go to, and returns the unsubscribe.
export type BrowserListen = (listener: (data: BrowserStreamFrame) => void) => () => void;
export type BrowserView = { state: BrowserState | null; frame: BrowserFrame | null; error: string | null; busy?: boolean };
const closes = new Map<string, Promise<void>>();
const ACTIVE_POLL_MS = 33;
const IDLE_POLL_MS = 250;
const RETRY_MS = 1000;
type Task = { kind?: "move" | "wheel" | "resize"; key?: string; input?: BrowserInput; release?: boolean; cancel?: () => void; run: () => Promise<void> };
type Button = "left" | "middle" | "right";
type InputTarget = { generation: string; pageId: string; viewportRevision?: number; controlEpoch?: number };
type HeldPointer = InputTarget & { x: number; y: number };

// Whether a frame is of the page, layout, controller and private session the state describes.
const fits = (frame: BrowserFrame, state: BrowserState): boolean => (!frame.pageId || frame.pageId === state.activePageId)
  && (frame.controlEpoch === undefined || frame.controlEpoch === state.controlEpoch)
  && (frame.privateEpoch === undefined || frame.privateEpoch === state.privateEpoch)
  && (frame.viewportRevision === undefined || frame.viewportRevision === state.viewportRevision);

// Input and commands stay ordered. One independent, bounded frame request can never hold up a key
// release or dialog answer. Local epochs and server revisions fence responses across either lane.
// While shown, a daemon's live stream brings the pictures, and the poll reads only the state.
export class BrowserSession {
  view: BrowserView = { state: null, frame: null, error: null, busy: false };
  #tasks: Task[] = [];
  #running = false;
  #timer: ReturnType<typeof setTimeout> | undefined;
  #framePending: Promise<void> | null = null;
  #cursor: { streamId: string; sequence: number } | undefined;
  #epoch = 0;
  #fences = 0;
  #activeUntil = 0;
  #visible = true;
  #suspended = false;
  #suspending: Promise<void> | null = null;
  #closing = false;
  #done: Promise<void> | null = null;
  #human = false;
  #askedHuman = false;
  #keys = new Map<string, InputTarget>();
  #buttons = new Map<Button, HeldPointer>();
  #heldKeys = new Map<string, InputTarget>();
  #heldButtons = new Map<Button, HeldPointer>();
  // The live stream: listening while it is asked for or on. An older daemon (unknown_method) is only polled.
  #unlisten: (() => void) | undefined;
  #unsupported = false;
  #streamCalls: Promise<unknown> = Promise.resolve(); // on and off reach the daemon in the order they were asked
  #awaiting: StreamedBrowserFrame | null = null; // the streamed frame handed to the view, acknowledged once drawn
  #behind = false; // a streamed frame was dropped, so a pulled one brings the picture up to date

  constructor(private readonly botId: string, private readonly call: BrowserCall, private readonly onChange: (view: BrowserView) => void, private readonly listen?: BrowserListen) {}

  open(): void {
    const earlier = closes.get(this.botId);
    this.#push({ run: async () => {
      await earlier;
      this.#apply(await this.call("browser.open", { botId: this.botId }));
      this.#poll(0);
    } });
  }

  // Backgrounding releases this device's input/control as well as pausing pictures.
  setVisible(visible: boolean): void {
    this.#visible = visible;
    if (visible) { this.#suspended = false; this.#poll(0); }
    else { void this.suspend(); }
  }

  suspend(): Promise<void> {
    if (this.#closing) return this.#done ?? Promise.resolve();
    this.#visible = false;
    this.#suspended = true;
    this.#epoch++;
    this.#stopTimer();
    this.#stopStream();
    this.#discardPresses();
    if (this.#suspending) return this.#suspending;
    let finish = (): void => {};
    const pending = new Promise<void>((resolve) => (finish = resolve));
    this.#suspending = pending;
    this.#set({ frame: null });
    this.#push({ release: true, run: async () => {
      try { await this.#handBack(); }
      catch (error) { this.#set({ error: errorText(error) }); }
      finally {
        this.#suspending = null;
        this.#epoch++;
        this.#set({});
        finish();
        this.#poll(0);
      }
    } });
    return pending;
  }

  // Terminal unmount cleanup. A new session waits for this one, so a late handback cannot steal
  // a new takeover, nor a late stream stop end its stream. Suspending instead retains this session,
  // its frame and the UI's address draft.
  close(): Promise<void> {
    if (this.#done) return this.#done;
    this.#stopTimer();
    this.#stopStream();
    this.#epoch++;
    this.#discardPresses();
    let finish = (): void => {};
    const done = new Promise<void>((resolve) => (finish = resolve)).finally(() => {
      if (closes.get(this.botId) === done) closes.delete(this.botId);
    });
    this.#done = done;
    closes.set(this.botId, done);
    this.#closing = true;
    this.#push({ release: true, run: async () => {
      try { await this.#handBack(); }
      catch { /* best effort at terminal unmount */ }
      finally { await this.#streamCalls; finish(); }
    } });
    return done;
  }

  // The view drew this picture, so the stream may send the next.
  drawn(frame: BrowserFrame): void {
    if (frame === this.#awaiting) this.#ack();
  }

  #discardPresses(): void {
    for (const task of this.#tasks) if (!task.release) task.cancel?.();
    this.#tasks = this.#tasks.filter((task) => task.release);
    this.#keys.clear();
    this.#buttons.clear();
  }

  async #handBack(): Promise<void> {
    let state: BrowserState | null = null;
    // Read ownership/dialog state without waiting for an outstanding image or opening a second frame lane.
    try { state = await this.call("browser.open", { botId: this.botId }); }
    catch { /* use last known ownership */ }
    if (state) this.#apply(state);
    const human = state ? state.control === "human" && state.canControl !== false : this.#human || this.#askedHuman;
    if (!human) return;
    const pageId = state?.activePageId;
    if (state && pageId) {
      const { generation } = state;
      if (state.dialog) await this.call("browser.dialog", { botId: this.botId, generation, pageId: state.dialog.pageId, controlEpoch: state.controlEpoch, accept: false }).catch(() => {});
      for (const [button, at] of [...this.#heldButtons]) await this.#send(at.generation, at.pageId, { kind: "mouse", action: "up", x: at.x, y: at.y, button }, at.viewportRevision, at.controlEpoch).catch(() => {});
      for (const [key, target] of [...this.#heldKeys]) await this.#send(target.generation, target.pageId, { kind: "key", action: "up", key }, target.viewportRevision, target.controlEpoch).catch(() => {});
    }
    this.#apply(await this.call("browser.control", { botId: this.botId, control: "bot", generation: state?.generation, controlEpoch: state?.controlEpoch, releaseOnly: true }));
    this.#heldKeys.clear(); this.#heldButtons.clear();
  }

  releaseAll(): void {
    for (const [button, at] of this.#buttons) this.input({ kind: "mouse", action: "up", x: at.x, y: at.y, button });
    for (const key of [...this.#keys.keys()]) this.input({ kind: "key", action: "up", key });
  }

  input(input: BrowserInput): void {
    const state = this.view.state;
    const pageId = state?.activePageId;
    if (!state || !pageId || state.control !== "human" || state.canControl === false || state.privateActive || this.#closing) return;
    const release = (input.kind === "key" || input.kind === "mouse") && input.action === "up";
    if (!release && (!this.view.frame || this.view.busy || this.#suspended)) return;
    if (!release && this.view.frame && (this.view.frame.controlEpoch !== undefined && this.view.frame.controlEpoch !== state.controlEpoch || this.view.frame.privateEpoch !== undefined && this.view.frame.privateEpoch !== state.privateEpoch)) return;
    let target: InputTarget = { generation: state.generation, pageId, viewportRevision: state.viewportRevision, controlEpoch: state.controlEpoch };
    if (input.kind === "key") {
      if (input.action === "up" && !this.#keys.has(input.key)) return;
      if (input.action === "down") this.#keys.set(input.key, target);
      else { target = this.#keys.get(input.key)!; this.#keys.delete(input.key); }
    }
    if (input.kind === "mouse" && input.button) {
      if (input.action === "down") this.#buttons.set(input.button, { ...target, x: input.x, y: input.y });
      if (input.action === "up") { target = this.#buttons.get(input.button) ?? target; this.#buttons.delete(input.button); }
    }
    const task: Task = {
      kind: input.kind === "wheel" ? "wheel" : input.kind === "mouse" && input.action === "move" ? "move" : undefined,
      key: `${target.generation}:${target.pageId}:${target.viewportRevision ?? ""}:${target.controlEpoch ?? ""}`, input, release,
      run: async () => { await this.#send(target.generation, target.pageId, task.input!, target.viewportRevision, target.controlEpoch); this.#wake(); },
    };
    const last = this.#tasks.at(-1);
    if (task.kind === "move" && last?.kind === "move" && last.key === task.key) this.#tasks[this.#tasks.length - 1] = task;
    else if (input.kind === "wheel" && last?.kind === "wheel" && last.key === task.key && last.input?.kind === "wheel"
      && last.input.x === input.x && last.input.y === input.y && Math.abs(last.input.deltaX + input.deltaX) <= 10_000 && Math.abs(last.input.deltaY + input.deltaY) <= 10_000) {
      last.input = { ...input, deltaX: last.input.deltaX + input.deltaX, deltaY: last.input.deltaY + input.deltaY };
    } else this.#push(task);
    this.#wake();
  }

  setControl(control: BrowserState["control"]): void {
    if (this.#suspended || this.#closing) return;
    if (control === "human") this.#askedHuman = true;
    const state = this.view.state;
    this.#command(async () => this.#apply(await this.call("browser.control", { botId: this.botId, control, generation: state?.generation, controlEpoch: state?.controlEpoch })));
  }
  navigate(url: string): void { this.#onPage((generation, pageId, controlEpoch) => this.call("browser.navigate", { botId: this.botId, generation, pageId, controlEpoch, url }), true); }
  history(action: "back" | "forward" | "reload"): void { this.#onPage((generation, pageId, controlEpoch) => this.call("browser.history", { botId: this.botId, generation, pageId, controlEpoch, action }), true); }
  resize(width: number, height: number): void { this.#onPage((generation, pageId, controlEpoch) => this.call("browser.resize", { botId: this.botId, generation, pageId, controlEpoch, width, height }), true, "resize"); }
  dialog(accept: boolean, promptText?: string): void { this.#onPage((generation, pageId, controlEpoch) => this.call("browser.dialog", { botId: this.botId, generation, pageId, controlEpoch, accept, promptText })); }
  tab(action: "new" | "select" | "close", pageId?: string): void {
    const generation = this.view.state?.generation;
    const controlEpoch = this.view.state?.controlEpoch;
    if (generation) this.#command(async () => this.#apply(await this.call("browser.tab", { botId: this.botId, generation, controlEpoch, action, pageId })), true);
  }

  #onPage(run: (generation: string, pageId: string, controlEpoch?: number) => Promise<BrowserState>, reshape = false, kind?: "resize"): void {
    const state = this.view.state;
    if (state?.activePageId) {
      const pageId = state.activePageId;
      this.#command(async () => this.#apply(await run(state.generation, pageId, state.controlEpoch)), reshape, kind, `${state.generation}:${pageId}`);
    }
  }

  #command(run: () => Promise<void>, reshape = false, kind?: "resize", key?: string): void {
    if (this.#closing || this.#suspended) return;
    this.#epoch++; this.#fences++;
    if (reshape) this.#cursor = undefined;
    this.#set(reshape ? { frame: null } : {});
    const finish = (): void => { this.#fences--; this.#epoch++; this.#set({}); this.#wake(); };
    const task: Task = { kind, key, cancel: finish, run: async () => { try { await run(); } finally { finish(); } } };
    const last = this.#tasks.at(-1);
    if (kind === "resize" && last?.kind === "resize" && last.key === key) { last.cancel?.(); this.#tasks[this.#tasks.length - 1] = task; }
    else this.#push(task);
  }

  async #send(generation: string, pageId: string, input: BrowserInput, viewportRevision?: number, controlEpoch?: number): Promise<void> {
    await this.call("browser.input", { botId: this.botId, generation, pageId, input, viewportRevision, controlEpoch });
    if (input.kind === "key") { if (input.action === "down") this.#heldKeys.set(input.key, { generation, pageId, viewportRevision, controlEpoch }); else this.#heldKeys.delete(input.key); }
    else if (input.kind === "mouse" && input.button) {
      if (input.action === "down") this.#heldButtons.set(input.button, { generation, pageId, viewportRevision, controlEpoch, x: input.x, y: input.y });
      else if (input.action === "up") this.#heldButtons.delete(input.button);
    }
  }

  #push(task: Task): void {
    if (this.#closing && !task.release) { task.cancel?.(); return; }
    this.#tasks.push(task);
    void this.#pump();
  }
  async #pump(): Promise<void> {
    if (this.#running) return;
    this.#running = true;
    try {
      for (let task = this.#tasks.shift(); task; task = this.#tasks.shift()) {
        try { await task.run(); } catch (error) { this.#set({ error: errorText(error) }); }
      }
    } finally { this.#running = false; }
  }

  #wake(): void {
    if (this.#current()) return; // the stream brings the next picture
    this.#activeUntil = Date.now() + 1000; this.#stopTimer(); this.#poll(0);
  }
  #poll(delay: number): void {
    if (this.#closing || this.#suspended || this.#suspending || !this.#visible || this.#framePending || this.#timer !== undefined || !this.view.state) return;
    this.#timer = setTimeout(() => {
      this.#timer = undefined;
      const epoch = this.#epoch;
      let retry = false;
      const pending = this.#pull(epoch).catch((error: unknown) => {
        retry = true;
        if (epoch === this.#epoch) this.#set({ error: errorText(error) });
      }).finally(() => {
        this.#framePending = null;
        this.#poll(retry ? RETRY_MS : !this.#unlisten && (Date.now() < this.#activeUntil || this.view.busy) ? ACTIVE_POLL_MS : IDLE_POLL_MS);
      });
      this.#framePending = pending;
    }, delay);
  }
  async #pull(epoch: number): Promise<void> {
    if (this.#current()) { // the tabs, address, dialog and control; the pictures come with the stream
      const state = await this.call("browser.open", { botId: this.botId });
      if (!this.#closing && this.#visible && epoch === this.#epoch && this.#fences === 0) this.#apply(state);
      return;
    }
    const shown = this.view.frame;
    const result = await this.call("browser.frame", { botId: this.botId, after: this.#cursor });
    if (this.#closing || !this.#visible || epoch !== this.#epoch || this.#fences > 0 || !this.#fresh(result.state)) return;
    let frame = result.unchanged ? this.view.frame : result.frame;
    if (this.#unlisten && this.view.frame !== shown) frame = this.view.frame; // a streamed frame came meanwhile
    if (frame && !fits(frame, result.state)) frame = null;
    if (frame?.streamId) this.#cursor = { streamId: frame.streamId, sequence: frame.sequence };
    else if (!frame) this.#cursor = undefined;
    const before = this.view.frame;
    if (frame && before && frame.data === before.data && frame.width === before.width && frame.height === before.height
      && frame.viewportWidth === before.viewportWidth && frame.viewportHeight === before.viewportHeight
      && frame.controlEpoch === before.controlEpoch && frame.privateEpoch === before.privateEpoch
      && frame.pageId === before.pageId && frame.viewportRevision === before.viewportRevision && frame.streamId === before.streamId) frame = before;
    this.#behind = false;
    this.#set({ state: result.state, frame, error: null });
    if (!result.state.privateActive) this.#startStream();
  }

  // The stream keeps the picture current, so polls read only the state and input needs no follow-up picture.
  #current(): boolean {
    const { frame, state } = this.view;
    return !!this.#unlisten && !this.#behind && !!frame && !!state && fits(frame, state);
  }
  #startStream(): void {
    if (!this.listen || this.#unlisten || this.#unsupported) return;
    const off = this.listen((data) => this.#streamed(data)); // before asking, so no frame is missed
    this.#unlisten = off;
    void this.#streamCall(true).then((state) => { if (this.#unlisten === off) this.#apply(state); }, (error: unknown) => {
      if (this.#unlisten !== off) return; // stopped or asked again meanwhile
      this.#endStream();
      this.#unsupported = error instanceof ClientRpcError && error.code === "unknown_method";
    });
  }
  // Hidden, closed or private: the daemon stops sending.
  #stopStream(): void {
    if (!this.#unlisten) return;
    this.#endStream();
    void this.#streamCall(false).catch(() => {});
  }
  #endStream(): void {
    this.#unlisten?.();
    this.#unlisten = undefined;
    this.#ack();
  }
  #streamCall(on: boolean): Promise<BrowserState> {
    const answer = this.#streamCalls.then(() => this.call("browser.stream", { botId: this.botId, on }));
    this.#streamCalls = answer.catch(() => {});
    return answer;
  }
  #streamed({ botId, frame }: BrowserStreamFrame): void {
    if (botId !== this.botId) return;
    // It ended (a private interaction, the browser closed or the desktop stopped it). The next poll pulls browser.frame, and asks
    // for the stream again once it isn't private. Paced by the poll, so a desktop that keeps stopping it can't make a loop.
    if (!frame) { this.#endStream(); return; }
    this.#ack(); // the frame before it goes first, drawn or now replaced
    this.#awaiting = frame;
    const state = this.view.state;
    if (this.#fences === 0 && state && fits(frame, state)) { this.#behind = false; this.#set({ frame }); }
    else { this.#behind = true; this.#ack(); } // mid-command, or of another page or controller
  }
  // Every streamed frame is acknowledged once, in order: when drawn, replaced or dropped. The daemon holds at most two.
  #ack(): void {
    const frame = this.#awaiting;
    if (!frame) return;
    this.#awaiting = null;
    void this.call("browser.streamAck", { botId: this.botId, sequence: frame.sequence }).catch(() => {});
  }
  #stopTimer(): void { clearTimeout(this.#timer); this.#timer = undefined; }
  #fresh(state: BrowserState): boolean {
    const before = this.view.state;
    return before?.generation !== state.generation || before.revision === undefined || state.revision === undefined || state.revision >= before.revision;
  }
  #apply(state: BrowserState): void { if (this.#fresh(state)) this.#set({ state, error: null }); }
  #set(patch: Partial<BrowserView>): void {
    if (patch.state) { this.#human = patch.state.control === "human"; if (!this.#human) this.#askedHuman = false; }
    if (patch.state?.privateActive) this.#stopStream(); // asked for again once a poll shows it over
    if (this.#closing) return;
    const before = this.view;
    const state = patch.state && JSON.stringify(patch.state) !== JSON.stringify(before.state) ? patch.state : before.state;
    const moved = state && (state.generation !== before.state?.generation || state.activePageId !== before.state.activePageId || state.viewportRevision !== before.state.viewportRevision);
    if (moved) this.#cursor = undefined;
    const frame = state?.privateActive ? null : patch.frame !== undefined ? patch.frame : moved ? null : before.frame;
    if (this.#awaiting && frame !== this.#awaiting) this.#ack(); // replaced before it was drawn
    const next = { ...before, ...patch, state, frame, busy: this.#fences > 0 || !!this.#suspending || !!state?.busy };
    if (next.state === before.state && next.frame === before.frame && next.error === before.error && next.busy === before.busy) return;
    this.view = next;
    this.onChange(next);
  }
}

// Legacy callers use the whole rectangle; new object-contain layouts use browser-geometry.framePoint.
export function pagePoint(box: { left: number; top: number; width: number; height: number }, frame: { width: number; height: number }, clientX: number, clientY: number): { x: number; y: number } {
  const at = (offset: number, size: number, of: number): number => size > 0 ? Math.min(Math.max(of - 1, 0), Math.max(0, Math.round(offset / size * of))) : 0;
  return { x: at(clientX - box.left, box.width, frame.width), y: at(clientY - box.top, box.height, frame.height) };
}
export function viewportSize(width: number): { width: number; height: number } {
  const w = Math.min(2560, Math.max(240, Math.round(width)));
  return { width: w, height: Math.min(1600, Math.max(180, Math.round(w * 3 / 4))) };
}
