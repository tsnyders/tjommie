import type { Bot, Message, Routine } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { matches, nextRoutineLine, overviewSummary, roomLine, talkingWith, taskLine } from "./overview";

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
      message("m2", "approval", { body: "Pnet apply wants to save CV.pdf", data: { approvalId: "a1", tool: "Write", detail: "{}", status: "pending", rule: "Write" } }),
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
    expect(taskLine(bot("waiting"), messages)).toBe("Pnet apply wants to save CV.pdf");
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
});
