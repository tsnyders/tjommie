"use client";

import { Suspense } from "react";
import { useSearchParams } from "next/navigation";
import { RoomScreen } from "@/components/room/room-screen";
import { RoomsList } from "@/components/room/rooms-list";

function Rooms() {
  const roomId = useSearchParams().get("id");
  // key: each room starts fresh, with its own draft, panels and dialogs.
  return roomId ? <RoomScreen key={roomId} roomId={roomId} /> : <RoomsList />;
}

// A static export can't have /room/<id>, so the room comes as ?id=<id>, and with none this is the rooms list. useSearchParams needs the Suspense.
export default function RoomPage() {
  return (
    <Suspense>
      <Rooms />
    </Suspense>
  );
}
