import { EventEmitter } from "node:events";
import { join } from "node:path";
import type { BrowserContext, Page } from "playwright-core";
import { describe, expect, it, vi } from "vitest";
import type { StoredBot } from "./bots.ts";
import { BrowserManager, browserProfile, findBrowser, jpegDimensions, navigationUrl } from "./browser-manager.ts";
import { browserServer, sharedBrowserServer } from "./browser.ts";

const env = { ProgramFiles: "C:\\Apps", "ProgramFiles(x86)": "C:\\Apps32", LOCALAPPDATA: "C:\\User" };
const bot = { id: "b1", homeFolder: "C:\\Bot", autonomy: "folder", browser: true } as StoredBot;

// A bounded SOF header is enough to exercise metadata parsing; real decoding is covered by the real fixture.
const jpeg = (label: string, width = 1000, height = 720): string => Buffer.concat([Buffer.from([255, 216, 255, 192, 0, 11, 8, height >> 8, height & 255, width >> 8, width & 255, 1, 1, 17, 0]), Buffer.from(label)]).toString("base64");

function fakeContext() {
  const pageEvents = new EventEmitter();
  const mainFrame = {};
  const keyboard = { down: vi.fn(async () => {}), up: vi.fn(async () => {}), insertText: vi.fn(async () => {}) };
  const mouse = { move: vi.fn(async () => {}), down: vi.fn(async () => {}), up: vi.fn(async () => {}), wheel: vi.fn(async () => {}) };
  const page = Object.assign(pageEvents, {
    url: () => "about:blank", title: async () => "Blank", mainFrame: () => mainFrame, keyboard, mouse,
    viewportSize: () => ({ width: 1000, height: 720 }), goto: vi.fn(async () => null), goBack: vi.fn(async () => null), goForward: vi.fn(async () => null), reload: vi.fn(async () => null), bringToFront: vi.fn(async () => {}),
    setViewportSize: vi.fn(async () => {}), close: vi.fn(async () => { pageEvents.emit("close"); }),
  }) as unknown as Page;
  const events = new EventEmitter();
  const close = vi.fn(async () => { events.emit("close"); });
  const cdp = () => Object.assign(new EventEmitter(), { send: vi.fn(async (method: string): Promise<unknown> => method === "Browser.getWindowForTarget" ? { windowId: 7 } : undefined), detach: vi.fn(async () => {}) });
  const observers: ReturnType<typeof cdp>[] = [];
  const context = Object.assign(events, { pages: () => [page], setDefaultTimeout: vi.fn(), setDefaultNavigationTimeout: vi.fn(), newCDPSession: vi.fn(async () => { const observer = cdp(); observers.push(observer); return observer; }), close }) as unknown as BrowserContext;
  return { context, page, pageEvents, keyboard, mouse, close, observers };
}

