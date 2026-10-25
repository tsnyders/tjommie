import type { Bot, Message, Routine } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { matches, nextRoutineLine, overviewLede, overviewSummary, phoneFilters, roomLine, talkingWith, taskLine } from "./overview";

const now = new Date(2026, 8, 27, 14, 0);
const bot = (state: Bot["state"], extra: Partial<Bot> = {}) => ({ id: "b1", name: "Pnet apply", state, limitedUntil: null, setupReason: null, paused: state === "paused", ...extra });
const message = (id: string, kind: Message["kind"], extra: Partial<Message> = {}): Message => ({
  id,
  conversationId: "b1",
  author: "b1",
  kind,
  body: "",
  data: null,
  status: "done",
  createdAt: new Date(2026, 8, 27, 9, 0).toISOString(),
  ...extra,
});

describe("the Agents overview", () => {
  it("sums up the bots and filters them", () => {
    expect(overviewSummary([bot("working"), bot("working"), bot("waiting"), bot("idle"), bot("idle"), bot("paused")])).toBe("6 bots, 2 working, 1 waiting for you");
    expect(overviewSummary([bot("idle")])).toBe("1 bot");
    expect(matches({ name: "CV Writer", state: "paused" }, "paused", "")).toBe(true);
    expect(matches({ name: "CV Writer", state: "paused" }, "working", "")).toBe(false);
    expect(matches({ name: "CV Writer", state: "idle" }, "all", "writ")).toBe(true);
    expect(matches({ name: "CV Writer", state: "idle" }, "all", "pnet")).toBe(false);
  });

  it("says what each bot is doing", () => {
    const messages = [
      message("m1", "text", { body: "Morning briefing sent at 7:00 AM.\nDetails below." }),
      // Claude's description of a Write is just the file name.
      message("m2", "approval", { body: "hello.txt", data: { approvalId: "a1", tool: "Write", detail: "{}", status: "pending", rule: "Write" } }),
      message("m3", "activity", {
        body: "2 steps",
        data: {
          items: [
            { id: "s1", name: "Read", summary: "Read listing PN-48213", status: "done" },
            { id: "s2", name: "Fill", summary: "Filling the Apply form", status: "running" },
          ],
        },
      }),
    ];
    expect(taskLine(bot("idle"), messages)).toBe("Morning briefing sent at 7:00 AM.");
    expect(taskLine(bot("waiting"), messages)).toBe("Write: hello.txt");
    const sentence = message("m4", "approval", { body: "Run npm test", data: { approvalId: "a2", tool: "Shell", detail: "npm test", status: "pending", rule: null } });
    expect(taskLine(bot("waiting"), [sentence])).toBe("Run npm test");
    expect(taskLine(bot("working"), messages)).toBe("Filling the Apply form");
    expect(taskLine(bot("paused"), messages)).toBe("Paused by you");
    expect(taskLine(bot("needs_setup", { setupReason: "Sign in to Claude" }), messages)).toBe("Sign in to Claude");
    expect(taskLine(bot("idle"), [])).toBe("No messages yet");
  });

  it("shows the next routine run", () => {
    const routine = (name: string, nextRunAt: string | null, extra: Partial<Routine> = {}): Routine => ({
      id: name,
      botId: "b1",
      name,
      prompt: "Check",
      days: [1],
      times: ["08:48"],
      timeZone: "Africa/Johannesburg",
      timeLimitMinutes: null,
      enabled: true,
      nextRunAt,
      createdAt: now.toISOString(),
      ...extra,
    });
    const routines = [
      routine("Later", new Date(2026, 8, 28, 8, 48).toISOString()),
      routine("Soon", new Date(2026, 8, 27, 17, 0).toISOString()),
      routine("Off", new Date(2026, 8, 27, 15, 0).toISOString(), { enabled: false }),
    ];
    expect(nextRoutineLine({ id: "b1", paused: false }, routines, now)).toBe("Next: Soon, 5:00 PM");
    expect(nextRoutineLine({ id: "b1", paused: false }, [routines[0]], now)).toBe("Next: Later, Tomorrow 8:48 AM");
    expect(nextRoutineLine({ id: "b1", paused: true }, routines, now)).toBe("Paused");
    expect(nextRoutineLine({ id: "b2", paused: false }, routines, now)).toBe("No routine");
  });

  it("finds today's partners and the room's last exchange", () => {
    const handoff = (id: string, from: string, to: string, at: Date) =>
      message(id, "handoff", { createdAt: at.toISOString(), data: { exchangeId: `x-${id}`, fromBotId: from, toBotId: to, messages: 2, status: "replied" } });
    const today = new Date(2026, 8, 27, 9, 31);
    expect(talkingWith("b1", [handoff("h1", "b1", "b2", today), handoff("h2", "b3", "b1", today), handoff("h3", "b1", "b4", new Date(2026, 8, 26, 9, 0))], now)).toEqual(["b2", "b3"]);
    const bots = [
      { id: "b1", name: "Pnet apply" },
      { id: "b2", name: "CV Writer" },
    ];
    const post = message("p1", "text", { conversationId: "team", author: "b2", body: "Done.", createdAt: new Date(2026, 8, 27, 13, 56).toISOString(), data: { exchangeId: "x1", toBotId: "b1" } });
    expect(roomLine([post], bots, now)).toBe("CV Writer and Pnet apply talked 4 minutes ago");
    expect(roomLine([], bots, now)).toBe("No bot chats yet");
  });

  it("gives each room the line from its own posts", () => {
    const bots = [
      { id: "b1", name: "Sales" },
      { id: "b2", name: "Marketing" },
      { id: "b3", name: "Chief" },
    ];
    const post = (id: string, roomId: string, from: string, to: string, at: Date) =>
      message(id, "text", { conversationId: roomId, author: from, body: "Done.", createdAt: at.toISOString(), data: { exchangeId: `x-${id}`, toBotId: to } });
    // Sales and Marketing are both in Growth, so their exchange is mirrored there; Chief's isn't.
    const everyone = [post("p1", "team", "b1", "b2", new Date(2026, 8, 27, 13, 0)), post("p2", "team", "b3", "b1", new Date(2026, 8, 27, 13, 50))];
    const growth = [post("p3", "r1", "b1", "b2", new Date(2026, 8, 27, 13, 0))];
    expect(roomLine(everyone, bots, now)).toBe("Chief and Sales talked 10 minutes ago");
    expect(roomLine(growth, bots, now)).toBe("Sales and Marketing talked 1 hour ago");
    // A room with only your posts and its bots' answers has no bot chats yet.
    const asked = [message("p4", "text", { conversationId: "r2", author: "you", body: "@Sales hi" }), message("p5", "text", { conversationId: "r2", author: "b1", body: "Hi." })];
    expect(roomLine(asked, bots, now)).toBe("No bot chats yet");
  });
});

