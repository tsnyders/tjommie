import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { RoomComposer } from "./room-composer";

describe("RoomComposer", () => {
  it("points at the mention list only while it's open", () => {
    const html = renderToStaticMarkup(<RoomComposer />);
    expect(html).toContain('role="combobox"');
    expect(html).toContain('aria-expanded="false"');
    expect(html).not.toContain("aria-controls");
  });
});
