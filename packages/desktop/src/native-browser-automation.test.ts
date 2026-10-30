import { spawnSync } from "node:child_process";
import { EventEmitter } from "node:events";
import { realpathSync } from "node:fs";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import type { WebContents } from "electron";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { BrowserState } from "../../protocol/src/browser.ts";
import type { NativeBrowserEngine, NativeBrowserBot } from "./native-browser.ts";
import { cancelNativeBrowserAutomation, nativeBrowserAutomation, nativeBrowserTools, validateNativeBrowserTool, type NativeBrowserToolResult } from "./native-browser-automation.ts";

const roots: string[] = [];
afterEach(async () => { vi.useRealTimers(); await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true }))); });
const asRecord = (value: unknown): Record<string, unknown> => typeof value === "object" && value !== null ? value as Record<string, unknown> : {};
const words = (result: NativeBrowserToolResult): string => result.content.filter((item) => item.type === "text").map((item) => item.text).join("\n");
type Hook = (method: string, params: Record<string, unknown>) => unknown;
function fixture(hook: Hook = () => undefined) {
  const state: BrowserState = { botId: "one", generation: "generation", control: "bot", revision: 0, viewportRevision: 0, busy: false, activePageId: "page", tabs: [{ id: "page", url: "https://example.test", title: "Local fixture" }], dialog: null };
  const bot: NativeBrowserBot = { id: "one", homeFolder: "C:/synthetic/one" };
  const debuggerEvents = new EventEmitter(), events = new EventEmitter();
  const sendCommand = vi.fn(async (method: string, params: Record<string, unknown> = {}): Promise<unknown> => {
    const special = hook(method, params);
    if (special !== undefined) return special;
    if (method === "Accessibility.getFullAXTree") return { nodes: [
      { nodeId: "root", role: { value: "RootWebArea" }, childIds: ["button", "password"] },
      { nodeId: "button", role: { value: "button" }, name: { value: "Save" }, backendDOMNodeId: 12 },
      { nodeId: "password", role: { value: "textbox" }, name: { value: "Password" }, backendDOMNodeId: 13, value: { value: "NEVER-EXPOSE" }, properties: [{ name: "protected", value: { value: true } }] },
    ] };
    if (method === "DOM.resolveNode") return { object: { objectId: `object-${params.backendNodeId}` } };
    if (method === "DOM.getBoxModel") return { model: { content: [10, 20, 110, 20, 110, 60, 10, 60] } };
    if (method === "DOM.getNodeForLocation") return { backendNodeId: 12 };
    if (method === "Runtime.evaluate") return { result: { objectId: "object-12", value: params.expression === "window.devicePixelRatio || 1" ? 1 : true } };
    if (method === "Runtime.callFunctionOn") return { result: { value: String(params.functionDeclaration).includes("isConnected") ? { ok: true } : true } };
    if (method === "Page.getLayoutMetrics") return { cssVisualViewport: { pageX: 0, pageY: 0, clientWidth: 800, clientHeight: 600 }, cssContentSize: { width: 800, height: 1200 } };
    if (method === "Page.captureScreenshot") return { data: Buffer.from("synthetic-image").toString("base64") };
    return {};
  });
  const sendInputEvent = vi.fn<(value: unknown) => void>(), insertText = vi.fn(async (_text: string) => {});
  const contents = Object.assign(events, { debugger: Object.assign(debuggerEvents, { sendCommand }), sendInputEvent, insertText, getURL: () => state.tabs[0].url, getTitle: () => state.tabs[0].title, isLoadingMainFrame: () => false }) as unknown as WebContents;
  const act = vi.fn(async (_bot: NativeBrowserBot, command: unknown, actor: string) => { if (actor !== "bot" || state.control !== "bot") throw new Error("Human control"); if (asRecord(command).kind === "dialog") state.dialog = null; return state; });
  const withAutomation = vi.fn(async (_bot: NativeBrowserBot, _target: unknown, run: (contents: WebContents) => Promise<unknown>) => { if (state.control !== "bot") throw new Error("Human control"); return run(contents); });
  const resize = vi.fn(async () => state);
  const engine = { state: () => state, open: async () => state, act, withAutomation, resize } as unknown as NativeBrowserEngine;
  return { state, bot, engine, contents, debuggerEvents, events, sendCommand, sendInputEvent, insertText, act, withAutomation, resize, tool: (name: string, args: Record<string, unknown> = {}) => nativeBrowserAutomation(engine, bot, { kind: "tool", name, args }) };
}
async function temporary(): Promise<string> { const root = await mkdtemp(join(tmpdir(), "tjommie-native-tools-")); roots.push(root); return root; }

