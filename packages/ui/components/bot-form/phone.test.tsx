import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";
import { Shield } from "lucide-react";
import { NEW_BOT } from "@/lib/bot-form";
import { Identity } from "./identity";
import { Row } from "./rows";

vi.mock("@/lib/connection", () => ({ request: vi.fn(), daemon: vi.fn() }));

describe("the bot form on a phone", () => {
  it("stacks the avatar tile over the name and pickers, centres the tile, and lets the swatches wrap, so nothing is wider than the screen", () => {
    const out = renderToStaticMarkup(<Identity form={NEW_BOT} onChange={() => {}} nameError={null} />);
    expect(out).toContain("max-sm:flex-col");
    expect(out).toContain("max-sm:self-center");
    expect(out.match(/max-sm:flex-wrap/g)).toHaveLength(2); // the color and shape groups
    expect(out).toContain("min-w-0 flex-1"); // the name column may shrink
  });

  it("gives an open row's controls the row's width on a phone, scrolling inside it if one is still wider, never the page", () => {
    const out = renderToStaticMarkup(
      <Row icon={Shield} title="Autonomy" open onToggle={() => {}}>
        <span>controls</span>
      </Row>,
    );
    expect(out).toContain("pl-[46px]"); // the desktop indent
    expect(out).toContain("max-sm:pl-4");
    expect(out).toContain("max-sm:overflow-x-auto");
    expect(out).toContain("min-w-0");
  });

  it("lets a row's title wrap and its height grow on a phone, with a long value cut short beside it", () => {
    const out = renderToStaticMarkup(
      <Row icon={Shield} title="Account and model" value={<span className="truncate">A long account name · and a long model name</span>} open={false} onToggle={() => {}}>
        <span />
      </Row>,
    );
    expect(out).toContain("max-sm:h-auto");
    expect(out).toContain("max-sm:min-h-14");
    expect(out).toContain("max-sm:whitespace-normal");
    expect(out).toContain("max-w-[60%]");
  });
});
