import { renderToStaticMarkup } from "react-dom/server";
import { afterEach, describe, expect, it, vi } from "vitest";
import { companionName } from "@/lib/attachment-device";
import { previewKind } from "./file-viewer";
import { MarkdownText } from "./markdown";

afterEach(() => vi.unstubAllGlobals());

describe("safe attachment viewers", () => {
  it("keeps active/unknown formats download-only", () => {
    for (const type of ["image/svg+xml", "text/html", "application/javascript", "application/octet-stream"]) expect(previewKind(type)).toBe("unsupported");
    expect(previewKind("application/pdf")).toBe("pdf"); expect(previewKind("image/webp")).toBe("image");
  });
  it("allows Markdown anchors and only explicitly resolved local companion images", () => {
    const local = "app://tjommie/_attachments/41f8bac7-358c-48cf-9f48-a3a358804da6";
    const html = renderToStaticMarkup(<MarkdownText text={"# Notes\n\n[Jump](#notes) ![remote](https://example.com/private.png) ![shared](chart.png)\n\n<script>bad()</script>"} fileResources={(source) => source === "chart.png" ? local : null} />);
    expect(html).toContain('id="notes"'); expect(html).toContain('href="#notes"'); expect(html).toContain(`src="${local}"`); expect(html).not.toContain('src="https:'); expect(html).not.toContain("<script>");
  });
  it("refuses URLs, traversal, escaped traversal and host-path companion names", () => {
    for (const path of ["../private.png", "%2e%2e/private.png", "C:\\private.png", "//host/file", "https://example.com/file", "file.png?token=secret", "a/%2f../private"]) expect(companionName(path)).toBeNull();
    expect(companionName("images/chart.png")).toBe("images/chart.png");
  });
});
