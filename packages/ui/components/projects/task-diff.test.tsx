import { renderToStaticMarkup } from "react-dom/server";
import { expect, it } from "vitest";
import { TaskDiffView } from "./task-diff";

it("renders source as text and exposes dirty/truncated limitations accessibly", () => {
  const html = renderToStaticMarkup(<TaskDiffView diff={{ attemptId: "a", revision: 1, sourceSha: "a".repeat(40), baseSha: "b".repeat(40), text: "<script>bad()</script>", truncated: true, dirty: true }} />);
  expect(html).toContain("&lt;script&gt;"); expect(html).not.toContain("<script>"); expect(html).toContain('aria-label="Committed diff"'); expect(html).toContain("outside this commit"); expect(html).toContain("cannot claim complete coverage");
});
