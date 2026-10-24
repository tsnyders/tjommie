import { readFileSync } from "node:fs";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { mentionEscape, RoomComposer, withAt } from "./room-composer";

describe("RoomComposer", () => {
  it("points at the mention list only while it's open", () => {
    const html = renderToStaticMarkup(<RoomComposer />);
    expect(html).toContain('role="combobox"');
    expect(html).toContain('aria-expanded="false"');
    expect(html).not.toContain("aria-controls");
  });
});

describe("the mention list's size", () => {
  it("sits above the composer without taking its room, is capped, and scrolls inside itself; the box is capped on a phone too, so Send stays on screen", () => {
    const wrapper = readFileSync(new URL("./room-composer.tsx", import.meta.url), "utf8");
    expect(wrapper).toContain('className="relative shrink-0 px-4 pb-4"');
    expect(wrapper).toContain("max-sm:max-h-[min(200px,25dvh)]");
    const source = readFileSync(new URL("./room-composer.tsx", import.meta.url), "utf8");
    const list = source.slice(source.indexOf('id="mention-list"'), source.indexOf(">", source.indexOf('id="mention-list"')));
    expect(list).toContain("absolute bottom-full");
    expect(list).toContain("max-h-[min(240px,40dvh)]");
    expect(list).toContain("overflow-y-auto");
    expect(list).toContain("w-[min(280px,calc(100vw-32px))]"); // the width it already had
  });
});

describe("Escape and the mention list", () => {
  it("closes an open list for an Escape from anywhere in the composer, like Android's Back sent to the list itself", () => {
    expect(mentionEscape({ key: "Escape", defaultPrevented: false }, true)).toBe(true);
  });

  it("leaves an Escape the box already took, a closed list, and other keys alone", () => {
    expect(mentionEscape({ key: "Escape", defaultPrevented: true }, true)).toBe(false);
    expect(mentionEscape({ key: "Escape", defaultPrevented: false }, false)).toBe(false);
    expect(mentionEscape({ key: "Enter", defaultPrevented: false }, true)).toBe(false);
  });
});

describe("a post that finishes after more was typed", () => {
  it("clears the box only if nothing was typed since it was sent", () => {
    const source = readFileSync(new URL("./room-composer.tsx", import.meta.url), "utf8");
    expect(source).toContain("if (revision.now() === sentRevision) edit(\"\", 0);");
    expect(source).toContain('request("room.post", { text: sentText })');
    expect(source).toContain("guard.begin()");
  });
});

describe("the phone's @ button", () => {
  it("starts a mention at the caret, as its own word", () => {
    expect(withAt("", 0)).toEqual({ text: "@", caret: 1 });
    expect(withAt("hi ", 3)).toEqual({ text: "hi @", caret: 4 });
    expect(withAt("hi there", 2)).toEqual({ text: "hi @ there", caret: 4 });
  });
});
