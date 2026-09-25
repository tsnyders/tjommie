import { describe, expect, it } from "vitest";
import { BotInput, BotPatch, ClientMessage, PROTOCOL_VERSION, methods } from "./index.ts";

const bot = { name: "Writer", color: "blue", shape: "circle", instructions: "", provider: "claude", model: "sonnet" } as const;

describe("protocol", () => {
  it("is version 1", () => {
    expect(PROTOCOL_VERSION).toBe(1);
  });

  it("parses hello and request messages and rejects anything else", () => {
    expect(ClientMessage.parse({ type: "hello", protocol: 1, token: "t" }).type).toBe("hello");
    expect(ClientMessage.parse({ type: "req", id: 1, method: "bots.list", params: {} }).type).toBe("req");
    expect(() => ClientMessage.parse({ type: "nope" })).toThrow();
  });

  it("trims bot names and rejects blank ones", () => {
    expect(BotInput.parse({ ...bot, name: "  Writer " }).name).toBe("Writer");
    expect(() => BotInput.parse({ ...bot, name: "   " })).toThrow();
  });

  it("leaves unspecified fields out of a patch so they stay unchanged", () => {
    expect(BotPatch.parse({ autonomy: "full" })).toEqual({ autonomy: "full" });
  });

  it("applies chat.send defaults and rejects blank text", () => {
    expect(methods["chat.send"].params.parse({ botId: "b", text: " hi " })).toEqual({ botId: "b", text: "hi", files: [] });
    expect(() => methods["chat.send"].params.parse({ botId: "b", text: "  " })).toThrow();
  });
});