describe("the phone's filter chips", () => {
  const of = (...states: Bot["state"][]): Pick<Bot, "state">[] => states.map((state) => ({ state }));
  const shown = (bots: Pick<Bot, "state">[], current: Parameters<typeof phoneFilters>[1] = "all"): string[] =>
    phoneFilters(bots, current).map((chip) => `${chip.label} ${chip.count}`);

  it("offers only the states some bot is in, with counts", () => {
    expect(shown(of("working", "waiting", "idle"))).toEqual(["All 3", "Working 1", "Waiting 1"]);
  });

  it("shows nothing when no chip would narrow the list", () => {
    expect(shown(of("working", "working"))).toEqual([]);
    expect(shown(of("idle"))).toEqual([]);
    expect(shown([])).toEqual([]);
  });

  it("keeps the picked chip, so a filter that now matches nobody can be left", () => {
    expect(shown(of("idle", "idle"), "paused")).toEqual(["All 2", "Paused 0"]);
  });
});

describe("Folio's lede", () => {
  it("names a lone bot and counts several", () => {
    expect(overviewLede([{ name: "Nova", state: "waiting" }, { name: "Ada", state: "working" }, { name: "Bo", state: "working" }])).toBe("2 bots are working; Nova is waiting for you.");
    expect(overviewLede([{ name: "Ada", state: "idle" }])).toBe("All quiet.");
    expect(overviewLede([])).toBe("No bots yet.");
  });
});
