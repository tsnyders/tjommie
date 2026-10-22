import type { BrowserFrame, BrowserInput, BrowserState, MethodName, ParamsIn, Result } from "@tjommie/protocol";
import { errorText } from "./labels";

export type BrowserCall = <M extends MethodName>(method: M, params: ParamsIn<M>) => Promise<Result<M>>;
export type BrowserView = { state: BrowserState | null; frame: BrowserFrame | null; error: string | null };

// A bot's close that hasn't finished handing the browser back. A reopened panel waits for it, so the old close can't
// take back control the new one just asked for.
const closes = new Map<string, Promise<void>>();
const POLL_MS = 200;
const RETRY_MS = 1000;
type Task = { move?: boolean; release?: boolean; run: () => Promise<void> };
type Button = "left" | "middle" | "right";

// One bot's browser as the panel sees it. Every daemon call (open, frame pulls, controls and input) goes through one
// queue, so nothing overlaps and input reaches the page in the order it happened. Closing it never closes the browser.
export class BrowserSession {
  view: BrowserView = { state: null, frame: null, error: null };
  #tasks: Task[] = [];
  #running = false;
  #timer: ReturnType<typeof setTimeout> | undefined;
  #framing = false; // a frame pull is queued or running
  #visible = true;
  #closing = false;
  #done: Promise<void> | null = null;
  #human = false; // the browser last said the user is in control
  #changing = 0; // navigations, history moves and resizes queued or in flight: the page is about to look different
  #asking = 0; // Take control calls queued or in flight
  #keys = new Set<string>(); // keys the user has pressed, as far as the queue knows
  #buttons = new Map<Button, { x: number; y: number }>();
  #heldKeys = new Set<string>(); // what the page has actually been sent and not yet released
  #heldButtons = new Map<Button, { x: number; y: number }>();

  constructor(
    private readonly botId: string,
    private readonly call: BrowserCall,
    private readonly onChange: (view: BrowserView) => void,
  ) {}

