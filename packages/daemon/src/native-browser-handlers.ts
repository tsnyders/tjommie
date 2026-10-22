import { BrowserState } from "../../protocol/src/browser.ts";
import type { NativeBrowserCommand } from "../../protocol/src/native-browser.ts";
import { RpcError, type Handlers } from "./server.ts";

// Older app clients cannot accidentally start the retired headless browser when a native host is active.
export function nativeBrowserHandlers(request: (botId: string, command: NativeBrowserCommand) => Promise<unknown>): Handlers {
  const state = async (botId: string, command: NativeBrowserCommand) => BrowserState.parse(await request(botId, command));
  return {
    "browser.open": ({ botId }) => state(botId, { kind: "open" }),
    "browser.frame": async ({ botId }) => ({ state: await state(botId, { kind: "state" }), frame: null }),
    "browser.control": ({ botId, control }) => state(botId, { kind: "control", control }),
    "browser.navigate": ({ botId, generation, pageId, url }) => state(botId, { kind: "navigate", generation, pageId, url }),
    "browser.history": ({ botId, generation, pageId, action }) => state(botId, { kind: "history", generation, pageId, action }),
    "browser.tab": ({ botId, generation, pageId, action }) => state(botId, { kind: "tab", generation, pageId, action }),
    "browser.dialog": ({ botId, generation, pageId, accept, promptText }) => state(botId, { kind: "dialog", generation, pageId, accept, promptText }),
    "browser.resize": ({ botId }) => state(botId, { kind: "state" }),
    "browser.input": () => { throw new RpcError("bad_state", "Interact with the native browser directly in the desktop app."); },
  };
}
