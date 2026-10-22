import type { BrowserState } from "@tjommie/protocol";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { addressToUrl, BrowserSession, keyRoute, mouseButton, pagePoint, viewportSize, wheelPixels, type BrowserCall, type BrowserView } from "./browser-session";

const state = (patch: Partial<BrowserState> = {}): BrowserState => ({
  botId: "b1",
  generation: "g1",
  control: "human",
  activePageId: "p1",
  tabs: [{ id: "p1", url: "https://example.com/", title: "Example" }],
  dialog: null,
  ...patch,
});
const frame = (sequence: number) => ({ data: "AAAA", mime: "image/jpeg" as const, width: 800, height: 600, sequence });

type Log = { method: string; params: Record<string, unknown> }[];
function fake(initial = state()) {
  const log: Log = [];
  let current = initial;
  let seq = 0;
  let inflight = 0;
  let overlapped = false;
  let fail = new Set<string>();
  let gate: Promise<void> | null = null;
  const call: BrowserCall = async (method, params) => {
    inflight += 1;
    overlapped ||= inflight > 1;
    log.push({ method, params: params as Record<string, unknown> });
    await Promise.resolve();
    inflight -= 1;
    if (fail.has(method)) throw new Error(`failed: ${method}`);
    if (method === "browser.input") {
      await gate;
      return {} as never;
    }
    if (method === "browser.frame") return { state: current, frame: frame(++seq) } as never;
    if (method === "browser.control") await gate;
    if (method === "browser.dialog") current = { ...current, dialog: null };
    if (method === "browser.control") current = { ...current, control: (params as { control: "human" | "bot" }).control };
    return current as never;
  };
  const views: BrowserView[] = [];
  const session = new BrowserSession("b1", call, (view) => views.push(view));
  return { session, log, views, set: (next: BrowserState) => (current = next), failOn: (...methods: string[]) => (fail = new Set(methods)), again: () => new BrowserSession("b1", call, (view) => views.push(view)), overlapped: () => overlapped, hold: () => new Promise<() => void>((open) => (gate = new Promise<void>((release) => open(() => ((gate = null), release()))))) };
}
const flush = () => vi.advanceTimersByTimeAsync(0);
const methods = (log: Log) => log.map((entry) => entry.method);