  open(): void {
    const earlier = closes.get(this.botId);
    this.#push({
      run: async () => {
        await earlier;
        this.#apply(await this.call("browser.open", { botId: this.botId }));
        this.#poll(0);
      },
    });
  }

  // Frames are only pulled while the panel can be seen.
  setVisible(visible: boolean): void {
    this.#visible = visible;
    if (visible) this.#poll(0);
    else this.#stopTimer();
  }

  // Stops, after the queue drains: queued releases still go out, queued presses don't. Then, from the browser's own
  // word on where things stand (a take-control still in flight is not in the local view): a page dialog the user left
  // open is dismissed, held keys and buttons are released, and the bot gets the browser back. The browser stays open.
  close(): Promise<void> {
    if (this.#done) return this.#done;
    this.#stopTimer();
    this.#tasks = this.#tasks.filter((task) => task.release);
    let finish = (): void => {};
    const done = new Promise<void>((resolve) => (finish = resolve)).finally(() => {
      if (closes.get(this.botId) === done) closes.delete(this.botId);
    });
    this.#done = done;
    closes.set(this.botId, done);
    this.#push({
      release: true,
      run: async () => {
        try {
          await this.#handBack();
        } catch {
          // best effort: nothing here may surface
        } finally {
          finish();
        }
      },
    });
    this.#closing = true;
    return done;
  }

  async #handBack(): Promise<void> {
    let state: BrowserState | null = null;
    try {
      state = (await this.call("browser.frame", { botId: this.botId })).state;
    } catch {
      try {
        state = await this.call("browser.open", { botId: this.botId }); // no picture to take, so it can't fail the same way
      } catch {
        // the browser can't be asked
      }
    }
    // With no word from the browser, a Take control that was asked for (or seen) counts as having worked.
    const human = state ? state.control === "human" : this.#human || this.#asking > 0;
    if (!human) return;
    const pageId = state?.activePageId;
    if (state && pageId) {
      const { generation } = state;
      if (state.dialog) await this.call("browser.dialog", { botId: this.botId, generation, pageId, accept: false }).catch(() => {});
      for (const [button, at] of [...this.#heldButtons]) await this.#send(generation, pageId, { kind: "mouse", action: "up", x: at.x, y: at.y, button }).catch(() => {});
      for (const key of [...this.#heldKeys]) await this.#send(generation, pageId, { kind: "key", action: "up", key }).catch(() => {});
    }
    await this.call("browser.control", { botId: this.botId, control: "bot" }).catch(() => {});
  }

  releaseAll(): void {
    const state = this.view.state;
    const page = state?.activePageId;
    if (!state || !page) return;
    for (const [button, at] of this.#buttons) this.input({ kind: "mouse", action: "up", x: at.x, y: at.y, button });
    for (const key of [...this.#keys]) this.input({ kind: "key", action: "up", key });
  }

  // The generation and page are fixed when the user acts, so a page that changed meanwhile rejects the input.
  input(input: BrowserInput): void {
    const state = this.view.state;
    const page = state?.activePageId;
    if (!state || !page || state.control !== "human") return;
    const release = (input.kind === "key" || input.kind === "mouse") && input.action === "up";
    if (!this.view.frame && !release) return; // nothing of this page has been shown yet
    if (input.kind === "key") {
      if (input.action === "up" && !this.#keys.has(input.key)) return; // a key the page never saw go down (a paste shortcut)
      if (input.action === "down") this.#keys.add(input.key);
      else this.#keys.delete(input.key);
    }
    if (input.kind === "mouse") {
      if (input.button && input.action === "down") this.#buttons.set(input.button, { x: input.x, y: input.y });
      if (input.button && input.action === "up") this.#buttons.delete(input.button);
    }
    const task: Task = { move: input.kind === "mouse" && input.action === "move", release, run: () => this.#send(state.generation, page, input) };
    const last = this.#tasks[this.#tasks.length - 1];
    if (task.move && last?.move) this.#tasks[this.#tasks.length - 1] = task; // only the newest pointer position matters
    else this.#push(task);
  }

  setControl(control: BrowserState["control"]): void {
    if (control === "human") this.#asking += 1;
    this.#act(async () => {
      try {
        this.#apply(await this.call("browser.control", { botId: this.botId, control }));
      } finally {
        if (control === "human") this.#asking -= 1;
      }
    });
  }

  navigate(url: string): void {
    this.#reshape((generation, pageId) => this.call("browser.navigate", { botId: this.botId, generation, pageId, url }));
  }

  history(action: "back" | "forward" | "reload"): void {
    this.#reshape((generation, pageId) => this.call("browser.history", { botId: this.botId, generation, pageId, action }));
  }

  resize(width: number, height: number): void {
    this.#reshape((generation, pageId) => this.call("browser.resize", { botId: this.botId, generation, pageId, width, height }));
  }

  dialog(accept: boolean, promptText?: string): void {
    this.#onPage((generation, pageId) => this.call("browser.dialog", { botId: this.botId, generation, pageId, accept, promptText }));
  }

  tab(action: "new" | "select" | "close", pageId?: string): void {
    const generation = this.view.state?.generation;
    if (generation) this.#act(async () => this.#apply(await this.call("browser.tab", { botId: this.botId, generation, action, pageId })));
  }

  // What the user sees stops being what the page looks like the moment this is queued, so the picture goes now, and no
  // input is taken (or frame accepted) until the action is done and a new frame arrives.
  #reshape(run: (generation: string, pageId: string) => Promise<BrowserState>): void {
    const state = this.view.state;
    if (!state?.activePageId || this.#closing) return;
    this.#changing += 1;
    this.#set({ frame: null });
    const pageId = state.activePageId;
    this.#act(async () => {
      try {
        this.#apply(await run(state.generation, pageId));
      } finally {
        this.#changing -= 1;
      }
    });
  }

  #onPage(run: (generation: string, pageId: string) => Promise<BrowserState>): void {
    const state = this.view.state;
    if (state?.activePageId) this.#act(async () => this.#apply(await run(state.generation, state.activePageId!)));
  }

  async #send(generation: string, pageId: string, input: BrowserInput): Promise<void> {
    await this.call("browser.input", { botId: this.botId, generation, pageId, input });
    if (input.kind === "key") {
      if (input.action === "down") this.#heldKeys.add(input.key);
      else this.#heldKeys.delete(input.key);
    } else if (input.kind === "mouse" && input.button) {
      if (input.action === "down") this.#heldButtons.set(input.button, { x: input.x, y: input.y });
      else if (input.action === "up") this.#heldButtons.delete(input.button);
    }
  }

  // A failed action is shown; the queue and the polling go on.
  #act(run: () => Promise<void>): void {
    this.#push({ run });
  }

  #push(task: Task): void {
    if (this.#closing && !task.release) return;
    this.#tasks.push(task);
    void this.#pump();
  }

  async #pump(): Promise<void> {
    if (this.#running) return;
    this.#running = true;
    try {
      for (let task = this.#tasks.shift(); task; task = this.#tasks.shift()) {
        try {
          await task.run();
        } catch (error) {
          this.#set({ error: errorText(error) });
        }
      }
    } finally {
      this.#running = false;
    }
  }

  #poll(delay: number): void {
    if (this.#closing || !this.#visible || this.#framing || this.#timer !== undefined || !this.view.state) return;
    this.#timer = setTimeout(() => {
      this.#timer = undefined;
      if (this.#closing || !this.#visible) return;
      this.#framing = true;
      this.#push({
        run: async () => {
          let retry = POLL_MS;
          try {
            const { state, frame } = await this.call("browser.frame", { botId: this.botId });
            this.#set({ state, frame: this.#changing > 0 ? undefined : (frame ?? undefined), error: null });
          } catch (error) {
            retry = RETRY_MS;
            this.#set({ error: errorText(error) });
          } finally {
            this.#framing = false;
            this.#poll(retry);
          }
        },
      });
    }, delay);
  }

  #stopTimer(): void {
    clearTimeout(this.#timer);
    this.#timer = undefined;
  }

  #apply(state: BrowserState): void {
    this.#set({ state, error: null });
  }

  #set(patch: Partial<BrowserView>): void {
    if (patch.state) this.#human = patch.state.control === "human";
    if (this.#closing) return;
    // A frame belongs to one page of one generation: when either changes, the old picture goes until a new one arrives.
    const before = this.view.state;
    const moved = patch.state && (patch.state.generation !== before?.generation || patch.state.activePageId !== before.activePageId);
    const frame = patch.frame !== undefined ? patch.frame : moved ? null : this.view.frame;
    this.view = { ...this.view, ...patch, frame };
    this.onChange(this.view);
  }
}

