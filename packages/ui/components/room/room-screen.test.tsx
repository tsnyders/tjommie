import { readFileSync } from "node:fs";
import type { Bot, Message, Room } from "@tjommie/protocol";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { MembersPanel } from "./members-panel";
import { RoomPost } from "./room-post";

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

describe("your post in a room", () => {
  it("marks the mentions the room's bots answer, read against every bot's name", () => {
    const bots = [{ ...sales, id: "b", name: "Bot" }, { ...sales, id: "b2", name: "Bot 2" }];
    const post = (body: string): Message => ({ id: "m1", conversationId: "r1", author: "you", kind: "text", body, data: null, status: "done", createdAt: "2026-10-25T09:00:00.000Z" });
    const html = (body: string): string => renderToStaticMarkup(<RoomPost message={post(body)} bots={bots} members={[bots[0]]} onOpenExchange={() => {}} />);
    expect(html("@Bot 2 please help")).not.toContain("text-mention"); // Bot 2 isn't in the room, and isn't taken for Bot
    expect(html("@Bot please help")).toContain('<span class="font-semibold text-mention">@Bot</span>');
    expect(html("@everyone hi")).toContain('<span class="font-semibold text-mention">@everyone</span>');
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
