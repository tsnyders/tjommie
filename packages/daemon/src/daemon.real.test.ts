import { existsSync } from "node:fs";
import { join } from "node:path";
import { Bot, Message, ProviderSetup, type BotInput } from "@tjommie/protocol";
import { afterAll, beforeAll, expect, it } from "vitest";
import { z } from "zod";
import { connect, type Client } from "./client.ts";
import { startDaemon, type Daemon } from "./daemon.ts";
import { realAccountsDir, tempRoot, waitFor } from "./testing.ts";

process.env.TJOMMIE_BROWSER_HEADLESS = "1";

let daemon: Daemon | undefined;
beforeAll(async () => {
  daemon = await startDaemon({ root: tempRoot(), port: 0, accountsDir: realAccountsDir() });
});
afterAll(async () => {
  await daemon?.close();
});

function runningDaemon(): Daemon {
  if (!daemon) throw new Error("The test daemon did not start.");
  return daemon;
}

async function withClient(test: (client: Client) => Promise<void>): Promise<void> {
  const daemon = runningDaemon();
  const client = await connect(daemon.port, daemon.token);
  try {
    await test(client);
  } finally {
    await client.close();
  }
}

// Waits for a finished text by this author in this conversation after the given event index.
const said = (client: Client, conversationId: string, author: string, word: string, from = 0) =>
  waitFor(() => {
    for (const event of client.events.slice(from)) {
      if (
        event.event === "message.updated" &&
        event.data.conversationId === conversationId &&
        event.data.author === author &&
        event.data.kind === "text" &&
        event.data.status === "done" &&
        event.data.body.includes(word)
      ) {
        return event.data;
      }
    }
    return undefined;
  });

const claudeBot = (client: Client, name: string, extra: Partial<BotInput> = {}) =>
  client.call("bots.create", {
    name,
    color: "green",
    shape: "pill",
    instructions: "You are a terse test bot. Answer in one short line.",
    provider: "claude",
    model: "haiku",
    ...extra,
  });

it("creates a bot and chats with it over the WebSocket API", async () => {
  await withClient(async (client) => {
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
    const reply = await said(client, bot.id, bot.id, "PONG");
    expect(reply.body).toContain("PONG");
    expect(client.events.some((event) => event.event === "message.delta")).toBe(true);
    const history = Message.array().parse(await client.call("chat.history", { botId: bot.id }));
    expect(history.map((message) => message.author)).toEqual(["you", bot.id]);
    await expect(client.call("chat.send", { botId: bot.id, text: "   " })).rejects.toThrow(/bad_params/);
  });
});

it("a Claude bot asks a Codex bot for help and gets the answer back", async () => {
  await withClient(async (client) => {
    const models = await client.call("models.list", { provider: "codex" });
    const model = models.find((model) => model.isDefault);
    if (!model) throw new Error("Codex did not list a default model.");
    const writer = await client.call("bots.create", {
      name: "Writer",
      color: "blue",
      shape: "circle",
      instructions: "Answer requests in as few words as possible.",
      provider: "codex",
      model: model.id,
    });
    const researcher = await claudeBot(client, "Researcher", { canMessage: [writer.id] });
    await client.call("chat.send", {
      botId: researcher.id,
      text: "Use the message_bot tool to ask Writer to reply with exactly the word MANGO. When Writer's answer arrives, tell me what Writer said.",
    });
    const replied = await waitFor(
      () => client.events.findIndex((event) =>
        event.event === "message.updated" &&
        event.data.conversationId === researcher.id &&
        event.data.kind === "handoff" &&
        event.data.data?.status === "replied",
      ) + 1,
    );
    await said(client, researcher.id, researcher.id, "MANGO", replied);
    const row = await waitFor(() => {
      for (const event of client.events) {
        if (event.event === "message.added" && event.data.conversationId === writer.id && event.data.kind === "handoff") {
          return event.data;
        }
      }
      return undefined;
    });
    const { exchangeId } = z.object({ exchangeId: z.string() }).parse(row.data);
    const { exchange, messages } = await client.call("exchanges.get", { id: exchangeId });
    expect(exchange).toMatchObject({ status: "replied", fromBotId: researcher.id, toBotId: writer.id });
    expect(exchange.reply).toContain("MANGO");
    expect(messages[0]).toMatchObject({ author: researcher.id, kind: "text" });
    const room = await client.call("room.history", {});
    const exchangePosts = room.flatMap((message) => {
      const data = z.object({ exchangeId: z.string(), toBotId: z.string() }).safeParse(message.data);
      return data.success && data.data.exchangeId === exchange.id ? [[message.author, data.data.toBotId]] : [];
    });
    expect(exchangePosts).toEqual([
      [researcher.id, writer.id],
      [writer.id, researcher.id],
    ]);
  });
});