// Where a pointer is on the page: its place on the shown frame, scaled to the frame's own pixels.
export function pagePoint(box: { left: number; top: number; width: number; height: number }, frame: { width: number; height: number }, clientX: number, clientY: number): { x: number; y: number } {
  const at = (offset: number, size: number, of: number): number => (size > 0 ? Math.min(Math.max(of - 1, 0), Math.max(0, Math.round((offset / size) * of))) : 0);
  return { x: at(clientX - box.left, box.width, frame.width), y: at(clientY - box.top, box.height, frame.height) };
}

export function mouseButton(button: number): Button | null {
  return button === 0 ? "left" : button === 1 ? "middle" : button === 2 ? "right" : null;
}

// Wheel deltas in pixels: lines and pages become pixels, and the daemon's limit holds.
export function wheelPixels(delta: number, mode: number): number {
  const pixels = mode === 1 ? delta * 16 : mode === 2 ? delta * 400 : delta;
  return Math.max(-10_000, Math.min(10_000, Math.round(pixels)));
}

// What the address bar means: a full address as typed, a host with https (http for this computer), anything else a search.
export function addressToUrl(text: string): string {
  const value = text.trim();
  if (/^(https?|about|file):/i.test(value)) return value;
  if (/^(localhost|127\.0\.0\.1|\[::1\])(:\d+)?([/?#]|$)/i.test(value)) return `http://${value}`;
  if (!/\s/.test(value) && /^[^\s/]+\.[^\s/]{2,}([/?#:]|$)/.test(value)) return `https://${value}`;
  return `https://duckduckgo.com/?q=${encodeURIComponent(value)}`;
}

// The size to ask the page for: the shown width, at the viewport's 4:3, within what the daemon accepts.
export function viewportSize(width: number): { width: number; height: number } {
  const w = Math.min(2560, Math.max(240, Math.round(width)));
  return { width: w, height: Math.min(1600, Math.max(180, Math.round((w * 3) / 4))) };
}

// What a key in the viewport does: "leave" is Shift+Tab, which returns focus to the panel's controls instead of the page;
// "paste" waits for the paste event, which carries the text; everything else goes to the page.
export function keyRoute(event: { key: string; shiftKey: boolean; ctrlKey: boolean; metaKey: boolean }): "leave" | "paste" | "send" {
  if (event.key === "Tab" && event.shiftKey) return "leave";
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "v") return "paste";
  return "send";
}
