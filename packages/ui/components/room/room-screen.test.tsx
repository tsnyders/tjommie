import { readFileSync } from "node:fs";
import type { Bot, Room } from "@tjommie/protocol";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { MembersPanel } from "./members-panel";

const everyone: Room = { id: "team", name: "Everyone", botIds: ["b1", "b2"], everyone: true, createdAt: null };
const growth: Room = { id: "r1", name: "Growth", botIds: ["b1"], everyone: false, createdAt: "2026-10-25T09:00:00.000Z" };
const sales = { id: "b1", name: "Sales", color: "blue", shape: "circle", state: "idle", limitedUntil: null } as Bot;
const panel = (room: Room): string => renderToStaticMarkup(<MembersPanel room={room} members={[sales]} onEdit={() => {}} onHide={() => {}} />);

describe("a room's members panel", () => {
  it("lists the room's own bots, and offers Edit members in a room other than Everyone", () => {
    expect(panel(growth)).toContain('aria-label="Growth members"');
    expect(panel(growth)).toContain(">Sales<");
    expect(panel(growth)).toContain(">Edit members<");
    expect(panel(everyone)).not.toContain("Edit members"); // Everyone always holds every bot
  });
});

describe("Everyone's header", () => {
  it("has no Edit room or Delete room", () => {
    const source = readFileSync(new URL("./room-screen.tsx", import.meta.url), "utf8");
    // More's two items share the guard, and on a desktop Everyone has no More at all.
    expect(source.match(/\{!room\.everyone && \(\s*<Menu\.Item/g)).toHaveLength(2);
    expect(source).toContain("{!room.everyone && <MoreMenu");
  });
});
