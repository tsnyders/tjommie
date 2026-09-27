// Dev tool: chat with a bot through the running daemon before the desktop app exists.
//   npm run chat -w @tjommie/daemon -- "<bot name>" [claude|codex] [ask|folder|full]
// In the chat: /stop interrupts, /allow <id>, /always <id> and /deny <id> answer cards,
// /room <text> posts in the Team room, Ctrl+C quits.
import { readFileSync } from "node:fs";
import { createInterface } from "node:readline/promises";
import { Autonomy, Provider, TEAM_ROOM_ID, messageData, type ApprovalDecision } from "@tjommie/protocol";
import { ClientConnectionError, ClientRpcError, connect, type Client, type ClientEvent } from "@tjommie/client";
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
  console.error(missingToken
    ? "The terminal chat only works with a development daemon (npm start -w @tjommie/daemon)."
    : error instanceof Error ? error.message : String(error));
  process.exit(1);
}
void client.disconnected.then(disconnected);

const names = new Map<string, string>([["system", "Tjommie"]]);
let botId = "";

function showEvent(message: ClientEvent): void {
  if (message.event === "bot.updated") {
    names.set(message.data.id, message.data.name);
  } else if (message.event === "bot.deleted" && message.data.id === botId) {
    process.stdout.write("(This bot was deleted.)\n", () => process.exit(0));
  } else if (message.event === "message.delta") {
    if (message.data.conversationId === botId) process.stdout.write(message.data.delta);
  } else if (message.event === "message.added" || message.event === "message.updated") {
    const data = message.data;
    if (message.event === "message.updated" && data.conversationId === botId && data.kind === "text" && data.author === botId && data.status !== "streaming") {
      process.stdout.write("\n");
    } else if (data.status === "done" && data.conversationId === TEAM_ROOM_ID && data.author !== "you" && (data.kind === "text" || data.kind === "notice" || data.kind === "error")) {
      let to = "";
      try {
        const payload = messageData(data);
        if (payload.kind === "text" && payload.data !== null && "toBotId" in payload.data && payload.data.toBotId) {
          to = ` to ${names.get(payload.data.toBotId) ?? "a bot"}`;
        }
      } catch {
        // Keep the body readable if an older or malformed payload cannot be typed.
      }
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
    const botName = names.get(data.botId) ?? "A bot";
    if (data.botId === botId) {
      console.log(`  [${botName}] ${data.title}: ${data.body}`);
    } else {
      let title = data.title;
      if (title.startsWith(`${botName} is `)) title = title.slice(botName.length + 4);
      else if (title.startsWith(`${botName}'s `)) title = title.slice(botName.length + 3);
      else if (title.startsWith(`${botName} `)) title = title.slice(botName.length + 1);
      console.log(`  ${botName}: ${title.charAt(0).toUpperCase()}${title.slice(1)}: ${data.body}`);
    }
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
  const { messages } = await client.call("chat.history", { botId, limit: 10 }).catch(async (error: unknown) => {
    if (error instanceof ClientRpcError && error.code === "not_found") {
      await new Promise<void>((resolve) => process.stdout.write("(This bot was deleted.)\n", () => resolve()));
      process.exit(0);
    }
    throw error;
  });
  const streamingRows = messages.filter((row) => row.kind === "text" && row.author === botId && row.status === "streaming");
  const streamingIds = new Set(streamingRows.map((row) => row.id));
  console.log("Earlier:");
  for (const row of messages) {
    if (row.kind === "text") {
      if (streamingIds.has(row.id)) continue;
      if (row.body) console.log(row.author === "you" ? `  You: ${row.body}` : row.body);
    } else if (row.kind === "activity") {
      console.log(`  [${row.body}]`);
    } else if (row.kind === "approval") {
      try {
        const parsed = messageData(row);
        if (parsed.kind === "approval") {
          const { approvalId, status } = parsed.data;
          if (status === "pending") console.log(`\n  Waiting for you: ${row.body}\n  /allow ${approvalId}   /always ${approvalId}   /deny ${approvalId}`);
          else console.log(`  Approval ${approvalId} is ${status}: ${row.body}`);
          continue;
        }
      } catch {
        // Show the saved card body if its older payload cannot be typed.
      }
      console.log(`  (${row.body})`);
    } else {
      console.log(`  (${row.body})`);
    }
  }
  console.log("Now:");
  for (const row of streamingRows) if (row.body) process.stdout.write(row.body);
  const historyIds = new Set(messages.map((row) => row.id));
  // Replay events received during setup before subscribing to new ones.
  for (const event of client.events) {
    if (
      (event.event === "message.added" || event.event === "message.updated" || event.event === "message.delta") &&
      historyIds.has(event.data.id) &&
      !(event.event === "message.updated" && streamingIds.has(event.data.id))
    ) continue;
    if (event.event === "approval.added" && historyIds.has(event.data.messageId)) continue;
    showEvent(event);
  }
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
