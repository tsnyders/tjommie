import type { ApprovalData, Bot } from "@tjommie/protocol";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { ApprovalCard } from "./approval-card";

const proposal = "Add a routine: Morning digest, Weekdays at 6:30 AM, stop after 1 hour.";
const routine = (status: ApprovalData["status"]): ApprovalData => ({ approvalId: "a1", tool: "add_routine", detail: "Summarize the news.", status, kind: "routine" });
const writer: Bot = {
  id: "b1",
  name: "Writer",
  color: "blue",
  shape: "circle",
  instructions: "",
  provider: "codex",
  model: "gpt-5",
  autonomy: "ask",
  homeFolder: "C:\\bots\\b1",
  projectFolder: null,
  browser: false,
  paused: false,
  canMessage: [],
  state: "waiting",
  setupReason: null,
  limitedUntil: null,
  createdAt: "2026-09-27T07:00:00.000Z",
};
const buttons = (html: string): string[] => [...html.matchAll(/<button[^>]*>([^<]*)<\/button>/g)].map((button) => button[1]);

describe("ApprovalCard", () => {
  it("offers Always allow only when the daemon has a rule to save", () => {
    const edit = renderToStaticMarkup(<ApprovalCard summary="Edit notes.txt" data={{ approvalId: "a3", tool: "fileChange", detail: "notes.txt", status: "pending", rule: null }} bot={writer} />);
    expect(buttons(edit)).toEqual(["Allow once", "Deny"]);
    expect(edit).toContain("Always allow isn&#x27;t available for this request yet.");
    expect(edit).not.toContain("Always allow saves");

    const command = renderToStaticMarkup(<ApprovalCard summary="Run npm test" data={{ approvalId: "a4", tool: "Bash", detail: "npm test", status: "pending", rule: "Bash(npm test:*)" }} bot={writer} />);
    expect(buttons(command)).toEqual(["Allow once", "Always allow", "Deny"]);
    expect(command).toContain("Always allow saves: Bash(npm test:*), for Writer only");
    expect(command).not.toContain("isn&#x27;t available");

    const proposed = renderToStaticMarkup(<ApprovalCard summary={proposal} data={routine("pending")} bot={writer} />);
    expect(buttons(proposed)).toEqual(["Add routine", "Dismiss"]);
    expect(proposed).not.toContain("Always allow");
  });

  it("names an answered routine card by the routine, without the proposal's own prefix", () => {
    expect(renderToStaticMarkup(<ApprovalCard summary={proposal} data={routine("allowed")} bot={undefined} />)).toContain(
      "Routine added: Morning digest, Weekdays at 6:30 AM, stop after 1 hour.",
    );
    expect(renderToStaticMarkup(<ApprovalCard summary={proposal} data={routine("denied")} bot={undefined} />)).toContain(
      "Dismissed: Morning digest, Weekdays at 6:30 AM, stop after 1 hour.",
    );
  });

  it("keeps a tool card's summary as it is", () => {
    const html = renderToStaticMarkup(<ApprovalCard summary="Write hello.txt" data={{ approvalId: "a2", tool: "Write", detail: "hello.txt", status: "denied", rule: null }} bot={undefined} />);
    expect(html).toContain("Denied: Write hello.txt");
  });
});
