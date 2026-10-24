# Plan: Multiple team rooms with chosen bots

## Context
Today there is exactly one room: conversation id `"team"` (`TEAM_ROOM_ID`, `packages/protocol/src/model.ts:3`), and every bot is in it. Nothing records membership. The spec already lists this as later work (`docs/superpowers/specs/2026-09-25-tjommie-design.md` §16: "Several rooms, or group chats with chosen bots"). The user wants to create several team rooms and choose which bots are in each one.

**Decisions (from the user):**
- The existing `"team"` room stays as **Everyone**. It always holds every bot. It can't be renamed, deleted or have its members edited.
- A `message_bot` exchange is mirrored into Everyone **and** into every custom room that has both the sender and the receiver as members.
- Room membership does **not** change `message_bot` reach. `canMessage` (`bot_links`) still controls that.
- Rooms appear as cards on the Agents overview. The rail/tab room icon opens a **rooms list** page that has a "New room" button.

**Base:** all paths are in the active worktree `C:/Users/tyrel/.paseo/worktrees/2cb7zj88/tjommie-phase1-completion` (branch `phase2-accounts-mobile-20261022`). It has about 186 uncommitted files, including the room screen's phone work, so commit or land that first. Then build this on a new branch cut from it.

## Data model (daemon)
`packages/daemon/src/db.ts`: append **migration 8** to the `migrations` array:
```sql
ALTER TABLE conversations ADD COLUMN name TEXT;
ALTER TABLE conversations ADD COLUMN created_at TEXT;
UPDATE conversations SET name = 'Everyone' WHERE id = 'team';
CREATE TABLE room_members (
  room_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  bot_id  TEXT NOT NULL REFERENCES bots(id) ON DELETE CASCADE,
  PRIMARY KEY (room_id, bot_id));
```
- A custom room is a `conversations` row with `kind='room'` plus its `room_members` rows. Everyone has no member rows: its members are always `listStoredBots(db)`.
- Deleting a bot already cascades its memberships away. Deleting a room cascades its messages and members.

## Protocol (`packages/protocol/src`)
- `model.ts`: add `Room = z.object({ id, name: z.string(), botIds: z.array(z.string()), everyone: z.boolean(), createdAt: z.string().nullable() })`, `RoomInput = { name: trim().min(1).max(60), botIds: array(string).min(1) }`, and `RoomPatch = RoomInput.partial()`.
- `RoomHandoffData` (strictObject): add `roomId: z.string().optional()`. Old rows have none, and the UI reads a missing value as `TEAM_ROOM_ID`.
- `wire.ts` methods:
  - `room.history`: add `roomId: z.string().default(TEAM_ROOM_ID)` to its params.
  - `room.post`: add `roomId: z.string().default(TEAM_ROOM_ID)`. Old callers keep working.
  - New: `rooms.list` (Empty → `Room[]`), `rooms.create` (`RoomInput` → `Room`), `rooms.update` (`{id, patch: RoomPatch}` → `Room`), `rooms.delete` (`Id` → Empty).
- `wire.ts` events: `room.updated: Room` and `room.deleted: Id`, mirroring `routine.updated`/`routine.deleted`.

## Daemon logic
- **New `packages/daemon/src/rooms.ts`**, small and modeled on `bots.ts` (`setLinks`/`validateLinks` at :241-254):
  - `listRooms` and `getRoom`. For Everyone, `getRoom` fills `botIds` with all bots and sets `everyone: true`.
  - `createRoom`, `updateRoom`, `deleteRoom`.
  - Validation: unknown bot ids → `bad_params`; a duplicate name → `name_taken`; editing or deleting `"team"` → `bad_state`.
  - `deleteRoom` refuses with `busy` while any `work_items` row with `status IN ('queued','running')` has `conversation_id = room`. `work_items.conversation_id` has no foreign key, so a turn still answering would otherwise write into a missing conversation.
  - `roomMembers(db, roomId)` returns the member `StoredBot[]`: all bots for `"team"`, the joined rows otherwise.
- **`team.ts`:**
  - `postToRoom(roomId, text)`: throw `not_found` if the conversation isn't `kind='room'`. Replace every `TEAM_ROOM_ID` in it with `roomId`, and run `findMentions` over `roomMembers(db, roomId)`, so `@everyone` means the room's members. `roomPrompt` takes the room name: `"[Team room "<name>": …"`.
  - `turnEnded` (:311): replace `item.conversationId === TEAM_ROOM_ID` with "the conversation is a room". Use the room name in the handoff body ("Answered in <name>") and add `roomId` to the data.
  - `#messageBot` (:465), `#answer` (:587) and `#fail` (:611): instead of one `#add` into `TEAM_ROOM_ID`, loop over a new `#roomsFor(fromId, toId)`. It returns `TEAM_ROOM_ID` plus the ids of rooms whose `room_members` include both bots (one SQL query with `GROUP BY room_id HAVING COUNT(*) = 2`). `#fail` after a bot deletion simply finds fewer rooms, which is fine.
- **`daemon.ts`:**
  - Handlers at :437-438 pass `roomId` through.
  - Add the four `rooms.*` handlers, which broadcast `room.updated` or `room.deleted`.
  - When a bot is deleted, broadcast `room.updated` for every room it was in, so the UI doesn't hold stale `botIds`. The UI can also just drop the id locally on `bot.deleted`; the plan uses the UI approach (see State).
- `bots.ts` `botPrompt`: no change. Bots learn about a room from each turn's `roomPrompt`.

