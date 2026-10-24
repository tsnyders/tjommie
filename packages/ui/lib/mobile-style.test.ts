import { describe, expect, it } from "vitest";
import { hexColor, mobileStyle, mobileStyleScript } from "./mobile-style";

describe("the native canvas colour", () => {
  it("is always #rrggbb, whatever way the CSS wrote it", () => {
    expect(hexColor(" #F3EFE6 ")).toBe("#f3efe6");
    expect(hexColor("#fff")).toBe("#ffffff");
    expect(hexColor("#05060aff")).toBe("#05060a");
    expect(hexColor("rgb(7, 7, 7)")).toBe("#070707");
    expect(hexColor("rgba(255 255 255 / 0.5)")).toBe("#ffffff");
  });

  it("is nothing for a value it can't read, so the native side is never sent junk", () => {
    expect(hexColor("")).toBeNull();
    expect(hexColor("var(--canvas)")).toBeNull();
    expect(hexColor("#12345")).toBeNull();
  });
});

describe("the phone style", () => {
  it("is Hush where storage is out of reach", () => {
    expect(mobileStyle()).toBe("hush");
  });

  it("is put on <html> before the first paint, falling back to Hush for anything unknown", () => {
    expect(mobileStyleScript).toContain("tjommie.mobileStyle");
    expect(mobileStyleScript).toContain('s==="folio"||s==="aurora"?s:"hush"');
    expect(mobileStyleScript).toContain("data-reduce-effects");
  });
});
