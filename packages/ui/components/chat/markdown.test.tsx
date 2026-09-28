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
    const html = renderToStaticMarkup(
      <MarkdownText text={'<img src=x onerror="alert(1)"> ![x](https://example.com/a.png) [click](javascript:alert(1)) [page](data:text/html,hi) [box](vbscript:msgbox(1)) <script>alert(2)</script>'} />,
    );
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<script");
    expect(html).not.toContain("javascript:");
    expect(html).not.toContain("data:");
    expect(html).not.toContain("vbscript:");
  });

  it("opens links outside the app window", () => {
    const html = renderToStaticMarkup(<MarkdownText text="See https://pnet.co.za/jobs" />);
    expect(html).toContain('href="https://pnet.co.za/jobs"');
    expect(html).toContain('target="_blank"');
  });

  it("shows a web image as a link to it, and anything that can't open as plain text", () => {
    const html = renderToStaticMarkup(
      <MarkdownText text="![Listing](https://example.com/a.png) ![](https://example.com/b.png) ![Saved](shot.png) [![Badge](https://example.com/c.svg)](https://github.com/x) [Run it](javascript:alert(1)) [Email me](mailto:you@example.com)" />,
    );
    expect(html).toBe(
      '<div class="markdown"><p><a href="https://example.com/a.png" target="_blank" rel="noreferrer">Listing</a> <a href="https://example.com/b.png" target="_blank" rel="noreferrer">https://example.com/b.png</a> Saved <a href="https://github.com/x" target="_blank" rel="noreferrer">Badge</a> Run it Email me</p></div>',
    );
  });

  it("never leaves a link without text when its image has no alt text", () => {
    const html = renderToStaticMarkup(<MarkdownText text="[![](https://example.com/badge.svg)](https://github.com/x) [![](badge.png)](https://github.com/y)" />);
    expect(html).toBe(
      '<div class="markdown"><p><a href="https://github.com/x" target="_blank" rel="noreferrer">https://example.com/badge.svg</a> <a href="https://github.com/y" target="_blank" rel="noreferrer">badge.png</a></p></div>',
    );
  });
});