describe("BrowserSession", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("opens, then pulls frames one at a time about every 200 ms", async () => {
    const t = fake();
    t.session.open();
    await flush();
    expect(methods(t.log)).toEqual(["browser.open", "browser.frame"]);
    await vi.advanceTimersByTimeAsync(650);
    expect(methods(t.log).filter((m) => m === "browser.frame").length).toBe(4);
    expect(t.overlapped()).toBe(false);
    expect(t.session.view.frame?.sequence).toBe(4);
  });

  it("stops pulling while hidden and resumes when visible", async () => {
    const t = fake();
    t.session.open();
    await flush();
    t.session.setVisible(false);
    await vi.advanceTimersByTimeAsync(1000);
    const frames = t.log.filter((e) => e.method === "browser.frame").length;
    expect(frames).toBeLessThanOrEqual(2);
    await vi.advanceTimersByTimeAsync(1000);
    expect(t.log.filter((e) => e.method === "browser.frame").length).toBe(frames);
    t.session.setVisible(true);
    await flush();
    expect(t.log.filter((e) => e.method === "browser.frame").length).toBe(frames + 1);
  });

  it("keeps pulling after a failed pull, slower, and shows the problem", async () => {
    const t = fake();
    t.session.open();
    await flush();
    t.failOn("browser.frame");
    await vi.advanceTimersByTimeAsync(250);
    expect(t.session.view.error).toBe("failed: browser.frame".replace(/^[a-z_]+: /, ""));
    t.failOn();
    await vi.advanceTimersByTimeAsync(1100);
    expect(t.session.view.error).toBeNull();
  });

  it("sends input in order, with the generation and page from when the user acted", async () => {
    const t = fake();
    t.session.open();
    await flush();
    t.log.length = 0;
    t.session.input({ kind: "key", action: "down", key: "a" });
    t.session.input({ kind: "key", action: "up", key: "a" });
    t.set(state({ generation: "g2", activePageId: "p2" }));
    await vi.advanceTimersByTimeAsync(250);
    const inputs = t.log.filter((e) => e.method === "browser.input");
    expect(inputs.map((e) => (e.params.input as { action: string }).action)).toEqual(["down", "up"]);
    expect(inputs.every((e) => e.params.generation === "g1" && e.params.pageId === "p1")).toBe(true);
  });

  it("keeps only the newest of queued pointer moves", async () => {
    const t = fake();
    t.session.open();
    await flush();
    t.log.length = 0;
    t.session.input({ kind: "key", action: "down", key: "x" }); // in flight, so the moves queue behind it
    for (const x of [1, 2, 3]) t.session.input({ kind: "mouse", action: "move", x, y: 0 });
    await flush();
    const moves = t.log.filter((e) => (e.params.input as { kind?: string } | undefined)?.kind === "mouse");
    expect(moves).toHaveLength(1);
    expect((moves[0].params.input as { x: number }).x).toBe(3);
  });

  it("sends no input while the bot is in control", async () => {
    const t = fake(state({ control: "bot" }));
    t.session.open();
    await flush();
    t.log.length = 0;
    t.session.input({ kind: "text", text: "hi" });
    await flush();
    expect(methods(t.log)).not.toContain("browser.input");
  });

  it("releases held keys and buttons, and closing releases them too but leaves the browser open", async () => {
    const t = fake();
    t.session.open();
    await flush();
    t.session.input({ kind: "key", action: "down", key: "Shift" });
    t.session.input({ kind: "mouse", action: "down", x: 5, y: 6, button: "left" });
    await flush();
    t.log.length = 0;
    t.session.releaseAll();
    await flush();
    const sent = t.log.map((e) => e.params.input as { kind: string; action: string });
    expect(sent).toEqual([expect.objectContaining({ kind: "mouse", action: "up" }), expect.objectContaining({ kind: "key", action: "up" })]);

    t.session.input({ kind: "key", action: "down", key: "b" });
    await flush();
    t.log.length = 0;
    t.session.close();
    await vi.advanceTimersByTimeAsync(1000);
    expect(t.log.map((e) => e.method)).toEqual(["browser.frame", "browser.input", "browser.control"]);
  });

  it("releases a key whose press was still in flight when the panel closed", async () => {
    const t = fake();
    t.session.open();
    await flush();
    const open = await t.hold();
    t.session.input({ kind: "key", action: "down", key: "a" });
    await flush();
    t.session.close();
    open();
    await vi.advanceTimersByTimeAsync(1000);
    const sent = t.log.filter((e) => e.method === "browser.input").map((e) => (e.params.input as { action: string }).action);
    expect(sent).toEqual(["down", "up"]);
  });

  it("still sends a queued release at close, and drops a queued press", async () => {
    const t = fake();
    t.session.open();
    await flush();
    t.session.input({ kind: "key", action: "down", key: "a" });
    await flush();
    t.log.length = 0;
    const open = await t.hold();
    t.session.input({ kind: "key", action: "down", key: "b" }); // in flight
    t.session.input({ kind: "key", action: "up", key: "a" }); // queued release
    t.session.input({ kind: "key", action: "down", key: "c" }); // queued press
    await flush();
    t.session.close();
    open();
    await vi.advanceTimersByTimeAsync(1000);
    const sent = t.log.filter((e) => e.method === "browser.input").map((e) => (e.params.input as { action: string; key: string }));
    expect(sent.map((i) => `${i.action}:${i.key}`)).toEqual(["down:b", "up:a", "up:b"]);
  });

  it("hands the browser back to the bot when closed, and not when the bot already has it", async () => {
    const t = fake();
    t.session.open();
    await flush();
    t.log.length = 0;
    t.session.close();
    await vi.advanceTimersByTimeAsync(500);
    expect(t.log.at(-1)).toEqual({ method: "browser.control", params: { botId: "b1", control: "bot" } });

    const bot = fake(state({ control: "bot" }));
    bot.session.open();
    await flush();
    bot.log.length = 0;
    bot.session.close();
    await vi.advanceTimersByTimeAsync(500);
    expect(methods(bot.log)).not.toContain("browser.control");
  });

  it("does not leave the user in control when closed while Take control is still in flight", async () => {
    const t = fake(state({ control: "bot" }));
    t.session.open();
    await flush();
    const open = await t.hold();
    t.session.setControl("human");
    await flush();
    t.session.close(); // the reply to Take control arrives after this and is not shown
    open();
    await vi.advanceTimersByTimeAsync(500);
    const controls = t.log.filter((e) => e.method === "browser.control").map((e) => e.params.control);
    expect(controls).toEqual(["human", "bot"]);
    expect(t.session.view.state?.control).toBe("bot");
  });

  it("hands control back even when the browser can't be asked, if Take control was in flight", async () => {
    const t = fake(state({ control: "bot" }));
    t.session.open();
    await flush();
    const open = await t.hold();
    t.session.setControl("human");
    await flush();
    t.session.close();
    t.failOn("browser.frame", "browser.open");
    open();
    await vi.advanceTimersByTimeAsync(500);
    expect(t.log.filter((e) => e.method === "browser.control").map((e) => e.params.control)).toEqual(["human", "bot"]);
  });

  it("asks the browser to open when a fresh frame fails, and hands back what it says is human", async () => {
    const t = fake(state({ control: "human" }));
    t.session.open();
    await flush();
    t.log.length = 0;
    t.failOn("browser.frame");
    t.session.close();
    await vi.advanceTimersByTimeAsync(500);
    expect(methods(t.log)).toEqual(["browser.frame", "browser.open", "browser.control"]);
  });

  it("makes a quick reopen wait for the old panel's hand-back", async () => {
    const t = fake(state({ control: "human" }));
    t.session.open();
    await flush();
    t.log.length = 0;
    const open = await t.hold();
    t.session.input({ kind: "key", action: "down", key: "a" }); // keeps the old teardown waiting
    await flush();
    void t.session.close();
    const next = t.again();
    next.open();
    await vi.advanceTimersByTimeAsync(100);
    expect(methods(t.log)).toEqual(["browser.input"]);
    open();
    await vi.advanceTimersByTimeAsync(500);
    const order = methods(t.log);
    expect(order.indexOf("browser.control")).toBeGreaterThan(-1);
    expect(order.indexOf("browser.open")).toBeGreaterThan(order.indexOf("browser.control"));
    expect(t.log.find((e) => e.method === "browser.control")?.params.control).toBe("bot");
  });

  it("clears the picture when the page is resized or moved, and takes no input until a fresh frame", async () => {
    for (const act of [(t: ReturnType<typeof fake>) => t.session.resize(500, 375), (t: ReturnType<typeof fake>) => t.session.navigate("https://b.test/"), (t: ReturnType<typeof fake>) => t.session.history("back")]) {
      const t = fake();
      t.session.open();
      await flush();
      const old = t.session.view.frame?.sequence;
      act(t);
      expect(t.session.view.frame).toBeNull(); // at once, before the call even goes out
      t.log.length = 0;
      t.session.input({ kind: "text", text: "x" });
      await flush();
      expect(methods(t.log)).not.toContain("browser.input");
      await vi.advanceTimersByTimeAsync(500);
      expect(t.session.view.frame?.sequence).toBeGreaterThan(old ?? 0);
      t.session.input({ kind: "text", text: "x" });
      await flush();
      expect(methods(t.log)).toContain("browser.input");
    }
  });

  it("keeps the picture through a key press, a control change or a dialog answer", async () => {
    const t = fake();
    t.session.open();
    await flush();
    t.session.input({ kind: "key", action: "down", key: "a" });
    t.session.dialog(false);
    t.session.setControl("human");
    expect(t.session.view.frame).not.toBeNull();
  });

  it("dismisses a page dialog left open before handing the browser back", async () => {
    const dialog = { pageId: "p1", type: "confirm", message: "Sure?", defaultValue: "" };
    const t = fake(state({ dialog }));
    t.session.open();
    await flush();
    t.log.length = 0;
    t.session.close();
    await vi.advanceTimersByTimeAsync(500);
    expect(methods(t.log)).toEqual(["browser.frame", "browser.dialog", "browser.control"]);
    expect(t.log[1].params).toMatchObject({ accept: false, generation: "g1", pageId: "p1" });
  });

  it("ignores a key-up for a key it never saw go down", async () => {
    const t = fake();
    t.session.open();
    await flush();
    t.log.length = 0;
    t.session.input({ kind: "key", action: "up", key: "v" });
    await flush();
    expect(methods(t.log)).not.toContain("browser.input");
  });

  it("does nothing after close, even for a pull that was in flight", async () => {
    const t = fake();
    t.session.open();
    await flush();
    t.session.close();
    const seen = t.views.length;
    await vi.advanceTimersByTimeAsync(1000);
    expect(t.views.length).toBe(seen);
  });

  it("drops the old picture when the page or generation changes, and takes no input until a new one shows", async () => {
    const t = fake();
    t.session.open();
    await flush();
    expect(t.session.view.frame).not.toBeNull();
    t.session.tab("new");
    t.set(state({ activePageId: "p2" }));
    await flush();
    expect(t.session.view.frame).toBeNull();
    t.log.length = 0;
    t.session.input({ kind: "text", text: "x" });
    await flush();
    expect(methods(t.log)).not.toContain("browser.input");
  });

  it("switches control and applies the returned state", async () => {
    const t = fake();
    t.session.open();
    await flush();
    t.session.setControl("bot");
    await flush();
    expect(t.session.view.state?.control).toBe("bot");
    expect(t.log.find((e) => e.method === "browser.control")?.params).toEqual({ botId: "b1", control: "bot" });
  });

  it("sends navigation, history, tab, resize and dialog calls for the active page", async () => {
    const t = fake();
    t.session.open();
    await flush();
    t.log.length = 0;
    t.session.navigate("https://a.test/");
    t.session.history("back");
    t.session.tab("select", "p1");
    t.session.resize(800, 600);
    t.session.dialog(true, "x");
    await flush();
    const calls = t.log.filter((e) => e.method !== "browser.frame");
    expect(calls.map((e) => e.method)).toEqual(["browser.navigate", "browser.history", "browser.tab", "browser.resize", "browser.dialog"]);
    expect(calls[0].params).toMatchObject({ generation: "g1", pageId: "p1", url: "https://a.test/" });
  });
});