describe("owned embedded browser", () => {
  it("reads JPEG pixel dimensions and rejects malformed or truncated headers", () => {
    expect(jpegDimensions(jpeg("", 1280, 800))).toEqual({ width: 1280, height: 800 });
    for (const data of ["not-an-image", "", Buffer.from([255, 216, 255, 192, 0, 30, 8]).toString("base64")]) expect(jpegDimensions(data)).toBeNull();
  });

  it("reports downsampled pixels separately and omits unchanged image bytes by stream cursor", async () => {
    const fake = fakeContext();
    vi.spyOn(fake.page, "viewportSize").mockReturnValue({ width: 2560, height: 1600 });
    const manager = new BrowserManager(async () => fake.context);
    await manager.frame(bot);
    const stream = fake.observers.at(-1)!;
    stream.emit("Page.screencastFrame", { data: jpeg("large", 1280, 800), sessionId: 1, metadata: { deviceWidth: 2560, deviceHeight: 1600 } });
    const first = await manager.frame(bot);
    expect(first.frame).toMatchObject({ width: 1280, height: 800, viewportWidth: 2560, viewportHeight: 1600, pageId: first.state.activePageId, viewportRevision: first.state.viewportRevision });
    const cursor = { streamId: first.frame!.streamId!, sequence: first.frame!.sequence };
    expect(await manager.frame(bot, cursor)).toMatchObject({ frame: null, unchanged: true });
    expect((await manager.frame(bot, { ...cursor, streamId: "retired" })).frame).not.toBeNull();
    fake.observers[0].emit("Page.frameNavigated", { frame: { loaderId: "new-document" } });
    expect((await manager.frame(bot, cursor)).frame).toBeNull();
    stream.emit("Page.screencastFrame", { data: jpeg("late"), sessionId: 2, metadata: { deviceWidth: 2560, deviceHeight: 1600 } });
    expect((await manager.frame(bot)).frame).toBeNull();
    await manager.closeAll();
  });

  it("fences pointer coordinates from an obsolete layout without rejecting text, keys or releases", async () => {
    const fake = fakeContext();
    const manager = new BrowserManager(async () => fake.context);
    const state = await manager.control(bot, "human");
    await manager.input(bot, state.generation, state.activePageId!, { kind: "key", action: "down", key: "Shift" }, state.viewportRevision);
    await manager.resize(bot, state.generation, state.activePageId!, 900, 650);
    await expect(manager.input(bot, state.generation, state.activePageId!, { kind: "mouse", action: "down", x: 10, y: 20 }, state.viewportRevision)).rejects.toThrow(/view changed/);
    await expect(manager.input(bot, state.generation, state.activePageId!, { kind: "wheel", x: 10, y: 20, deltaX: 0, deltaY: 10 }, state.viewportRevision)).rejects.toThrow(/view changed/);
    await manager.input(bot, state.generation, state.activePageId!, { kind: "text", text: "still typing" }, state.viewportRevision);
    await manager.input(bot, state.generation, state.activePageId!, { kind: "key", action: "down", key: "a" }, state.viewportRevision);
    await manager.input(bot, state.generation, state.activePageId!, { kind: "key", action: "up", key: "Shift" }, state.viewportRevision);
    await manager.input(bot, state.generation, state.activePageId!, { kind: "mouse", action: "up", button: "left", x: 2000, y: 1000 }, state.viewportRevision);
    expect(fake.keyboard.insertText).toHaveBeenCalledWith("still typing");
    expect(fake.mouse.move).toHaveBeenLastCalledWith(999, 719);
    expect(fake.keyboard.up).toHaveBeenCalledWith("Shift");
    await manager.closeAll();
  });

  it("updates same-document URL state without replacing the stream or layout epoch", async () => {
    const fake = fakeContext();
    const manager = new BrowserManager(async () => fake.context);
    await manager.frame(bot);
    const stream = fake.observers.at(-1)!;
    stream.emit("Page.screencastFrame", { data: jpeg("SPA"), sessionId: 1, metadata: { deviceWidth: 1000, deviceHeight: 720 } });
    const before = await manager.frame(bot);
    for (let step = 0; step < 16; step++) {
      fake.pageEvents.emit("framenavigated", fake.page.mainFrame());
      const view = await manager.frame(bot);
      expect(view.frame?.streamId).toBe(before.frame?.streamId);
      expect(view.state.viewportRevision).toBe(before.state.viewportRevision);
    }
    expect(fake.observers).toHaveLength(2); // one page observer and one unchanged stream
    expect((await manager.state(bot)).revision).toBeGreaterThan(before.state.revision!);
    await manager.closeAll();
  });

  it("queues real key and pointer releases under an open modal without holding the dialog RPC", async () => {
    const fake = fakeContext();
    const manager = new BrowserManager(async () => fake.context);
    const state = await manager.control(bot, "human");
    await manager.input(bot, state.generation, state.activePageId!, { kind: "key", action: "down", key: "Shift" });
    await manager.input(bot, state.generation, state.activePageId!, { kind: "mouse", action: "down", button: "left", x: 10, y: 20 });
    fake.keyboard.up.mockClear(); fake.mouse.up.mockClear();
    const blocked = Promise.withResolvers<void>();
    const dismiss = vi.fn(async () => { blocked.resolve(); });
    fake.keyboard.up.mockImplementation(() => blocked.promise);
    fake.pageEvents.emit("dialog", { type: () => "confirm", message: () => "Continue?", defaultValue: () => "", dismiss });
    await manager.input(bot, state.generation, state.activePageId!, { kind: "key", action: "up", key: "Shift" });
    await manager.input(bot, state.generation, state.activePageId!, { kind: "mouse", action: "up", button: "left", x: 10, y: 20 });
    expect(fake.keyboard.up).toHaveBeenCalledWith("Shift");
    expect(fake.mouse.up).not.toHaveBeenCalled(); // remains ordered behind the pending key release
    await manager.dialog(bot, state.generation, state.activePageId!, false);
    expect(dismiss).toHaveBeenCalledOnce();
    await vi.waitFor(() => expect(fake.mouse.up).toHaveBeenCalledWith({ button: "left" }));
    await manager.closeAll();
  });

  it("reports takeover busy while automation drains and allows dialogs to be answered", async () => {
    const fake = fakeContext();
    const manager = new BrowserManager(async () => fake.context);
    await manager.open(bot);
    const running = Promise.withResolvers<void>();
    const entered = Promise.withResolvers<void>();
    const automation = manager.automation(bot, async () => { entered.resolve(); await running.promise; });
    await entered.promise;
    const takeover = await manager.control(bot, "human");
    expect(takeover.busy).toBe(true);
    expect((await manager.frame(bot)).state.busy).toBe(true);
    const dismiss = vi.fn(async () => {});
    fake.pageEvents.emit("dialog", { type: () => "confirm", message: () => "Continue?", defaultValue: () => "", dismiss });
    await manager.dialog(bot, takeover.generation, takeover.activePageId!, false);
    expect(dismiss).toHaveBeenCalledOnce();
    running.resolve(); await automation;
    await vi.waitFor(async () => expect((await manager.state(bot)).busy).toBe(false));
    expect((await manager.state(bot)).revision).toBeGreaterThan(takeover.revision!);
    await manager.closeAll();
  });

  it("finds Chrome first, checks all Edge roots, and reports no installed browser", () => {
    const chrome = join(env.LOCALAPPDATA, "Google", "Chrome", "Application", "chrome.exe");
    const edge = join(env.ProgramFiles, "Microsoft", "Edge", "Application", "msedge.exe");
    expect(findBrowser((path) => path === chrome || path === edge, env)).toEqual({ channel: "chrome", path: chrome });
    for (const root of Object.values(env)) {
      const path = join(root, "Microsoft", "Edge", "Application", "msedge.exe");
      expect(findBrowser((candidate) => candidate === path, env)).toEqual({ channel: "msedge", path });
    }
    expect(() => findBrowser(() => false, env)).toThrow(/Install Google Chrome or Microsoft Edge/);
  });

  it("uses an authenticated HTTP MCP descriptor and cannot start a headed legacy browser", () => {
    expect(sharedBrowserServer(bot, "http://127.0.0.1:123/mcp/browser", "test-token")).toEqual({
      name: "browser", url: "http://127.0.0.1:123/mcp/browser", headers: { Authorization: "Bearer test-token" }, autoAllow: true,
    });
    expect(sharedBrowserServer({ ...bot, autonomy: "ask" }, "url", "token").autoAllow).toBe(false);
    expect(() => browserServer(bot, {})).toThrow(/daemon's shared browser/);
    expect(browserProfile(bot)).toBe(join(bot.homeFolder, "chrome-profile"));
  });

  it.each(["file:///C:/Windows/win.ini", "javascript:alert(1)", "chrome://settings", "data:text/html,hello", "app://tjommie", "about:settings", "https://user:password@example.com"])("rejects unsafe user navigation %s", (url) => {
    expect(() => navigationUrl(url)).toThrow();
  });

  it("keeps one context per bot and separates different bots", async () => {
    const a = fakeContext();
    const b = fakeContext();
    const launch = vi.fn().mockResolvedValueOnce(a.context).mockResolvedValueOnce(b.context);
    const manager = new BrowserManager(launch);
    const [one, same] = await Promise.all([manager.open(bot), manager.open(bot)]);
    expect(one.generation).toBe(same.generation);
    expect(await manager.context(bot)).toBe(a.context);
    await manager.open({ ...bot, id: "b2" });
    expect(launch).toHaveBeenCalledTimes(2);
    await manager.close(bot.id);
    expect(a.close).toHaveBeenCalledOnce();
    expect(b.close).not.toHaveBeenCalled();
    await manager.closeAll();
    expect(b.close).toHaveBeenCalledOnce();
  });

  it("gates automation while a human controls the browser and rejects stale input", async () => {
    const fake = fakeContext();
    const manager = new BrowserManager(async () => fake.context);
    const state = await manager.open(bot);
    expect(state.control).toBe("bot");
    await manager.control(bot, "human");
    await expect(manager.automation(bot, async () => "work")).rejects.toThrow(/person is using/);
    await expect(manager.input(bot, "old", state.activePageId!, { kind: "text", text: "secret" })).rejects.toThrow(/restarted/);
    expect(fake.keyboard.insertText).not.toHaveBeenCalled();
    await manager.input(bot, state.generation, state.activePageId!, { kind: "text", text: "typed" });
    expect(fake.keyboard.insertText).toHaveBeenCalledWith("typed");
    await expect(manager.input(bot, state.generation, state.activePageId!, { kind: "mouse", action: "down", x: 1001, y: 2 })).rejects.toThrow(/outside/);
    await manager.control(bot, "bot");
    expect(await manager.automation(bot, async () => "work")).toBe("work");
    await expect(manager.input(bot, state.generation, state.activePageId!, { kind: "text", text: "secret" })).rejects.toThrow(/Take over/);
    await manager.closeAll();
  });

  it("waits for the owned browser to exit before close resolves", async () => {
    const fake = fakeContext();
    const exited = Promise.withResolvers<void>();
    fake.close.mockImplementation(() => exited.promise);
    const manager = new BrowserManager(async () => fake.context);
    await manager.open(bot);
    let complete = false;
    const closing = manager.close(bot.id).then(() => { complete = true; });
    await Promise.resolve();
    await Promise.resolve();
    expect(complete).toBe(false);
    await expect(manager.open(bot)).rejects.toThrow(/closing/);
    exited.resolve();
    await closing;
    expect(complete).toBe(true);
  });

  it("starts in bot control for either opener and for a frame that recreates a browser", async () => {
    for (const first of ["open", "automation"] as const) {
      const manager = new BrowserManager(async () => fakeContext().context);
      if (first === "open") await manager.open(bot);
      else await manager.automation(bot, async () => {});
      expect((await manager.open(bot)).control).toBe("bot");
      await manager.control(bot, "human");
      const before = await manager.state(bot);
      await manager.close(bot.id);
      const reopened = await manager.frame(bot);
      expect(reopened.state.control).toBe("bot");
      expect(reopened.state.generation).not.toBe(before.generation);
      await expect(manager.automation(bot, async () => "ready")).resolves.toBe("ready");
      await manager.closeAll();
    }
  });

  it.each(["navigate", "back", "forward", "reload"] as const)("discards old frames during and after %s", async (action) => {
    const fake = fakeContext();
    const manager = new BrowserManager(async () => fake.context);
    const state = await manager.control(bot, "human");
    const frame = (data: string) => ({ data: jpeg(data), sessionId: 1, metadata: { deviceWidth: 1000, deviceHeight: 720 } });
    await manager.frame(bot);
    const oldStream = fake.observers.at(-1)!;
    oldStream.emit("Page.screencastFrame", frame("old-page"));
    expect((await manager.frame(bot)).frame?.data).toBe(jpeg("old-page"));
    const committed = Promise.withResolvers<null>();
    const method = action === "navigate" ? "goto" : action === "back" ? "goBack" : action === "forward" ? "goForward" : "reload";
    vi.mocked(fake.page[method]).mockImplementation(() => committed.promise);
    const navigation = action === "navigate"
      ? manager.navigate(bot, state.generation, state.activePageId!, "https://example.com/new")
      : manager.history(bot, state.generation, state.activePageId!, action);
    await vi.waitFor(() => expect(fake.page[method]).toHaveBeenCalledOnce());
    expect((await manager.frame(bot)).frame).toBeNull();
    committed.resolve(null);
    await navigation;
    expect((await manager.frame(bot)).frame).toBeNull();
    oldStream.emit("Page.screencastFrame", frame("late-old-page"));
    expect((await manager.frame(bot)).frame).toBeNull();
    fake.observers.at(-1)!.emit("Page.screencastFrame", frame("new-page"));
    expect((await manager.frame(bot)).frame?.data).toBe(jpeg("new-page"));
    await manager.closeAll();
  });

  it("hides a physical capture that disagrees with the page's input viewport", async () => {
    const fake = fakeContext();
    const manager = new BrowserManager(async () => fake.context);
    await manager.frame(bot);
    const stream = fake.observers.at(-1)!;
    stream.emit("Page.screencastFrame", { data: jpeg("valid"), sessionId: 1, metadata: { deviceWidth: 1000, deviceHeight: 720 } });
    expect((await manager.frame(bot)).frame?.data).toBe(jpeg("valid"));
    stream.emit("Page.screencastFrame", { data: "wrong-surface", sessionId: 2, metadata: { deviceWidth: 984, deviceHeight: 625 } });
    expect((await manager.frame(bot)).frame).toBeNull();
    stream.emit("Page.screencastFrame", { data: jpeg("repaired"), sessionId: 3, metadata: { deviceWidth: 1000, deviceHeight: 720 } });
    expect((await manager.frame(bot)).frame?.data).toBe(jpeg("repaired"));
    await manager.closeAll();
  });

  it.each(["'Browser.setContentsSize' wasn't found", "Method not found", "Not supported"])("keeps the Playwright viewport when content sizing reports %s", async (reason) => {
    const fake = fakeContext();
    const manager = new BrowserManager(async () => fake.context);
    const state = await manager.control(bot, "human");
    const observer = fake.observers[0];
    observer.send.mockImplementation(async (method) => {
      if (method === "Browser.setContentsSize") throw new Error(`cdpSession.send: Protocol error (Browser.setContentsSize): ${reason}`);
      return method === "Browser.getWindowForTarget" ? { windowId: 7 } : undefined;
    });
    await expect(manager.resize(bot, state.generation, state.activePageId!, 528, 396)).resolves.toMatchObject({ control: "human" });
    expect(fake.page.setViewportSize).toHaveBeenCalledWith({ width: 528, height: 396 });
    expect(observer.send).toHaveBeenCalledWith("Browser.setContentsSize", { windowId: 7, width: 528, height: 396 });
    await fake.page.close();
    expect(observer.detach).toHaveBeenCalledOnce(); // the reused observer is detached at its page-lifetime boundary
    await manager.closeAll();
  });

  it.each(["Browser.setContentsSize", "Browser.getWindowForTarget"])("does not mask other %s failures", async (method) => {
    const fake = fakeContext();
    const manager = new BrowserManager(async () => fake.context);
    const state = await manager.control(bot, "human");
    const observer = fake.observers[0];
    const failure = new Error(`Protocol error (${method}): Invalid window id`);
    observer.send.mockImplementation(async (called) => {
      if (called === method) throw failure;
      return called === "Browser.getWindowForTarget" ? { windowId: 7 } : undefined;
    });
    await expect(manager.resize(bot, state.generation, state.activePageId!, 528, 396)).rejects.toBe(failure);
    await fake.page.close();
    expect(observer.detach).toHaveBeenCalledOnce();
    await manager.closeAll();
  });

  it("does not block deletion when a pending browser launch fails", async () => {
    const launching = Promise.withResolvers<BrowserContext>();
    const manager = new BrowserManager(() => launching.promise);
    const opened = expect(manager.open(bot)).rejects.toThrow("launch failed");
    const closing = manager.close(bot.id);
    launching.reject(new Error("launch failed"));
    await opened;
    await expect(closing).resolves.toBeUndefined();
  });

  it("waits for every browser close even if another one fails", async () => {
    const a = fakeContext();
    const b = fakeContext();
    const secondClosed = Promise.withResolvers<void>();
    a.close.mockRejectedValue(new Error("close failed"));
    b.close.mockImplementation(() => secondClosed.promise);
    const manager = new BrowserManager(async (value) => value.id === bot.id ? a.context : b.context);
    await manager.open(bot);
    await manager.open({ ...bot, id: "two" });
    let settled = false;
    const all = manager.closeAll().finally(() => { settled = true; });
    const rejected = expect(all).rejects.toMatchObject({ name: "AggregateError" });
    await vi.waitFor(() => expect(b.close).toHaveBeenCalledOnce());
    expect(settled).toBe(false);
    secondClosed.resolve();
    await rejected;
  });
});
