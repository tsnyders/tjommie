import type { ApprovalData } from "@tjommie/protocol";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { ApprovalCard } from "./approval-card";

const proposal = "Add a routine: Morning digest, Weekdays at 6:30 AM, stop after 1 hour.";
const routine = (status: ApprovalData["status"]): ApprovalData => ({ approvalId: "a1", tool: "add_routine", detail: "Summarize the news.", status, kind: "routine" });

describe("ApprovalCard", () => {
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