describe("browser helpers", () => {
  it("maps a pointer onto the page's own pixels, inside its edges", () => {
    const box = { left: 100, top: 50, width: 400, height: 300 };
    expect(pagePoint(box, { width: 800, height: 600 }, 300, 200)).toEqual({ x: 400, y: 300 });
    expect(pagePoint(box, { width: 800, height: 600 }, 0, 9999)).toEqual({ x: 0, y: 599 });
    expect(pagePoint({ ...box, width: 0 }, { width: 800, height: 600 }, 5, 5).x).toBe(0);
  });

  it("names mouse buttons, and ignores the side buttons", () => {
    expect([0, 1, 2, 3].map(mouseButton)).toEqual(["left", "middle", "right", null]);
  });

  it("turns wheel lines and pages into pixels within the daemon's limit", () => {
    expect(wheelPixels(3, 1)).toBe(48);
    expect(wheelPixels(1, 2)).toBe(400);
    expect(wheelPixels(-99999, 0)).toBe(-10_000);
  });

  it("reads the address bar as an address, a host, this computer, or a search", () => {
    expect(addressToUrl(" https://a.test/x ")).toBe("https://a.test/x");
    expect(addressToUrl("example.com/path")).toBe("https://example.com/path");
    expect(addressToUrl("localhost:3000/a")).toBe("http://localhost:3000/a");
    expect(addressToUrl("cheap flights")).toBe("https://duckduckgo.com/?q=cheap%20flights");
  });

  it("lets Shift+Tab leave the page, waits on paste, and sends other keys", () => {
    const plain = { shiftKey: false, ctrlKey: false, metaKey: false };
    expect(keyRoute({ ...plain, key: "Tab", shiftKey: true })).toBe("leave");
    expect(keyRoute({ ...plain, key: "Tab" })).toBe("send");
    expect(keyRoute({ ...plain, key: "V", ctrlKey: true })).toBe("paste");
    expect(keyRoute({ ...plain, key: "a" })).toBe("send");
  });

  it("keeps the requested viewport 4:3 and inside the daemon's limits", () => {
    expect(viewportSize(400)).toEqual({ width: 400, height: 300 });
    expect(viewportSize(100)).toEqual({ width: 240, height: 180 });
    expect(viewportSize(9000)).toEqual({ width: 2560, height: 1600 });
  });
});