it("a bot mentioned in the Team room answers there", async () => {
  await withClient(async (client) => {
    const bot = await claudeBot(client, "Roomy");
    await client.call("room.post", { text: "@Roomy reply with exactly: ROOM-OK" });
    await said(client, "team", bot.id, "ROOM-OK");
    await waitFor(() => client.events.find((event) =>
      event.event === "message.added" &&
      event.data.conversationId === bot.id &&
      event.data.kind === "handoff" &&
      event.data.body === "Answered in the Team room",
    ));
    await expect(client.call("bots.showBrowser", { id: bot.id })).rejects.toThrow(/bad_params/);
  });
});

it("a bot proposes a routine from chat, and approving the card creates it", async () => {
  await withClient(async (client) => {
    const bot = await claudeBot(client, "Planner");
    await client.call("chat.send", {
      botId: bot.id,
      text: "Use the add_routine tool to propose a routine named Morning check, on weekdays at 08:48, that checks the news. Then reply DONE.",
    });
    const card = await waitFor(() => {
      for (const event of client.events) {
        if (event.event === "approval.added" && event.data.botId === bot.id && event.data.kind === "routine") return event.data;
      }
      return undefined;
    });
    await client.call("approvals.respond", { approvalId: card.id, decision: "once" });
    const routines = await client.call("routines.list", { botId: bot.id });
    expect(routines).toMatchObject([
      { name: expect.stringMatching(/morning check/i), days: [1, 2, 3, 4, 5], times: ["08:48"], enabled: true },
    ]);
    // The later test advances the scheduler clock, so this demo must leave no enabled news routine.
    for (const routine of routines) await client.call("routines.update", { id: routine.id, patch: { enabled: false } });
    await said(client, bot.id, bot.id, "DONE");
  });
});

it("runs a routine when it's due, under a routine label", async () => {
  await withClient(async (client) => {
    const bot = await claudeBot(client, "Clockwork");
    const routine = await client.call("routines.create", {
      botId: bot.id,
      name: "Tick",
      prompt: "Reply with exactly: TICK",
      days: [0, 1, 2, 3, 4, 5, 6],
      times: ["03:00"],
    });
    if (routine.nextRunAt === null) throw new Error("The enabled routine has no next run.");
    runningDaemon().scheduler.tick(Date.parse(routine.nextRunAt));
    await said(client, bot.id, bot.id, "TICK");
    await waitFor(() => client.events.find((event) =>
      event.event === "routine.run" && event.data.routineId === routine.id && event.data.status === "done",
    ));
    const chat = await client.call("chat.history", { botId: bot.id });
    expect(chat[0]).toMatchObject({ kind: "routine", body: "Routine: Tick" });
  });
});

it("a bot with the browser on reads a page in its own browser profile", async () => {
  await withClient(async (client) => {
    const bot = await claudeBot(client, "Surfer", { browser: true });
    await client.call("chat.send", {
      botId: bot.id,
      text: "Use your browser tools to open https://example.com and reply with the page title only.",
    });
    await said(client, bot.id, bot.id, "Example Domain");
    expect(existsSync(join(bot.homeFolder, "chrome-profile", "Default"))).toBe(true);
  });
});
