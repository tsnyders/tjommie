import type { BrowserState } from "@tjommie/protocol";
import type { BrowserView } from "./browser-session";
import type { NativeBrowserApi, NativeBrowserCommand, NativeBrowserRect } from "./desktop";
import { errorText } from "./labels";

// A bot's close that hasn't finished handing the browser back. A reopened panel waits for it, so the old close can't take
// back control the new one just asked for.
const closes = new Map<string, Promise<void>>();

const MOUNT_RETRY_MS = 500; // after a failed mount, the next one waits this long
const MOUNT_RETRIES = 6; // and a failed mount is tried again by itself this many times, whatever else happens

type Task = { release?: boolean; run: () => Promise<void> };

// What the shared browser chrome (tabs, address bar, control strip) shows for a native browser state. There is no picture: the
// page is drawn by the desktop app itself, so the view's frame stays null.
export function nativeView(state: BrowserState | null, error: string | null): BrowserView {
  return { state, frame: null, error };
}

export const sameRect = (a: NativeBrowserRect | null, b: NativeBrowserRect | null): boolean => (a === null || b === null ? a === b : a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height);

// One bot's native browser as the panel sees it. Commands (open, control, navigate, tabs, the hand-back) go through one queue, so
// they reach the desktop app in the order the user acted. Where the page is shown (mount) has a lane of its own, so hiding it
// for a menu never waits behind a slow navigation. Closing hands the browser back to the bot and hides the view; the browser
// itself (its tabs and sign-ins) stays in the desktop app.
export class NativeBrowserSession {
  view: BrowserView = nativeView(null, null);
  hideError: string | null = null; // set if a close couldn't get the page hidden: the page may still cover the window
  #tasks: Task[] = [];
  #running = false;
  #closing = false;
  #done: Promise<void> | null = null;
  #off: (() => void)[] = [];
  #opened = false; // the desktop app has the bot's browser, so it can be mounted
  #rect: NativeBrowserRect | null = null; // where the view should be, as last asked
  #sent: NativeBrowserRect | null | undefined; // where it was last mounted successfully (undefined: never)
  #last: NativeBrowserRect = { x: 0, y: 0, width: 1, height: 1 }; // the last place it was shown, kept for hiding it
  #mounting: Promise<void> | null = null;
  #retryAt = 0;
  #retryTimer: ReturnType<typeof setTimeout> | undefined;
  #failures = 0;
  #events = 0; // state events seen, to tell a reply that an event overtook
  #latest: BrowserState | null = null; // the newest state an event brought
  #human = false; // the browser last said the user is in control
  #asking = 0; // Take control calls queued or in flight

  constructor(
    private readonly botId: string,
    private readonly api: NativeBrowserApi,
    private readonly onChange: (view: BrowserView) => void,
    private readonly onFocusReturn: () => void = () => {},
  ) {}

