import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { MarkdownText } from "./markdown";

describe("MarkdownText (markdown safety)", () => {
  it("formats Markdown", () => {
    const html = renderToStaticMarkup(<MarkdownText text={"**Done.** Two roles:\n\n- Sabenza IT\n- Takealot"} />);
    expect(html).toContain("<strong>Done.</strong>");
    expect(html).toContain("<li>Sabenza IT</li>");
  });

  it("never renders HTML from a reply, and drops script links", () => {
    const html = renderToStaticMarkup(<MarkdownText text={'<img src=x onerror="alert(1)"> [click](javascript:alert(1)) <script>alert(2)</script>'} />);
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<script");
    expect(html).not.toContain("javascript:");
  });

  it("opens links outside the app window", () => {
    const html = renderToStaticMarkup(<MarkdownText text="See https://pnet.co.za/jobs" />);
    expect(html).toContain('href="https://pnet.co.za/jobs"');
    expect(html).toContain('target="_blank"');
  });
});
