import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { BackLink } from "./back-link";

vi.mock("next/navigation", () => ({ useRouter: () => ({ back: () => {} }) }));

describe("BackLink", () => {
  it("is a named link to where it goes, shown on a phone only", () => {
    const out = renderToStaticMarkup(<BackLink href="/agents/" label="Back to Agents" />);
    expect(out).toContain('aria-label="Back to Agents"');
    expect(out).toContain('href="/agents"');
    expect(out).toContain("sm:hidden");
    expect(out).toContain("h-11"); // a 44 px target
  });

  it("can show a word beside the arrow, as Settings' sections do", () => {
    expect(renderToStaticMarkup(<BackLink href="/settings/" label="Back to Settings">Settings</BackLink>)).toContain(">Settings<");
  });
});
