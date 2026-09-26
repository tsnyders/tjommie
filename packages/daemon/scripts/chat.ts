// Dev tool: chat with a bot through the running daemon before the desktop app exists.
//   npm run chat -w @tjommie/daemon -- "<bot name>" [claude|codex] [ask|folder|full]
// In the chat: /stop interrupts, /allow <id>, /always <id> and /deny <id> answer cards,
// /room <text> posts in the Team room, Ctrl+C quits.
import { readFileSync } from "node:fs";
import { createInterface } from "node:readline/promises";
import { Autonomy, Provider, TEAM_ROOM_ID, type ApprovalDecision } from "@tjommie/protocol";
import { ClientConnectionError, connect, type Client, type ClientEvent } from "../src/client.ts";
import { DEFAULT_PORT, dataPaths } from "../src/paths.ts";

function describeClose(code: number, reason: string): string {
  const invalidServerFrame = code === 4400 && reason.startsWith("Invalid server frame");
  const explanation =
    code === 4401
      ? "The daemon rejected the token. Restart the chat after the daemon has started."
      : code === 4003
        ? "The daemon and chat client use different protocol versions. Restart them from the same Tjommie version."
        : code === 4400
          ? invalidServerFrame
            ? "The chat client could not read a message from the daemon."
            : "The daemon rejected a message from the chat client."
          : "The daemon closed the connection unexpectedly.";
  const reasonText = reason.trim() ? `${invalidServerFrame ? "Client" : "Daemon"} reason: ${reason.trim()}` : "The daemon did not provide a reason.";
  return `Connection closed with code ${code}. ${explanation} ${reasonText}`;
}

const [name = "Tester", providerArg = "claude", autonomyArg = "folder"] = process.argv.slice(2);
const provider = Provider.safeParse(providerArg);
const autonomy = Autonomy.safeParse(autonomyArg);
if (!provider.success || !autonomy.success) {
  console.error("Use a provider of claude or codex and an autonomy of ask, folder or full.");
  process.exit(1);
}
const port = Number(process.env.TJOMMIE_PORT ?? DEFAULT_PORT);
const connectionHelp = `Could not connect to the daemon on 127.0.0.1:${port}. Is it running? Start it with: npm start -w @tjommie/daemon`;

function disconnected(error: ClientConnectionError): never {
  if (!error.opened) {
    console.error(connectionHelp);
    process.exit(1);
  }
  if (error.code === 1000 || error.code === 1005) {
    console.log("Disconnected from the daemon.");
    process.exit(0);
  }
  console.error(describeClose(error.code, error.reason));
  process.exit(1);
}

let client: Client;
try {
  const token = readFileSync(dataPaths().token, "utf8").trim();
  client = await connect(port, token);
} catch (error) {
  if (error instanceof ClientConnectionError) disconnected(error);
  const missingToken = error instanceof Error && "code" in error && error.code === "ENOENT";
  console.error(missingToken ? connectionHelp : error instanceof Error ? error.message : String(error));
  process.exit(1);
}
void client.disconnected.then(disconnected);

const names = new Map<string, string>([["system", "Tjommie"]]);
let botId = "";

function showEvent(message: ClientEvent): void {
  if (message.event === "bot.updated") {
    names.set(message.data.id, message.data.name);
  } else if (message.event === "message.delta") {
    if (message.data.conversationId === botId) process.stdout.write(message.data.delta);
  } else if (message.event === "message.added" || message.event === "message.updated") {
    const data = message.data;
    if (message.event === "message.updated" && data.conversationId === botId && data.kind === "text" && data.author === botId && data.status !== "streaming") {
      process.stdout.write("\n");
    } else if (data.status === "done" && data.conversationId === TEAM_ROOM_ID && data.author !== "you" && (data.kind === "text" || data.kind === "notice" || data.kind === "error")) {
      const toBotId = data.data?.toBotId;
      const to = typeof toBotId === "string" && toBotId ? ` to ${names.get(toBotId) ?? "a bot"}` : "";
      console.log(`  [Team room] ${names.get(data.author) ?? "A bot"}${to}: ${data.body}`);
    } else if (message.event === "message.added" && data.conversationId === botId) {
      if (data.kind === "activity") console.log(`  [${data.body}]`);
      else if (["notice", "error", "handoff", "routine", "file"].includes(data.kind)) console.log(`  (${data.body})`);
    }
  } else if (message.event === "approval.added" && message.data.botId === botId) {
    const data = message.data;
    console.log(`\n  Waiting for you: ${data.summary}\n  /allow ${data.id}   /always ${data.id}   /deny ${data.id}`);
  } else if (message.event === "approval.resolved" && message.data.botId === botId) {
    const data = message.data;
    console.log(`  Approval ${data.id} is ${data.status}: ${data.summary}`);
  } else if (message.event === "notify") {
    const data = message.data;
    console.log(`  [${names.get(data.botId) ?? "A bot"}] ${data.title}: ${data.body}`);
  }
}

try {
  const bots = await client.call("bots.list", {});
  for (const bot of bots) names.set(bot.id, bot.name);
  let bot = bots.find((candidate) => candidate.name === name);
  if (!bot) {
    const models = await client.call("models.list", { provider: provider.data });
    const model = models.find((candidate) => candidate.isDefault) ?? models[0];
    if (!model) throw new Error(`No models are available for ${provider.data}. Sign in before creating a bot.`);
    bot = await client.call("bots.create", {
      name,
      color: "blue",
      shape: "circle",
      instructions: "",
      provider: provider.data,
      model: model.id,
      autonomy: autonomy.data,
    });
  }
  botId = bot.id;
  names.set(bot.id, bot.name);
  console.log(`Chatting with ${bot.name} (${bot.provider}, ${bot.autonomy}, ${bot.state}). /room <text> posts in the Team room, /stop interrupts, Ctrl+C quits.`);
  // Replay events received during setup before subscribing to new ones.
  for (const event of client.events) showEvent(event);
  client.subscribe(showEvent);
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exit(1);
}

const decisions: Readonly<Partial<Record<string, ApprovalDecision>>> = { "/allow": "once", "/always": "always", "/deny": "deny" };
for await (const line of createInterface({ input: process.stdin })) {
  const text = line.trim();
  const [command, arg] = text.split(/\s+/, 2);
  const decision = Object.hasOwn(decisions, command) ? decisions[command] : undefined;
  try {
    if (command === "/stop") await client.call("chat.stop", { botId });
    else if (command === "/room") {
      if (!arg) console.log("Usage: /room <message>");
      else await client.call("room.post", { text: text.slice("/room ".length) });
    } else if (decision) {
      if (!arg) console.log(`Usage: ${command} <approval id>`);
      else await client.call("approvals.respond", { approvalId: arg, decision });
    } else if (text) await client.call("chat.send", { botId, text: line });
  } catch (error) {
    console.log(`  (${error instanceof Error ? error.message : String(error)})`);
  }
}
await client.close();
