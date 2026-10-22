import type { BrowserState } from "@tjommie/protocol";
import { describe, expect, it, vi } from "vitest";
import type { NativeBrowserApi, NativeBrowserRect } from "./desktop";
import { NativeBrowserSession, nativeView, sameRect } from "./native-browser-session";

const state = (patch: Partial<BrowserState> = {}): BrowserState => ({
  botId: "b1",
  generation: "g1",
  control: "bot",
  activePageId: "p1",
  tabs: [{ id: "p1", url: "https://example.com/", title: "Example" }],
  dialog: null,
  ...patch,
});
const rect = (x: number): NativeBrowserRect => ({ x, y: 100, width: 400, height: 300 });

type Gate = "control" | "navigate" | "mount" | "state";

function fake(initial = state()) {
  let current = initial;
  const log: string[] = [];
  const gates = new Map<Gate, Promise<void>>();
  const failing = new Set<string>();
  const stateListeners = new Set<(state: BrowserState) => void>();
  const focusListeners = new Set<(botId: string) => void>();
  const note = (entry: string): void => void log.push(entry);
  const wait = async (gate: Gate): Promise<void> => void (await gates.get(gate));
  const api: NativeBrowserApi = {
    command: async (_bot, command) => {
      if (failing.has(command.kind)) throw new Error(`failed: ${command.kind}`);
      switch (command.kind) {
        case "control":
          await wait("control");
          note(`control:${command.control}`);
          return (current = { ...current, control: command.control });
        case "dialog":
          note(`dialog:${command.generation}:${command.pageId}:${command.accept}`);
          return (current = { ...current, dialog: null });
        case "navigate":
          await wait("navigate");
          note(`navigate:${command.generation}:${command.pageId}:${command.url}`);
          return current;
        case "history":
          note(`history:${command.generation}:${command.pageId}:${command.action}`);
          return current;
        case "tab":
          note(`tab:${command.generation}:${command.action}`);
          return current;
        case "state":
          await wait("state");
          note("state");
          return current;
        default:
          note(command.kind);
          return current;
      }
    },
    mount: async (_bot, at) => {
      await wait("mount");
      if (failing.has("mount")) throw new Error("failed: mount");
      note(at.visible ? `bounds:${at.bounds.x}` : "bounds:null");
      return current;
    },
    onState: (listener) => (stateListeners.add(listener), () => void stateListeners.delete(listener)),
    onFocusReturn: (listener) => (focusListeners.add(listener), () => void focusListeners.delete(listener)),
  };
  const returned = vi.fn();
  const session = new NativeBrowserSession("b1", api, () => {}, returned);
  return {
    session,
    log,
    returned,
    set: (next: BrowserState) => (current = next),
    push: (next: BrowserState) => stateListeners.forEach((listener) => listener(next)),
    pushFocus: (botId: string) => focusListeners.forEach((listener) => listener(botId)),
    listeners: () => stateListeners.size,
    fail: (...kinds: string[]) => (failing.clear(), kinds.forEach((kind) => failing.add(kind))),
    hold: (gate: Gate) => new Promise<() => void>((open) => gates.set(gate, new Promise<void>((release) => open(() => (gates.delete(gate), release()))))),
    again: () => new NativeBrowserSession("b1", api, () => {}),
  };
}
const settle = async (times = 4): Promise<void> => {
  for (let i = 0; i < times; i += 1) await new Promise((resolve) => setTimeout(resolve, 0));
};

