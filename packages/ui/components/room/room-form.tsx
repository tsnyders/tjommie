"use client";

import { useRef, useState } from "react";
import { useRouter } from "next/navigation";
import type { Bot, Room, RoomPatch } from "@tjommie/protocol";
import { canMessageSummary, sameSet } from "@/lib/bot-form";
import { request } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { useApp } from "@/lib/store";
import { ifAlive, useAlive } from "@/lib/use-draft";
import { cn } from "@/lib/utils";
import { CanMessageRow } from "../bot-form/rows";
import { FIELD, Label } from "../controls";
import { Dialog } from "../dialog";
import { Pill } from "../pill";

// What saving an edit sends: only what was changed in the form, measured from the room as the form opened (`opened`), so a rename or a member
// change made elsewhere meanwhile is never put back. `botIds` are the form's members that still exist; a member deleted since the form opened left
// with its bot, so it isn't a change.
export function roomPatch(opened: Pick<Room, "name" | "botIds">, name: string, botIds: string[], bots: Pick<Bot, "id">[]): RoomPatch {
  const patch: RoomPatch = {};
  if (name.trim() !== opened.name) patch.name = name.trim();
  if (!sameSet(botIds, opened.botIds.filter((id) => bots.some((bot) => bot.id === id)))) patch.botIds = botIds;
  return patch;
}

// New room and Edit room, in a dialog: the name and the bots in the room. Save stays off until there are both (and, for an edit, a change), and the
// dialog stays open until it works, saying why when it doesn't. A new room opens once it's made.
export function RoomForm({ room, onClose }: { room?: Room; onClose: () => void }) {
  const router = useRouter();
  const bots = useApp((state) => state.bots);
  // The room as the form opened. The room prop follows changes made elsewhere; the edit is measured from this.
  const [opened] = useState(room);
  const [name, setName] = useState(room?.name ?? "");
  const [botIds, setBotIds] = useState(room?.botIds ?? []);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const nameField = useRef<HTMLInputElement>(null);
  // A save that finishes after this form was closed (and maybe another opened in its place) closes or opens nothing.
  const alive = useAlive();
  const chosen = botIds.filter((id) => bots.some((bot) => bot.id === id)); // a bot deleted meanwhile isn't sent
  const patch = opened ? roomPatch(opened, name, chosen, bots) : {};
  const canSave = !busy && name.trim() !== "" && chosen.length > 0 && (!opened || Object.keys(patch).length > 0);

  async function save(): Promise<void> {
    if (!canSave) return;
    setBusy(true);
    setProblem(null);
    try {
      if (opened) {
        await request("rooms.update", { id: opened.id, patch });
        ifAlive(alive, onClose);
      } else {
        const made = await request("rooms.create", { name: name.trim(), botIds: chosen });
        ifAlive(alive, () => router.push(`/room/?id=${encodeURIComponent(made.id)}`));
      }
    } catch (error) {
      ifAlive(alive, () => {
        setProblem(errorText(error));
        setBusy(false);
      });
    }
  }

  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
      title={opened ? "Edit room" : "New room"}
      width={420}
      closable
      initialFocus={nameField}
    >
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <div className="mt-4 flex flex-col gap-1">
          <Label htmlFor="room-name">Name</Label>
          <input id="room-name" ref={nameField} value={name} maxLength={60} onChange={(event) => setName(event.target.value)} className={cn(FIELD, "h-11")} />
        </div>
        <div className="mt-4">
          <CanMessageRow label="In this room" summary={canMessageSummary(chosen, bots)} ids={chosen} bots={bots} defaultOpen onChange={setBotIds}>
            The bots you mention answer here, and @everyone asks them all. Being in a room doesn&apos;t change who a bot can message.
          </CanMessageRow>
        </div>
        {problem && (
          <p role="alert" className="mt-3 text-[12px] leading-[18px] text-danger">
            {problem}
          </p>
        )}
        <div className="mt-5 flex justify-end gap-2">
          <Pill onClick={onClose}>Cancel</Pill>
          <Pill primary type="submit" disabled={!canSave}>
            {opened ? "Save" : "Create room"}
          </Pill>
        </div>
      </form>
    </Dialog>
  );
}