describe("native browser automation", () => {
  it("advertises native equivalents, rejects extra/invalid fields and never advertises Playwright Node execution", () => {
    for (const name of ["click", "type", "fill_form", "drag", "drop", "file_upload", "handle_dialog", "tabs", "snapshot", "take_screenshot", "evaluate", "emulate_media", "resize", "network_request", "find"]) expect(nativeBrowserTools[`browser_${name}`]).toBeDefined();
    expect(nativeBrowserTools.browser_run_code).toBeUndefined(); expect(nativeBrowserTools.browser_run_code_unsafe).toBeUndefined();
    expect(() => validateNativeBrowserTool("browser_click", { ref: "ref", shell: "bad" })).toThrow("Invalid arguments");
    expect(() => validateNativeBrowserTool("browser_wait_for", { time: 60 })).toThrow();
    expect(() => validateNativeBrowserTool("__proto__", {})).toThrow("Unsupported");
  });
  it("uses the actual guarded WebContents and produces stable refs without protected values", async () => {
    const f = fixture(), first = await f.tool("browser_snapshot"), second = await f.tool("browser_snapshot");
    expect(f.withAutomation).toHaveBeenCalledTimes(2); expect(words(first)).toContain("Save"); expect(words(first)).not.toContain("NEVER-EXPOSE");
    const ref = /ref=(n[0-9a-f]{8}_\d+)/.exec(words(first))![1]; expect(words(second)).toContain(ref);
    expect(await f.tool("browser_click", { ref })).not.toHaveProperty("isError", true);
    expect(f.sendInputEvent.mock.calls.map(([event]) => asRecord(event).type)).toEqual(["mouseMove", "mouseDown", "mouseUp"]);
    f.events.emit("did-navigate"); f.sendInputEvent.mockClear();
    expect(words(await f.tool("browser_click", { ref }))).toContain("stale"); expect(f.sendInputEvent).not.toHaveBeenCalled();
    expect(words(await fixture().tool("browser_click", { ref }))).toContain("another tab");
  });
  it("rejects human control before opening a debugger operation", async () => {
    const f = fixture(); f.state.control = "human";
    expect((await f.tool("browser_snapshot")).isError).toBe(true); expect(f.sendCommand).not.toHaveBeenCalled();
  });
  it.each(["control", "viewport"])("stops pending actions after %s changes, without sending input", async (change) => {
    let f: ReturnType<typeof fixture>;
    f = fixture((method) => { if (method === "DOM.getBoxModel") { if (change === "control") f.state.control = "human"; else f.state.viewportRevision = (f.state.viewportRevision ?? 0) + 1; } });
    const answer = await f.tool("browser_click", { target: "#save" });
    expect(answer.isError).toBe(true); expect(f.sendInputEvent).not.toHaveBeenCalled();
  });
  it("waits for actionability and refuses an overlay instead of clicking through", async () => {
    vi.useFakeTimers();
    const f = fixture((method, params) => method === "Runtime.callFunctionOn" && String(params.functionDeclaration).includes("function(hit)") ? { result: { value: false } } : undefined);
    const pending = f.tool("browser_click", { ref: "#covered" }); await vi.advanceTimersByTimeAsync(4000);
    expect(words(await pending)).toContain("covers"); expect(f.sendInputEvent).not.toHaveBeenCalled();
  });
  it("fills through native text input, clears empty text, and releases keys", async () => {
    const f = fixture(); await f.tool("browser_type", { target: "#field", text: "hello" }); expect(f.insertText).toHaveBeenCalledWith("hello");
    await f.tool("browser_type", { target: "#field", text: "" }); expect(f.sendInputEvent).toHaveBeenCalledWith(expect.objectContaining({ type: "keyDown", keyCode: "Backspace" }));
    await f.tool("browser_press_key", { key: "+" }); expect(f.sendInputEvent).toHaveBeenCalledWith(expect.objectContaining({ type: "char", keyCode: "+" }));
    f.sendInputEvent.mockImplementation((event) => { if (asRecord(event).type === "keyDown") throw new Error("Synthetic input failure"); });
    await f.tool("browser_press_key", { key: "Control+A" }); expect(f.sendInputEvent).toHaveBeenLastCalledWith(expect.objectContaining({ type: "keyUp", keyCode: "A" }));
  });
  it("routes navigation/tabs/dialog through bot actor without nesting engine queues", async () => {
    const f = fixture(); await f.tool("browser_tabs", { action: "select", index: 0 });
    expect(f.act).toHaveBeenCalledWith(f.bot, { kind: "tab", generation: "generation", action: "select", pageId: "page" }, "bot", expect.any(Function));
    f.state.dialog = { type: "alert", message: "test", pageId: "background", defaultValue: "" };
    await f.tool("browser_handle_dialog", { accept: false }); expect(f.withAutomation).not.toHaveBeenCalled();
    expect(f.act).toHaveBeenLastCalledWith(f.bot, expect.objectContaining({ kind: "dialog", pageId: "background" }), "bot", expect.any(Function));
    await f.tool("browser_navigate", { url: "https://fixture.test" }); expect(f.act).toHaveBeenLastCalledWith(f.bot, expect.objectContaining({ kind: "navigate", pageId: "page" }), "bot", expect.any(Function));
  });
  it("unblocks a pending evaluation on dialog without terminating the modal needed by handle_dialog", async () => {
    const f = fixture((method) => method === "Runtime.evaluate" ? new Promise<never>(() => {}) : undefined);
    const pending = f.tool("browser_evaluate", { function: "async () => new Promise(() => {})" });
    await vi.waitFor(() => expect(f.sendCommand).toHaveBeenCalledWith("Runtime.evaluate", expect.objectContaining({ awaitPromise: true, timeout: 10_000 })));
    f.debuggerEvents.emit("message", {}, "Page.javascriptDialogOpening", { type: "alert" });
    expect(words(await pending)).toContain("dialog"); expect(f.sendCommand.mock.calls.some(([method]) => method === "Runtime.terminateExecution")).toBe(false);
    expect(f.debuggerEvents.listenerCount("message")).toBe(1);
  });
  it("bounds hung evaluation and does not execute continuation after timeout", async () => {
    vi.useFakeTimers(); const f = fixture((method) => method === "Runtime.evaluate" ? new Promise<never>(() => {}) : undefined);
    const pending = f.tool("browser_evaluate", { function: "async () => new Promise(() => {})" }); await vi.advanceTimersByTimeAsync(10_100);
    expect(words(await pending)).toContain("timed out"); expect(f.sendCommand).toHaveBeenCalledWith("Runtime.terminateExecution", {}); expect(f.sendInputEvent).not.toHaveBeenCalled();
  });
  it("cancels an in-flight tool before its next input without closing the shared page", async () => {
    let resolveBox!: (value: unknown) => void;
    const f = fixture((method) => method === "DOM.getBoxModel" ? new Promise<unknown>((done) => { resolveBox = done; }) : undefined);
    const pending = f.tool("browser_click", { target: "#save" });
    await vi.waitFor(() => expect(resolveBox).toBeTypeOf("function"));
    await cancelNativeBrowserAutomation(f.bot.id); resolveBox({ model: { content: [0, 0, 100, 0, 100, 50, 0, 50] } });
    expect(words(await pending)).toContain("cancelled"); expect(f.sendInputEvent).not.toHaveBeenCalled();
    expect(f.sendCommand).toHaveBeenCalledWith("Runtime.terminateExecution", {});
    expect((await f.tool("browser_snapshot")).isError).not.toBe(true);
  });
  it("does not retain human-control traffic and redacts credential headers", async () => {
    const f = fixture(); await f.tool("browser_snapshot");
    f.state.control = "human"; f.debuggerEvents.emit("message", {}, "Runtime.consoleAPICalled", { type: "log", args: [{ value: "human secret" }] });
    f.debuggerEvents.emit("message", {}, "Network.requestWillBeSent", { requestId: "human", type: "XHR", request: { url: "https://secret.test", method: "GET" } });
    f.state.control = "bot"; f.debuggerEvents.emit("message", {}, "Network.requestWillBeSent", { requestId: "bot", type: "XHR", request: { url: "https://example.test/api", method: "GET", headers: { Authorization: "secret", Cookie: "secret", Accept: "json" }, postData: "never-retain" } });
    expect(words(await f.tool("browser_console_messages"))).not.toContain("human secret");
    const network = words(await f.tool("browser_network_request", { index: 1 })); expect(network).toContain("[redacted]"); expect(network).toContain("json"); expect(network).not.toContain("never-retain"); expect(network).not.toContain("secret");
    expect(words(await f.tool("browser_network_requests"))).not.toContain("secret.test");
  });
  it("bounds screenshots and restricts saved outputs to a new bot-folder file", async () => {
    const f = fixture(); f.bot.homeFolder = await temporary();
    const shot = await f.tool("browser_take_screenshot", { fullPage: true, type: "jpeg", filename: "page.jpg" });
    expect(shot.content.some((item) => item.type === "image" && item.mimeType === "image/jpeg")).toBe(true);
    expect(await readFile(join(f.bot.homeFolder, "browser-files", "page.jpg"), "utf8")).toBe("synthetic-image");
    expect((await f.tool("browser_take_screenshot", { filename: "../escape.png" })).isError).toBe(true);
    expect((await f.tool("browser_take_screenshot", { filename: "page.jpg" })).isError).toBe(true);
    const huge = fixture((method) => method === "Page.captureScreenshot" ? { data: Buffer.alloc(2 * 1024 * 1024 + 1).toString("base64") } : undefined);
    expect(words(await huge.tool("browser_take_screenshot"))).toContain("2 MiB");
  });
  it("uploads only real files inside bot folders to a pending bot chooser", async () => {
    const f = fixture(); f.bot.homeFolder = await temporary(); const outside = await temporary();
    const allowed = join(f.bot.homeFolder, "upload.txt"), denied = join(outside, "secret.txt"); await writeFile(allowed, "local"); await writeFile(denied, "outside");
    await f.tool("browser_snapshot"); f.debuggerEvents.emit("message", {}, "Page.fileChooserOpened", { backendNodeId: 12 });
    expect((await f.tool("browser_file_upload", { paths: [denied] })).isError).toBe(true); expect(f.sendCommand.mock.calls.some(([method]) => method === "DOM.setFileInputFiles")).toBe(false);
    expect((await f.tool("browser_file_upload", { paths: [allowed] })).isError).not.toBe(true);
    expect(f.sendCommand).toHaveBeenCalledWith("DOM.setFileInputFiles", { backendNodeId: 12, files: [realpathSync.native(allowed)] });
    expect(words(await f.tool("browser_file_upload", { paths: [allowed] }))).toContain("No bot-opened");
  });
  it.skipIf(process.platform !== "win32")("accepts an 8.3 upload alias and sends the canonical file path", async () => {
    const f = fixture(), root = await temporary(), file = join(root, "upload.txt");
    f.bot.homeFolder = root;
    await writeFile(file, "local");
    const short = spawnSync("cmd.exe", ["/d", "/c", "for %I in (\"" + file + "\") do @echo %~sI"], { encoding: "utf8", windowsHide: true, windowsVerbatimArguments: true });
    expect(short.status).toBe(0);
    const alias = short.stdout.trim();
    const canonical = realpathSync.native(file);
    expect(alias).toMatch(/~\d/);
    expect(realpathSync.native(alias)).toBe(canonical);
    await f.tool("browser_snapshot"); f.debuggerEvents.emit("message", {}, "Page.fileChooserOpened", { backendNodeId: 12 });
    expect((await f.tool("browser_file_upload", { paths: [alias] })).isError).not.toBe(true);
    expect(f.sendCommand).toHaveBeenCalledWith("DOM.setFileInputFiles", { backendNodeId: 12, files: [canonical] });
  });
  it("retains media overrides across calls and resets null values", async () => {
    const f = fixture(); await f.tool("browser_emulate_media", { colorScheme: "dark", reducedMotion: "reduce" }); await f.tool("browser_emulate_media", { colorScheme: null });
    expect(f.sendCommand).toHaveBeenCalledWith("Emulation.setEmulatedMedia", { media: "", features: [{ name: "prefers-reduced-motion", value: "reduce" }] });
    await f.tool("browser_resize", { width: 800, height: 600 }); expect(f.resize).toHaveBeenCalledWith(f.bot, 800, 600, expect.any(Function));
  });
  it("clears a pending bot chooser and restores native picking on human takeover", async () => {
    const f = fixture(); await f.tool("browser_snapshot");
    f.debuggerEvents.emit("message", {}, "Page.fileChooserOpened", { backendNodeId: 12 });
    f.events.emit("tjommie-browser-control", "human");
    expect(f.sendCommand).toHaveBeenCalledWith("Page.setInterceptFileChooserDialog", { enabled: false });
    expect(words(await f.tool("browser_file_upload"))).toContain("No bot-opened");
  });
  it("preserves an existing modal when cancelling an in-flight tool", async () => {
    const f = fixture((method) => method === "Runtime.evaluate" ? new Promise<never>(() => {}) : undefined);
    const pending = f.tool("browser_evaluate", { function: "() => new Promise(() => {})" });
    await vi.waitFor(() => expect(f.sendCommand.mock.calls.some(([method]) => method === "Runtime.evaluate")).toBe(true));
    f.state.dialog = { type: "confirm", pageId: "page", message: "human decision", defaultValue: "" };
    await cancelNativeBrowserAutomation(f.bot.id);
    expect(words(await pending)).toContain("cancelled");
    expect(f.sendCommand.mock.calls.some(([method]) => method === "Runtime.terminateExecution" || method === "Page.handleJavaScriptDialog")).toBe(false);
    expect(f.state.dialog?.message).toBe("human decision");
  });
  it("cancels queued navigation through the engine's execution check", async () => {
    const f = fixture(); let proceed!: () => void;
    f.act.mockImplementation(async (_bot, _command, _actor) => { await new Promise<void>((done) => { proceed = done; }); const check = (f.act.mock.calls[0] as unknown[])[3] as () => void; check(); return f.state; });
    const pending = f.tool("browser_navigate", { url: "https://example.test/next" });
    await vi.waitFor(() => expect(proceed).toBeTypeOf("function")); await cancelNativeBrowserAutomation(f.bot.id); proceed();
    expect(words(await pending)).toContain("cancelled"); expect(f.sendCommand).not.toHaveBeenCalled();
  });
  it("retries debugger initialization after cancellation instead of poisoning future tools", async () => {
    let attempts = 0;
    const f = fixture((method) => { if (method === "Runtime.enable" && ++attempts === 1) throw new Error("Synthetic setup failure"); });
    expect((await f.tool("browser_snapshot")).isError).toBe(true);
    expect(words(await f.tool("browser_snapshot"))).toContain("Save");
  });
  it("supports bounded form fields, native drag release, MIME drop and select operations", async () => {
    const f = fixture();
    expect((await f.tool("browser_fill_form", { fields: [{ name: "Name", type: "textbox", ref: "#name", value: "Synthetic" }, { name: "Size", type: "combobox", target: "#size", value: "Medium" }, { name: "Range", type: "slider", ref: "#range", value: "5" }] })).isError).not.toBe(true);
    expect(f.insertText).toHaveBeenCalledWith("Synthetic");
    expect((await f.tool("browser_select_option", { target: "#size", values: ["medium"] })).isError).not.toBe(true);
    expect((await f.tool("browser_drag", { startRef: "#from", endRef: "#to" })).isError).not.toBe(true);
    expect(f.sendInputEvent).toHaveBeenLastCalledWith(expect.objectContaining({ type: "mouseUp", button: "left" }));
    expect((await f.tool("browser_drop", { target: "#drop", data: { "text/plain": "local data" } })).isError).not.toBe(true);
    expect(f.sendCommand).toHaveBeenCalledWith("Input.dispatchDragEvent", expect.objectContaining({ type: "drop", data: { items: [{ mimeType: "text/plain", data: "local data" }], files: [], dragOperationsMask: 1 } }));
  });
  it("releases a held drag button if control changes mid-drag", async () => {
    const f = fixture();
    f.sendInputEvent.mockImplementation((event) => { if (asRecord(event).type === "mouseDown") f.state.control = "human"; });
    expect((await f.tool("browser_drag", { startRef: "#from", endRef: "#to" })).isError).toBe(true);
    const types = f.sendInputEvent.mock.calls.map(([event]) => asRecord(event).type);
    expect(types).toEqual(["mouseMove", "mouseDown", "mouseUp"]);
  });
});
