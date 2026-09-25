import { Bot, Message, ProviderSetup, type Message as ChatMessage } from "@tjommie/protocol";
import { afterAll, beforeAll, expect, it } from "vitest";
import { startDaemon, type Daemon } from "./daemon.ts";
import { realAccountsDir, tempRoot, waitFor } from "./testing.ts";

type RpcResponse =
  | { type: "res"; id: number; ok: true; result: unknown }
  | { type: "res"; id: number; ok: false; error: { code: string; message: string } };
type EventMessage = { type: "event"; event: string; data: unknown };

let daemon: Daemon;
beforeAll(async () => {
  daemon = await startDaemon({ root: tempRoot(), port: 0, accountsDir: realAccountsDir() });
});
afterAll(() => daemon.close());

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function isRpcResponse(value: unknown): value is RpcResponse {
  if (!isRecord(value) || value.type !== "res" || typeof value.id !== "number" || typeof value.ok !== "boolean") return false;
  if (value.ok) return "result" in value;
  return isRecord(value.error) && typeof value.error.code === "string" && typeof value.error.message === "string";
}

function isEventMessage(value: unknown): value is EventMessage {
  return isRecord(value) && value.type === "event" && typeof value.event === "string" && "data" in value;
}

function isCompletedTextReply(event: EventMessage, botId: string): event is EventMessage & { data: ChatMessage } {
  const parsed = Message.safeParse(event.data);
  return (
    event.event === "message.updated" &&
    parsed.success &&
    parsed.data.author === botId &&
    parsed.data.kind === "text" &&
    parsed.data.status === "done"
  );
}

// A minimal client: Node 24's built-in WebSocket, speaking the protocol like the UI will.
async function connect(port: number, token: string) {
  const ws = new WebSocket(`ws://127.0.0.1:${port}`);
  const events: EventMessage[] = [];
  const pending = new Map<number, (message: RpcResponse) => void>();
  let nextId = 1;
  let welcomed: () => void = () => {};
  const welcome = new Promise<void>((resolve) => (welcomed = resolve));
  ws.addEventListener("message", (event) => {
    const message: unknown = JSON.parse(String(event.data));
    if (isRecord(message) && message.type === "welcome") welcomed();
    if (isEventMessage(message)) events.push(message);
    if (isRpcResponse(message)) {
      pending.get(message.id)?.(message);
      pending.delete(message.id);
    }
  });
  await new Promise((resolve, reject) => {
    ws.addEventListener("open", resolve);
    ws.addEventListener("error", reject);
  });
  ws.send(JSON.stringify({ type: "hello", protocol: 1, token }));
  await welcome;
  const call = (method: string, params: unknown): Promise<unknown> =>
    new Promise((resolve, reject) => {
      const id = nextId++;
      pending.set(id, (message) =>
        message.ok ? resolve(message.result) : reject(new Error(`${message.error.code}: ${message.error.message}`)),
      );
      ws.send(JSON.stringify({ type: "req", id, method, params }));
    });
  return { call, events, close: () => ws.close() };
}

it("creates a bot and chats with it over the WebSocket API", async () => {
  const client = await connect(daemon.port, daemon.token);
  const setup = ProviderSetup.array().parse(await client.call("setup.status", {}));
  expect(setup.map((status) => status.signedIn)).toEqual([true, true]);
  const bot = Bot.parse(
    await client.call("bots.create", {
      name: "Echo",
      color: "green",
      shape: "pill",
      instructions: "Answer in as few words as possible.",
      provider: "claude",
      model: "haiku",
    }),
  );
  expect(bot).toMatchObject({ name: "Echo", state: "idle", autonomy: "folder" });
  await client.call("chat.send", { botId: bot.id, text: "Reply with exactly: PONG" });
  const reply = await waitFor(() => client.events.find((event) => isCompletedTextReply(event, bot.id)));
  expect(reply.data.body).toContain("PONG");
  expect(client.events.some((event) => event.event === "message.delta")).toBe(true);
  const history = Message.array().parse(await client.call("chat.history", { botId: bot.id }));
  expect(history.map((message) => message.author)).toEqual(["you", bot.id]);
  await expect(client.call("chat.send", { botId: bot.id, text: "   " })).rejects.toThrow(/bad_params/);
  client.close();
});
