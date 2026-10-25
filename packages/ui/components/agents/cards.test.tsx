import type { Room } from "@tjommie/protocol";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { TeamRoomCard } from "./cards";

const growth: Room = { id: "r1", name: "Growth", botIds: ["b1"], everyone: false, createdAt: "2026-10-25T09:00:00.000Z" };

describe("a room's card on Agents", () => {
  it("says nothing of the room's posts while they're being read, and that it has none once they are", () => {
    const reading = renderToStaticMarkup(<TeamRoomCard room={growth} messages={undefined} bots={[]} />);
    expect(reading).not.toContain("No bot chats yet");
    expect(reading).not.toContain("No posts yet");
    const read = renderToStaticMarkup(<TeamRoomCard room={growth} messages={[]} bots={[]} />);
    expect(read).toContain("No bot chats yet");
    expect(read).toContain("No posts yet");
  });
});
