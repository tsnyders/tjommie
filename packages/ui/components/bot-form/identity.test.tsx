import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { NEW_BOT } from "@/lib/bot-form";
import { Identity } from "./identity";

describe("Identity", () => {
  // Base UI picks each group's one tab stop (the chosen swatch) and handles the arrow keys once mounted, so the
  // server markup shows the groups and what's chosen.
  it("makes the color and shape pickers two radio groups, with the chosen swatches checked", () => {
    const html = renderToStaticMarkup(<Identity form={{ ...NEW_BOT, color: "teal", shape: "cloud" }} onChange={() => {}} nameError={null} />);
    expect([...html.matchAll(/role="radiogroup" aria-label="(\w+)"/g)].map((group) => group[1])).toEqual(["Color", "Shape"]);
    const radios = [...html.matchAll(/<span[^>]*role="radio"[^>]*>/g)].map((tag) => tag[0]);
    expect(radios).toHaveLength(18);
    const checked = radios.filter((tag) => tag.includes('aria-checked="true"')).map((tag) => /aria-label="([^"]+)"/.exec(tag)?.[1]);
    expect(checked).toEqual(["teal", "cloud"]);
  });
});
