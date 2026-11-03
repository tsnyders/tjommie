import { BrowserFrame, BrowserState } from "../../protocol/src/browser.ts";
import type { NativeBrowserCommand, NativeBrowserRemoteAction } from "../../protocol/src/native-browser.ts";
import type { RequestContext } from "./request-context.ts";
import { RpcError, type Handlers } from "./server.ts";

type Request = (botId: string, command: NativeBrowserCommand, authorized?: () => boolean) => Promise<unknown>;

export function nativeBrowserHandlers(request: Request): Handlers {
  const lifetimes = new WeakMap<AbortSignal, Map<string, () => void>>();
  const frames = new Map<string, Promise<{ state: BrowserState; frame: BrowserFrame | null }>>();
  const send = async (botId: string, action: NativeBrowserRemoteAction, context?: RequestContext): Promise<unknown> => {
    if (!context || context.signal.aborted) throw new RpcError("bad_state", "An active authenticated browser connection is required.");
    const controllerId = `rpc:${context.connectionId}`;
    let used = lifetimes.get(context.signal);
    if (!used) { used = new Map(); lifetimes.set(context.signal, used); }
    if (!used.has(botId)) {
      const release = (): void => {
        used.delete(botId); context.signal.removeEventListener("abort", release);
        void request(botId, { kind: "remote", controllerId, action: { kind: "disconnect" } }).catch(() => {});
      };
      used.set(botId, release); context.signal.addEventListener("abort", release, { once: true });
    }
    const value = await request(botId, { kind: "remote", controllerId, action }, () => !context.signal.aborted);
    if (context.signal.aborted) throw new RpcError("bad_state", "This browser connection ended; the result was discarded.");
    return value;
  };
  const state = async (botId: string, action: NativeBrowserRemoteAction, context?: RequestContext): Promise<BrowserState> => BrowserState.parse(await send(botId, action, context));
  const epoch = (value: number | undefined): number => {
    if (value === undefined) throw new RpcError("bad_state", "Use the current browser control epoch.");
    return value;
  };
  return {
    "browser.open": ({ botId }, context) => state(botId, { kind: "state" }, context),
    "browser.frame": async ({ botId, thumbnail }, context) => {
      if (!context || context.signal.aborted) throw new RpcError("bad_state", "An active authenticated browser connection is required.");
      const key = `${context.connectionId}:${botId}:${thumbnail ? "thumbnail" : "frame"}`;
      const existing = frames.get(key); if (existing) return existing;
      const pending = send(botId, { kind: "frame", thumbnail }, context).then((value) => {
        if (!value || typeof value !== "object" || !("state" in value) || !("frame" in value)) throw new RpcError("bad_state", "The desktop returned an invalid browser frame.");
        const current = BrowserState.parse(value.state), frame = value.frame === null ? null : BrowserFrame.parse(value.frame);
        if (current.privateActive && frame) throw new RpcError("bad_state", "Private browser frames are suppressed.");
        if (frame && (frame.pageId !== current.activePageId || frame.viewportRevision !== current.viewportRevision || frame.controlEpoch !== current.controlEpoch || frame.privateEpoch !== current.privateEpoch)) throw new RpcError("bad_state", "The desktop returned a stale browser frame.");
        return { state: current, frame };
      }).finally(() => { if (frames.get(key) === pending) frames.delete(key); });
      frames.set(key, pending); return pending;
    },
    "browser.control": ({ botId, control, generation, controlEpoch, releaseOnly }, context) => state(botId, { kind: "control", control, generation, controlEpoch, releaseOnly }, context),
    "browser.navigate": ({ botId, controlEpoch, ...page }, context) => state(botId, { kind: "navigate", ...page, controlEpoch: epoch(controlEpoch) }, context),
    "browser.history": ({ botId, controlEpoch, ...page }, context) => state(botId, { kind: "history", ...page, controlEpoch: epoch(controlEpoch) }, context),
    "browser.tab": ({ botId, controlEpoch, ...page }, context) => state(botId, { kind: "tab", ...page, controlEpoch: epoch(controlEpoch) }, context),
    "browser.dialog": ({ botId, controlEpoch, ...page }, context) => state(botId, { kind: "dialog", ...page, controlEpoch: epoch(controlEpoch) }, context),
    "browser.resize": ({ botId, controlEpoch, ...page }, context) => state(botId, { kind: "resize", ...page, controlEpoch: epoch(controlEpoch) }, context),
    "browser.input": async ({ botId, controlEpoch, viewportRevision, ...page }, context) => {
      if (viewportRevision === undefined) throw new RpcError("bad_state", "Use the current browser viewport revision.");
      await send(botId, { kind: "input", ...page, viewportRevision, controlEpoch: epoch(controlEpoch) }, context); return {};
    },
  };
}