## UI (`packages/ui`)
- **State** (`lib/state.ts`, `lib/connection.ts:123`):
  - Add `rooms: Room[]`, loaded with `rooms.list` in the same `Promise.all` as the other startup lists.
  - Handle `room.updated` (upsert) and `room.deleted` (remove), the same way as routines.
  - On `bot.deleted`, filter that id out of every room's `botIds`.
- **Connection:** change to `roomHistory(roomId, before)` (`lib/connection.ts:184`). Conversations are already keyed by id, so `loadConversation`/`loadOlder`/`retryConversation` work as they are.
- **Routes** (static export, so query params, as `/chat/?bot=` does):
  - `/room/?id=<roomId>` opens a room. With no `id`, it shows the rooms list.
  - `app/room/page.tsx` reads `useSearchParams` inside a Suspense boundary, copying `app/chat/page.tsx`.
- **`components/room/room-screen.tsx`:**
  - Take a `roomId` prop and swap every `TEAM_ROOM_ID` for it (:31, 44, 46, 58, 157).
  - Show the title from `room.name` and the count from `room.botIds.length` (:110-113, 144-146).
  - The More menu and desktop header get **Edit room** and **Delete room**, but not for Everyone. Delete asks for confirmation using the existing confirm dialog pattern from bot settings.
  - If the room disappears (a `room.deleted` event), go back to `/room/`.
- **`room-composer.tsx`:**
  - Take `roomId`, post `room.post {roomId, text}` (:92), and build mention options from the room's members only.
  - `@everyone` reads "All N bots in this room".
- **`members-panel.tsx`:** list `room.botIds` instead of all bots. For a custom room, add an "Edit members" button that opens the room form.
- **`exchange/exchange-view.tsx`** (:130, 138): it keeps posting to `TEAM_ROOM_ID`. The exchange's two bots are always both in Everyone.
- **New `components/room/room-form.tsx`:** a dialog with a name field and the bot multi-select. It is used for both create and edit.
  - To get the picker, generalize `CanMessageRow` (`components/bot-form/rows.tsx:105-144`): make `selfId` optional and pass the label and summary in as props. It is already a bot list with a `Switch` per bot that returns `string[]`, so don't write a second picker.
  - Save stays disabled until there is a name and at least one bot.
- **New `components/room/rooms-list.tsx`**, shown at `/room/`: Everyone first, then the custom rooms. Each row has the room name, an avatar stack (reuse the stack in `cards.tsx:76-80`) and the newest post line. A "New room" button opens the room form, and saving it navigates to `/room/?id=<new>`.
- **Agents overview** (`app/agents/page.tsx:43-48, 124`):
  - Fetch `room.history {roomId, limit: 10}` for each room in `rooms`, keeping a `Record<roomId, Message[]>` like `digests`.
  - Render one `TeamRoomCard` per room, each linking to `/room/?id=`.
  - `TeamRoomCard` (`components/agents/cards.tsx:190-231`) takes `room` and uses `room.name` instead of the hard-coded "Team room".
  - Add a small "New room" tile next to `NewBotCard`, reusing its dashed style. It opens the room form.
- **Links:**
  - The rail (`components/rail.tsx:27-28`) and phone tab (`lib/phone-nav.ts`) keep pointing at `/room/`, which is now the list. The active state should match `/room/` with or without `id`.
  - `chat/handoff-row.tsx:25-29` links to `/room/?id=${data.roomId ?? TEAM_ROOM_ID}`.

## Out of scope
- Old exchange posts between bots that have since been deleted still show "A deleted bot to A deleted bot" in Everyone. That is existing behavior (names aren't stored on messages) and is unchanged here.
- Per-room archive or mute, room icons, and moving existing history into a new room are all skipped. Add them if they're asked for.

## Tests (match existing style)
- `packages/daemon/src/db.test.ts`: migration 8 applies on a version-7 database. Everyone is named and has no member rows.
- New `packages/daemon/src/rooms.test.ts`:
  - create, update and delete
  - `name_taken`, and editing or deleting `"team"` → `bad_state`
  - deleting a room with a queued item → `busy`
  - deleting a bot cascades its memberships
- `team.test.ts` "Team room" block:
  - In a custom room, `@everyone` queues only members, and mentioning a non-member queues nothing.
  - Replies land in that room with a handoff that carries `roomId`.
  - An exchange between A and B is mirrored into Everyone and into a room containing A and B, but not into a room with only A.
  - A failure notice is mirrored the same way.
- `daemon.test.ts` end to end: `rooms.create`, then `room.post {roomId}`, then the `room.updated` and `message.added` events.
- Protocol `wire.test.ts`: `room.post` with no `roomId` defaults to `"team"`.
- UI:
  - `lib/state.test.ts`: `room.updated`/`room.deleted`, and `bot.deleted` prunes `botIds`.
  - `room-composer.test.tsx`: mention options are limited to members.
  - `lib/overview.test.ts`: `roomLine` works per room.

## Verification
1. `npm run typecheck`, `npm run lint` and `npm test` (via the PowerShell tool) for protocol, daemon and ui. The no-`any` gate must stay green.
2. Real app:
   - Create the room "Growth" with Sales, Marketing and Leads, and check that its card appears on Agents.
   - Post `@everyone hi`: only those three answer, in Growth. Each bot's chat shows "Answered in Growth", and its link opens Growth.
   - Make Sales `message_bot` Marketing: the exchange appears in Growth and in Everyone. A Sales-to-Chief exchange appears only in Everyone.
   - Edit members: remove Leads, and check that the count and the mention list update.
   - Delete Growth: it returns to the rooms list. Delete while a bot is answering: you get the "busy" message.
   - Everyone has no Edit or Delete.
3. Phone layout: `/room/` list and room sheet render, by running `phone-layout.real.test.ts`.
