import { readFileSync } from "node:fs";
import type { Room } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { roomPatch } from "./room-form";

// Growth as the form opened: Sales (b1) and Marketing (b2). Chief (b3) is the other bot.
const growth: Room = { id: "r1", name: "Growth", botIds: ["b1", "b2"], everyone: false, createdAt: "2026-10-25T09:00:00.000Z" };
const bots = [{ id: "b1" }, { id: "b2" }, { id: "b3" }];

describe("saving an edited room", () => {
  it("sends only what was changed in the form, so a change made elsewhere meanwhile isn't put back", () => {
    // Renamed "Growth 2" elsewhere after the form opened; here only Marketing was taken out, so the name isn't sent.
    expect(roomPatch(growth, "Growth", ["b1"], bots)).toEqual({ botIds: ["b1"] });
    // Chief added elsewhere; here only the name changed, so the members aren't sent.
    expect(roomPatch(growth, " Growth team ", ["b1", "b2"], bots)).toEqual({ name: "Growth team" });
    expect(roomPatch(growth, "Growth", ["b2", "b1"], bots)).toEqual({}); // nothing changed here: Save stays off
  });

  it("doesn't count a member deleted since the form opened as a change", () => {
    const left = bots.filter((bot) => bot.id !== "b2");
    expect(roomPatch(growth, "Growth", ["b1"], left)).toEqual({});
    expect(roomPatch(growth, "Growth", ["b1", "b3"], left)).toEqual({ botIds: ["b1", "b3"] });
  });

  it("measures the edit from the room as the form opened, not from the room as it is now", () => {
    const source = readFileSync(new URL("./room-form.tsx", import.meta.url), "utf8");
    expect(source).toContain("const [opened] = useState(room);");
    expect(source).toContain("roomPatch(opened, name, chosen, bots)");
  });
});
