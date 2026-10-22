import type { StoredBot } from "./bots.ts";
import { BrowserManager, browsers } from "./browser-manager.ts";
import { RpcError, type Handlers } from "./server.ts";

export function browserHandlers(getBot: (id: string) => StoredBot, manager: BrowserManager = browsers): Handlers {
  const bot = (id: string): StoredBot => {
    const value = getBot(id);
    if (!value.browser) throw new RpcError("bad_params", "This bot's browser is off.");
    return value;
  };
  return {
    "browser.open": ({ botId }) => manager.open(bot(botId)),
    "browser.frame": ({ botId, after }) => manager.frame(bot(botId), after),
    "browser.control": ({ botId, control }) => manager.control(bot(botId), control),
    "browser.navigate": ({ botId, generation, pageId, url }) => manager.navigate(bot(botId), generation, pageId, url),
    "browser.history": ({ botId, generation, pageId, action }) => manager.history(bot(botId), generation, pageId, action),
    "browser.tab": ({ botId, generation, pageId, action }) => manager.tab(bot(botId), generation, action, pageId),
    "browser.resize": ({ botId, generation, pageId, width, height }) => manager.resize(bot(botId), generation, pageId, width, height),
    "browser.input": async ({ botId, generation, pageId, input, viewportRevision }) => { await manager.input(bot(botId), generation, pageId, input, viewportRevision); return {}; },
    "browser.dialog": ({ botId, generation, pageId, accept, promptText }) => manager.dialog(bot(botId), generation, pageId, accept, promptText),
  };
}
