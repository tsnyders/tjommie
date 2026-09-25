// Dev tool: chat with a bot through the running daemon before the desktop app exists.
//   npm run chat -w @tjommie/daemon -- "<bot name>" [claude|codex] [ask|folder|full]
// In the chat: /stop interrupts, /allow <id>, /always <id> and /deny <id> answer approvals, Ctrl+C quits.
import { readFileSync } from "node:fs";
import { createInterface } from "node:readline/promises";
import { ApprovalDecision, Bot, ModelInfo, PROTOCOL_VERSION } from "@tjommie/protocol";
import { DEFAULT_PORT, dataPaths } from "../src/paths.ts";

type RpcResponse =
  | { type: "res"; id: number; ok: true; result: unknown }
  | { type: "res"; id: number; ok: false; error: { code: string; message: string } };
type EventMessage = { type: "event"; event: string; data: unknown };

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

const [name = "Tester", provider = "claude", autonomy = "folder"] = process.argv.slice(2);
const port = Number(process.env.TJOMMIE_PORT ?? DEFAULT_PORT);
const token = readFileSync(dataPaths().token, "utf8").trim();
const ws = new WebSocket(`ws://127.0.0.1:${port}`);
const pending = new Map<number, (message: RpcResponse) => void>();
let nextId = 1;
let botId = "";
let welcomed: () => void = () => {};
const welcome = new Promise<void>((resolve) => (welcomed = resolve));

const call = (method: string, params: unknown): Promise<unknown> =>
  new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, (message) =>
      message.ok ? resolve(message.result) : reject(new Error(`${message.error.code}: ${message.error.message}`)),
    );
    ws.send(JSON.stringify({ type: "req", id, method, params }));
  });

ws.addEventListener("message", (event) => {
  const message: unknown = JSON.parse(String(event.data));
  if (isRecord(message) && message.type === "welcome") return welcomed();
  if (isRpcResponse(message)) {
    pending.get(message.id)?.(message);
    pending.delete(message.id);
    return;
  }
  if (!isEventMessage(message) || !isRecord(message.data)) return;
  const data = message.data;
  if (
    message.event === "message.delta" &&
    data.conversationId === botId &&
    typeof data.delta === "string"
  ) {
    process.stdout.write(data.delta);
  } else if (
    message.event === "message.updated" &&
    data.conversationId === botId &&
    data.kind === "text" &&
    data.author === botId &&
    data.status !== "streaming"
  ) {
    process.stdout.write("\n");
  } else if (
    message.event === "message.added" &&
    data.conversationId === botId &&
    data.kind === "activity" &&
    typeof data.body === "string"
  ) {
    console.log(`  [${data.body}]`);
  } else if (
    message.event === "message.added" &&
    data.conversationId === botId &&
    (data.kind === "notice" || data.kind === "error") &&
    typeof data.body === "string"
  ) {
    console.log(`  (${data.body})`);
  } else if (
    message.event === "approval.added" &&
    data.botId === botId &&
    typeof data.summary === "string" &&
    typeof data.id === "string"
  ) {
    console.log(`\n  Waiting for you: ${data.summary}\n  /allow ${data.id}   /always ${data.id}   /deny ${data.id}`);
  }
});
ws.addEventListener("close", () => {
  console.log("Disconnected from the daemon.");
  process.exit(0);
});
await new Promise((resolve, reject) => {
  ws.addEventListener("open", resolve);
  ws.addEventListener("error", reject);
});
ws.send(JSON.stringify({ type: "hello", protocol: PROTOCOL_VERSION, token }));
await welcome;

const bots = Bot.array().parse(await call("bots.list", {}));
let bot = bots.find((candidate) => candidate.name === name);
if (!bot) {
  const models = ModelInfo.array().parse(await call("models.list", { provider }));
  const model = (models.find((candidate) => candidate.isDefault) ?? models[0]).id;
  bot = Bot.parse(
    await call("bots.create", {
      name,
      color: "blue",
      shape: "circle",
      instructions: "",
      provider,
      model,
      autonomy,
    }),
  );
}
botId = bot.id;
console.log(`Chatting with ${bot.name} (${bot.provider}, ${bot.autonomy}, ${bot.state}). /stop interrupts, Ctrl+C quits.`);

const decisions: Record<string, ApprovalDecision> = { "/allow": "once", "/always": "always", "/deny": "deny" };
for await (const line of createInterface({ input: process.stdin })) {
  const [command, arg] = line.trim().split(/\s+/, 2);
  try {
    if (command === "/stop") await call("chat.stop", { botId });
    else if (decisions[command] && arg) await call("approvals.respond", { approvalId: arg, decision: decisions[command] });
    else if (line.trim()) await call("chat.send", { botId, text: line });
  } catch (error) {
    console.log(`  (${error instanceof Error ? error.message : String(error)})`);
  }
}