  // Opening again (Try again, or React running an effect, its cleanup and the effect once more) rebinds the listeners rather
  // than adding to them, and after a close waits for that close like any reopen.
  open(): void {
    this.#closing = false;
    this.#done = null;
    for (const off of this.#off) off();
    const earlier = closes.get(this.botId);
    this.#off = [
      this.api.onState((state) => {
        if (state.botId !== this.botId) return;
        this.#events += 1;
        this.#latest = state;
        this.#accept(state);
      }),
      this.api.onFocusReturn?.((botId) => {
        if (botId === this.botId && !this.#closing) this.onFocusReturn();
      }) ?? (() => {}),
    ];
    void this.#push({
      run: async () => {
        await earlier;
        await this.#run({ kind: "open" });
        this.#opened = true;
        void this.#syncMount();
      },
    });
  }

  // Where the page goes: the viewport's rectangle in the window, or null to hide it. It is sent on a lane of its own, only when it
  // differs from what the desktop app last accepted, and only the newest is sent.
  setBounds(rect: NativeBrowserRect | null): void {
    this.#rect = rect;
    if (this.#opened) void this.#syncMount();
  }

  // Taking control gives the user the page directly: they click and type in it, not through this panel.
  setControl(control: BrowserState["control"]): void {
    if (control === "human") this.#asking += 1;
    void this.#push({
      run: async () => {
        try {
          await this.#run({ kind: "control", control });
        } finally {
          if (control === "human") this.#asking -= 1;
        }
      },
    });
  }

  navigate(url: string): void {
    this.#onPage((generation, pageId) => ({ kind: "navigate", generation, pageId, url }));
  }

  history(action: "back" | "forward" | "reload"): void {
    this.#onPage((generation, pageId) => ({ kind: "history", generation, pageId, action }));
  }

  // Answers the dialog that is open, on the page and generation it belongs to (which may not be the page in front).
  dialog(accept: boolean, promptText?: string): void {
    const state = this.view.state;
    const open = state?.dialog;
    if (state && open) void this.#push({ run: () => this.#run({ kind: "dialog", generation: state.generation, pageId: open.pageId, accept, promptText }) });
  }

  tab(action: "new" | "select" | "close", pageId?: string): void {
    const generation = this.view.state?.generation;
    if (generation) void this.#push({ run: () => this.#run({ kind: "tab", generation, action, pageId }) });
  }

  // Leaving the Browser pane: a page dialog the user left open is dismissed and the bot gets its browser back. The browser
  // stays open, and the view stays where it was until its bounds change.
  suspend(): Promise<void> {
    return this.#push({ run: () => this.#handBack() });
  }

  // Terminal: hides the view at once, hands the browser back, and stops listening. Anything queued but not yet sent is dropped,
  // except a hand-back. The browser itself stays open for the bot.
  close(): Promise<void> {
    if (this.#done) return this.#done;
    this.#tasks = this.#tasks.filter((task) => task.release);
    let finish = (): void => {};
    const done = new Promise<void>((resolve) => (finish = resolve)).finally(() => {
      if (closes.get(this.botId) === done) closes.delete(this.botId);
    });
    this.#done = done;
    closes.set(this.botId, done);
    const off = this.#off; // this close's own listeners, not the ones a reopen makes
    clearTimeout(this.#retryTimer);
    this.#retryTimer = undefined;
    this.#closing = true;
    this.#rect = null;
    if (this.#opened) void this.#syncMount(); // hiding doesn't wait for the hand-back
    void this.#push({
      release: true,
      run: async () => {
        try {
          await this.#handBack().catch(() => {});
          await this.#hidden();
        } finally {
          for (const stop of off) stop();
          finish();
        }
      },
    });
    return done;
  }

  // Waits until the desktop app has accepted the hide, trying again a few times, and says so if it never did.
  async #hidden(): Promise<void> {
    if (!this.#opened) return;
    await this.#mounting;
    for (let tries = 0; this.#sent !== null && tries < MOUNT_RETRIES; tries += 1) {
      await new Promise((resolve) => setTimeout(resolve, MOUNT_RETRY_MS));
      await this.#syncMount();
    }
    this.hideError = this.#sent === null ? null : "The browser couldn't be hidden.";
  }

  // The mount lane: sends the wanted place until the desktop app has it. A failed mount is shown and tried again after a
  // pause (the next bounds update), not dropped as if it had worked.
  #syncMount(): Promise<void> {
    if (this.#mounting) return this.#mounting;
    const loop = async (): Promise<void> => {
      await Promise.resolve(); // so the lane is registered before any early return clears it
      try {
        while (this.#sent === undefined || !sameRect(this.#rect, this.#sent)) {
          if (!this.#closing && Date.now() < this.#retryAt) return;
          const want = this.#rect;
          if (want) this.#last = want;
          const seen = this.#events;
          try {
            this.#accept(await this.api.mount(this.botId, { visible: want !== null, bounds: want ?? this.#last }), seen, false);
            this.#sent = want;
            this.#failures = 0;
          } catch (error) {
            this.#failures += 1;
            this.#retryAt = Date.now() + MOUNT_RETRY_MS;
            this.#set(this.view.state, errorText(error));
            this.#retryLater();
            return;
          }
        }
      } finally {
        this.#mounting = null;
      }
    };
    this.#mounting = loop();
    return this.#mounting;
  }

  // A failed mount is tried again by itself, a bounded number of times, whether or not the viewport sends more bounds: a failed
  // hide must not leave the page covering the window because nothing asked again.
  #retryLater(): void {
    if (this.#retryTimer || this.#failures > MOUNT_RETRIES) return;
    this.#retryTimer = setTimeout(() => {
      this.#retryTimer = undefined;
      void this.#syncMount();
    }, MOUNT_RETRY_MS);
  }

  // From the desktop app's own word on where things stand, since a take-control still in flight is not in the local view. If it
  // can't be asked, a take-control that was asked for (or seen) counts as having worked, so the hand-back is still tried.
  async #handBack(): Promise<void> {
    let state: BrowserState | null = null;
    const seen = this.#events;
    try {
      state = await this.api.command(this.botId, { kind: "state" });
    } catch {
      // the browser can't be asked
    }
    if (state) this.#accept(state, seen);
    const human = state ? state.control === "human" : this.#human || this.#asking > 0;
    if (!human) return;
    if (state?.dialog) await this.api.command(this.botId, { kind: "dialog", generation: state.generation, pageId: state.dialog.pageId, accept: false }).catch(() => {});
    const asked = this.#events;
    this.#accept(await this.api.command(this.botId, { kind: "control", control: "bot" }), asked);
  }

  // Runs a command and shows its reply, unless an event overtook the reply (see #accept).
  async #run(command: NativeBrowserCommand): Promise<void> {
    const seen = this.#events;
    this.#accept(await this.api.command(this.botId, command), seen);
  }

  #onPage(command: (generation: string, pageId: string) => NativeBrowserCommand): void {
    const state = this.view.state; // the page the user saw when they acted
    const page = state?.activePageId;
    if (state && page) void this.#push({ run: () => this.#run(command(state.generation, page)) });
  }

  // A state is shown unless something newer has been seen. Every reply, of any lane, is fenced against the events that arrived while
  // it was in flight (`seen` is the event count when the call began): an event is the desktop app's newer word, so such a reply is
  // dropped, across generations too, unless it is plainly newer (the same generation, a higher revision). Within a generation a
  // lower revision is older than what is shown. A dropped reply without a revision asks for the state again (not mount replies,
  // which are incidental and frequent).
  #accept(state: BrowserState, seen?: number, refresh = true): void {
    const now = this.view.state;
    if (now && now.generation === state.generation && now.revision !== undefined && state.revision !== undefined && state.revision < now.revision) return;
    if (seen !== undefined && seen !== this.#events) {
      const latest = this.#latest;
      const newer = latest !== null && latest.generation === state.generation && latest.revision !== undefined && state.revision !== undefined && state.revision > latest.revision;
      if (!newer) {
        if (refresh && state.revision === undefined) void this.#push({ run: async () => this.#accept(await this.api.command(this.botId, { kind: "state" })) });
        return;
      }
    }
    this.#human = state.control === "human";
    this.#set(state, null);
  }

  // Resolves once the task has run; a failure is shown and the queue goes on.
  #push(task: Task): Promise<void> {
    if (this.#closing && !task.release) return Promise.resolve();
    return new Promise((resolve) => {
      const run = task.run;
      this.#tasks.push({
        ...task,
        run: async () => {
          try {
            await run();
          } catch (error) {
            this.#set(this.view.state, errorText(error));
          } finally {
            resolve();
          }
        },
      });
      void this.#pump();
    });
  }

  async #pump(): Promise<void> {
    if (this.#running) return;
    this.#running = true;
    try {
      for (let task = this.#tasks.shift(); task; task = this.#tasks.shift()) await task.run();
    } finally {
      this.#running = false;
    }
  }

  #set(state: BrowserState | null, error: string | null): void {
    if (this.#closing) return;
    this.view = nativeView(state, error);
    this.onChange(this.view);
  }
}