describe("NativeBrowserSession", () => {
  it("opens the bot's browser and shows its state, with no picture", async () => {
    const t = fake();
    t.session.open();
    await settle();
    expect(t.log[0]).toBe("open");
    expect(t.session.view.state).toMatchObject({ botId: "b1", control: "bot", activePageId: "p1" });
    expect(t.session.view.frame).toBeNull();
  });

  it("follows the desktop app's state events for its own bot only", async () => {
    const t = fake();
    t.session.open();
    await settle();
    t.push(state({ control: "human" }));
    expect(t.session.view.state?.control).toBe("human");
    t.push(state({ botId: "b2", control: "bot" }));
    expect(t.session.view.state?.control).toBe("human");
  });

  it("rebinds its listeners when opened again, without leaking the old ones", async () => {
    const t = fake();
    t.session.open();
    t.session.open();
    t.session.open();
    await settle();
    expect(t.listeners()).toBe(1);
  });

  it("sends bounds only when they differ from what was accepted, and only the newest while one is in flight", async () => {
    const t = fake();
    t.session.open();
    await settle();
    t.log.length = 0;
    const open = await t.hold("mount");
    t.session.setBounds(rect(1));
    await settle(1); // now in flight, held
    t.session.setBounds(rect(2));
    t.session.setBounds(rect(3));
    t.session.setBounds(rect(3));
    open();
    await settle();
    expect(t.log).toEqual(["bounds:1", "bounds:3"]);
    t.session.setBounds(rect(3)); // idle, the same: nothing
    t.session.setBounds(rect(3));
    await settle();
    expect(t.log).toEqual(["bounds:1", "bounds:3"]);
  });

  it("tries a mount again after it fails, instead of treating the bounds as sent", async () => {
    vi.useFakeTimers();
    try {
      const t = fake();
      t.session.open();
      await vi.advanceTimersByTimeAsync(0);
      t.log.length = 0;
      t.fail("mount");
      t.session.setBounds(rect(7));
      await vi.advanceTimersByTimeAsync(0);
      expect(t.session.view.error).toContain("mount");
      t.fail();
      t.session.setBounds(rect(7)); // too soon: it waits out the pause
      await vi.advanceTimersByTimeAsync(0);
      expect(t.log).toEqual([]);
      await vi.advanceTimersByTimeAsync(600);
      t.session.setBounds(rect(7)); // the next frame's update retries the same bounds
      await vi.advanceTimersByTimeAsync(0);
      expect(t.log).toEqual(["bounds:7"]);
      expect(t.session.view.error).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("hides the view at once, not behind a slow navigation, and mounts the latest bounds when shown again", async () => {
    const t = fake();
    t.session.open();
    await settle();
    t.session.setBounds(rect(5));
    await settle();
    t.log.length = 0;
    const slow = await t.hold("navigate");
    t.session.navigate("https://slow.test/"); // in flight, held
    t.session.setBounds(null); // a menu opened
    await settle();
    expect(t.log).toEqual(["bounds:null"]); // hidden while the navigation is still pending
    t.session.setBounds(rect(8));
    t.session.setBounds(rect(9));
    await settle();
    expect(t.log).toEqual(["bounds:null", "bounds:9"]); // shown again at the latest, not at each step
    slow();
    await settle();
    expect(t.log.at(-1)).toBe("navigate:g1:p1:https://slow.test/");
  });

  it("takes and gives back control through the desktop app", async () => {
    const t = fake();
    t.session.open();
    await settle();
    t.log.length = 0;
    t.session.setControl("human");
    t.session.setControl("bot");
    await settle();
    expect(t.log).toEqual(["control:human", "control:bot"]);
  });

  it("acts on the generation and page the user saw, in the order they acted", async () => {
    const t = fake();
    t.session.open();
    await settle();
    t.log.length = 0;
    t.session.navigate("https://a.test/");
    t.session.history("back");
    t.session.tab("new");
    await settle();
    expect(t.log).toEqual(["navigate:g1:p1:https://a.test/", "history:g1:p1:back", "tab:g1:new"]);
  });

  it("answers the dialog on the page it belongs to, not the page in front", async () => {
    const t = fake(state({ control: "human", activePageId: "p1", tabs: [{ id: "p1", url: "a", title: "A" }, { id: "p2", url: "b", title: "B" }], dialog: { pageId: "p2", type: "confirm", message: "Sure?", defaultValue: "" } }));
    t.session.open();
    await settle();
    t.log.length = 0;
    t.session.dialog(true);
    await settle();
    expect(t.log).toEqual(["dialog:g1:p2:true"]);
    t.log.length = 0;
    t.session.dialog(true); // no dialog open any more: nothing to answer
    await settle();
    expect(t.log).toEqual([]);
  });

  it("never shows an older state than the one shown, within a generation", async () => {
    const t = fake();
    t.session.open();
    await settle();
    t.push(state({ revision: 5, control: "human" }));
    t.push(state({ revision: 4, control: "bot" }));
    expect(t.session.view.state).toMatchObject({ revision: 5, control: "human" });
    t.push(state({ generation: "g2", revision: 1, control: "bot" })); // a new generation starts its own count
    expect(t.session.view.state).toMatchObject({ generation: "g2", control: "bot" });
  });

  it("drops a reply that an event overtook (no revision), and reads the state again", async () => {
    const t = fake();
    t.session.open();
    await settle();
    t.log.length = 0;
    const open = await t.hold("navigate");
    t.session.navigate("https://a.test/");
    await settle();
    t.push(state({ control: "human" })); // the newer word, while the reply is in flight
    t.set(state({ control: "bot" })); // what the old reply will say
    open();
    await settle();
    expect(t.log).toContain("state"); // asked again instead of trusting the older reply
  });

  it("tells the panel when the page let go of the keyboard, for its own bot", async () => {
    const t = fake();
    t.session.open();
    await settle();
    t.pushFocus("b2");
    expect(t.returned).not.toHaveBeenCalled();
    t.pushFocus("b1");
    expect(t.returned).toHaveBeenCalledTimes(1);
  });

  it("suspends from the desktop app's own state: dismisses the dialog, then hands the browser back, keeping the session", async () => {
    const t = fake(state({ control: "human", dialog: { pageId: "p1", type: "confirm", message: "Sure?", defaultValue: "" } }));
    t.session.open();
    await settle();
    t.log.length = 0;
    await t.session.suspend();
    expect(t.log).toEqual(["state", "dialog:g1:p1:false", "control:bot"]);
    expect(t.session.view.state).toMatchObject({ control: "bot", dialog: null });
    expect(t.listeners()).toBe(1);
  });

  it("suspends a bot-controlled browser without touching it", async () => {
    const t = fake();
    t.session.open();
    await settle();
    t.log.length = 0;
    await t.session.suspend();
    expect(t.log).toEqual(["state"]);
  });

  it("still hands control back when the state can't be read but Take control was asked for", async () => {
    const t = fake();
    t.session.open();
    await settle();
    t.session.setControl("human");
    await settle();
    t.log.length = 0;
    t.fail("state");
    await t.session.suspend();
    expect(t.log).toEqual(["control:bot"]);
  });

  it("hands back a take-control that was still in flight when it closed, dropping queued actions, and hides the view", async () => {
    const t = fake();
    t.session.open();
    await settle();
    t.session.setBounds(rect(2));
    await settle();
    t.log.length = 0;
    const open = await t.hold("control");
    t.session.setControl("human"); // in flight
    t.session.navigate("https://dropped.test/");
    const closed = t.session.close();
    await settle();
    expect(t.log).toEqual(["bounds:null"]); // hidden at once, before the hand-back
    open();
    await closed;
    expect(t.log).toEqual(["bounds:null", "control:human", "state", "control:bot"]);
    expect(t.listeners()).toBe(0);
  });

  it("closes even when the hand-back fails, hiding the view", async () => {
    const t = fake(state({ control: "human" }));
    t.session.open();
    await settle();
    t.session.setBounds(rect(1));
    await settle();
    t.log.length = 0;
    t.fail("control");
    await t.session.close();
    expect(t.log).toContain("bounds:null");
    expect(t.listeners()).toBe(0);
  });

  it("makes a quick reopen wait for the old panel's hand-back", async () => {
    const t = fake();
    t.session.open();
    await settle();
    t.log.length = 0;
    const open = await t.hold("control");
    t.session.setControl("human"); // keeps the old close waiting
    void t.session.close();
    const next = t.again();
    next.open();
    await settle();
    expect(t.log).not.toContain("open");
    open();
    await settle(8);
    expect(t.log.indexOf("open")).toBeGreaterThan(t.log.indexOf("control:bot"));
  });
});

describe("fencing every reply against newer events", () => {
  it("drops a delayed mount reply from an older generation after an event of a newer one", async () => {
    const t = fake();
    t.session.open();
    await settle();
    const open = await t.hold("mount");
    t.session.setBounds(rect(1)); // the mount is in flight
    await settle(1);
    t.push(state({ generation: "g2", revision: 1, control: "human" })); // the newer word
    t.set(state({ generation: "g1", revision: 5 })); // what the delayed reply will say
    open();
    await settle();
    expect(t.session.view.state).toMatchObject({ generation: "g2", control: "human" });
  });

  it("doesn't let a mount reply without a revision overwrite a newer event, and doesn't ask again for it", async () => {
    const t = fake();
    t.session.open();
    await settle();
    const open = await t.hold("mount");
    t.session.setBounds(rect(1));
    await settle(1);
    t.push(state({ control: "human" }));
    t.set(state({ control: "bot" }));
    t.log.length = 0;
    open();
    await settle();
    expect(t.session.view.state?.control).toBe("human");
    expect(t.log).not.toContain("state");
  });

  it("still takes a reply that is plainly newer: the same generation, a higher revision", async () => {
    const t = fake();
    t.session.open();
    await settle();
    const open = await t.hold("mount");
    t.session.setBounds(rect(1));
    await settle(1);
    t.push(state({ revision: 3, control: "bot" }));
    t.set(state({ revision: 4, control: "human" }));
    open();
    await settle();
    expect(t.session.view.state).toMatchObject({ revision: 4, control: "human" });
  });
});

describe("a failed hide", () => {
  it("is tried again by itself, without the viewport asking again", async () => {
    vi.useFakeTimers();
    try {
      const t = fake();
      t.session.open();
      await vi.advanceTimersByTimeAsync(0);
      t.session.setBounds(rect(1));
      await vi.advanceTimersByTimeAsync(0);
      t.log.length = 0;
      t.fail("mount");
      t.session.setBounds(null); // the viewport asks once, then stops
      await vi.advanceTimersByTimeAsync(0);
      expect(t.session.view.error).toContain("mount");
      t.fail();
      await vi.advanceTimersByTimeAsync(600);
      expect(t.log).toEqual(["bounds:null"]);
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps a close waiting until the hide is accepted", async () => {
    vi.useFakeTimers();
    try {
      const t = fake();
      t.session.open();
      await vi.advanceTimersByTimeAsync(0);
      t.session.setBounds(rect(1));
      await vi.advanceTimersByTimeAsync(0);
      t.log.length = 0;
      t.fail("mount");
      let closed = false;
      void t.session.close().then(() => (closed = true));
      await vi.advanceTimersByTimeAsync(1200);
      expect(closed).toBe(false);
      t.fail();
      await vi.advanceTimersByTimeAsync(600);
      expect(closed).toBe(true);
      expect(t.log).toContain("bounds:null");
      expect(t.session.hideError).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("gives up after a bounded number of tries, and says the page may still be showing", async () => {
    vi.useFakeTimers();
    try {
      const t = fake();
      t.session.open();
      await vi.advanceTimersByTimeAsync(0);
      t.session.setBounds(rect(1));
      await vi.advanceTimersByTimeAsync(0);
      t.fail("mount");
      let closed = false;
      void t.session.close().then(() => (closed = true));
      await vi.advanceTimersByTimeAsync(20_000);
      expect(closed).toBe(true);
      expect(t.session.hideError).toContain("hidden");
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("reopening one session", () => {
  it("starts again after a close, waiting for it, and keeps its new listeners", async () => {
    const t = fake();
    t.session.open();
    await settle();
    void t.session.close();
    t.session.open();
    await settle(8);
    expect(t.listeners()).toBe(1);
    t.push(state({ control: "human" }));
    expect(t.session.view.state?.control).toBe("human");
  });
});

describe("native view helpers", () => {
  it("shows a state as the shared chrome expects it, with no frame", () => {
    const view = nativeView(state(), "oops");
    expect(view.frame).toBeNull();
    expect(view.error).toBe("oops");
    expect(view.state?.tabs).toHaveLength(1);
    expect(nativeView(null, null).state).toBeNull();
  });

  it("compares rectangles, with null as hidden", () => {
    expect(sameRect(rect(1), rect(1))).toBe(true);
    expect(sameRect(rect(1), rect(2))).toBe(false);
    expect(sameRect(null, null)).toBe(true);
    expect(sameRect(rect(1), null)).toBe(false);
  });
});
