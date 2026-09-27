import type { Message } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { recentActivity } from "./activity";

type Step = { id: string; summary: string; status: "running" | "done" | "failed" };
const activity = (id: string, createdAt: string, steps: Step[]): Message => ({
  id,
  conversationId: "b1",
  author: "b1",
  kind: "activity",
  body: `${steps.length} steps`,
  data: { items: steps.map((step) => ({ ...step, name: "Tool" })) },
  status: "done",
  createdAt,
});
const text = (id: string): Message => ({ id, conversationId: "b1", author: "b1", kind: "text", body: "Hi", data: null, status: "done", createdAt: "2026-09-27T09:30:00.000Z" });

describe("recentActivity", () => {
  it("lists the newest steps first, each with its turn's time, up to the limit", () => {
    const messages = [
      activity("a1", "2026-09-27T09:15:00.000Z", [{ id: "s1", summary: "Searched pnet.co.za for Full Stack roles", status: "done" }]),
      text("t1"),
      activity("a2", "2026-09-27T09:31:00.000Z", [
        { id: "s2", summary: "Read listing PN-48213", status: "done" },
        { id: "s3", summary: "Filling the Apply form on pnet.co.za", status: "running" },
      ]),
    ];
    expect(recentActivity(messages).map((step) => [step.summary, step.status, step.at])).toEqual([
      ["Filling the Apply form on pnet.co.za", "running", "2026-09-27T09:31:00.000Z"],
      ["Read listing PN-48213", "done", "2026-09-27T09:31:00.000Z"],
      ["Searched pnet.co.za for Full Stack roles", "done", "2026-09-27T09:15:00.000Z"],
    ]);
    expect(recentActivity(messages, 2)).toHaveLength(2);
  });

  it("skips activity rows whose payload doesn't match", () => {
    expect(recentActivity([{ ...text("x"), kind: "activity", data: { steps: 3 } }])).toEqual([]);
  });
});
