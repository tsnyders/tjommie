import { describe, expect, it, vi } from "vitest";
import { nativeBrowserHandlers } from "./native-browser-handlers.ts";
import type { RequestContext } from "./request-context.ts";
import type { NativeBrowserCommand } from "../../protocol/src/native-browser.ts";

const state = { botId: "one", generation: "g", control: "bot", activePageId: "p", tabs: [{ id: "p", title: "Page", url: "https://example.com/" }], dialog: null };
const context = (abort = new AbortController()): RequestContext => ({ connectionId: "trusted-connection", principal: { kind: "device", deviceId: "device-one" }, signal: abort.signal });
describe("authenticated native browser handlers", () => {
  it("binds frame/input/navigation to the transport connection and forwards the current epochs", async () => {
    const request = vi.fn(async (_bot: string, command: NativeBrowserCommand) => command.kind === "remote" && command.action.kind === "frame" ? { state, frame: null } : state);
    const handlers = nativeBrowserHandlers(request);
    const ctx = context();
    expect(await handlers["browser.frame"]?.({ botId: "one" }, ctx)).toEqual({ state, frame: null });
    await handlers["browser.navigate"]?.({ botId: "one", generation: "g", pageId: "p", controlEpoch: 4, url: "https://example.com/next" }, ctx);
    expect(request).toHaveBeenLastCalledWith("one", { kind: "remote", controllerId: "rpc:trusted-connection", action: { kind: "navigate", generation: "g", pageId: "p", controlEpoch: 4, url: "https://example.com/next" } }, expect.any(Function));
    await handlers["browser.input"]?.({ botId: "one", generation: "g", pageId: "p", controlEpoch: 4, viewportRevision: 2, input: { kind: "text", text: "synthetic PIN" } }, ctx);
    expect(request).toHaveBeenLastCalledWith("one", expect.objectContaining({ kind: "remote", controllerId: "rpc:trusted-connection", action: expect.objectContaining({ kind: "input", controlEpoch: 4, viewportRevision: 2 }) }), expect.any(Function));
  });
  it("rejects malformed native state instead of passing it to old clients", async () => {
    const handlers = nativeBrowserHandlers(async () => ({ bad: "state" }));
    await expect(handlers["browser.open"]?.({ botId: "one" }, context())).rejects.toThrow();
  });
  it("rejects absent/internal principals and missing input epochs before host dispatch", async () => {
    const request = vi.fn(async () => state), handlers = nativeBrowserHandlers(request);
    await expect(handlers["browser.open"]?.({ botId: "one" })).rejects.toThrow("authenticated");
    expect(() => handlers["browser.navigate"]?.({ botId: "one", generation: "g", pageId: "p", url: "https://example.com/" }, context())).toThrow("epoch");
    await expect(handlers["browser.input"]?.({ botId: "one", generation: "g", pageId: "p", controlEpoch: 1, input: { kind: "text", text: "synthetic" } }, context())).rejects.toThrow("viewport");
    expect(request).not.toHaveBeenCalled();
  });
  it("releases urgently on revoke/disconnect and discards a late frame", async () => {
    let finish!: (value: unknown) => void;
    const abort = new AbortController(), request = vi.fn(async (_id: string, command: NativeBrowserCommand) => command.kind === "remote" && command.action.kind === "frame" ? new Promise<unknown>((resolve) => { finish = resolve; }) : {});
    const handlers = nativeBrowserHandlers(request), pending = handlers["browser.frame"]!({ botId: "one" }, context(abort));
    abort.abort();
    expect(request).toHaveBeenLastCalledWith("one", { kind: "remote", controllerId: "rpc:trusted-connection", action: { kind: "disconnect" } });
    finish({ state, frame: null });
    await expect(pending).rejects.toThrow("discarded");
  });
  it("suppresses unexpected private frames", async () => {
    const handlers = nativeBrowserHandlers(async () => ({ state: { ...state, privateActive: true }, frame: { data: "AAAA", mime: "image/jpeg", width: 640, height: 480, sequence: 1 } }));
    await expect(handlers["browser.frame"]?.({ botId: "one" }, context())).rejects.toThrow("Private");
  });
});
