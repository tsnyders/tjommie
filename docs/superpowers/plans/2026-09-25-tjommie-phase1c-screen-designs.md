# Tjommie Phase 1C: Screen Designs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Design every Phase 1 screen that doesn't exist yet in the live OpenPencil wireframe document, so plan 1D can build the UI from pictures instead of guesses.

**Architecture:** All work happens in the running OpenPencil app through its MCP tools, on the existing "Wireframes" page, next to WF1 to WF4. New frames are mostly clones of WF1, WF3 and WF4, edited in place, so the rail, header, composer and panels stay identical to the approved wireframes. Each frame is exported as PNGs to `docs/wireframes/`, and the document is saved into the repo as `design/tjommie-wireframes.fig`.

**Tech Stack:** OpenPencil desktop 0.15.1 with `@open-pencil/mcp` 0.15.1 (tools `mcp__open-pencil__*`), Inter, Iconify lucide icons.

**Spec:** `docs/superpowers/specs/2026-09-25-tjommie-design.md` (sections 4 to 7 describe what these screens do; §12.1 lists the screens). Visual rules: `docs/design/brief.md` (tokens and roster) and `docs/design/wireframe-spec.md` (WF1 to WF4 measurements).

## What this plan designs

| Frame | Position on the page | Built from | For |
|---|---|---|---|
| WF1 Main chat, Phase 1 (dark) | (3080, 0) | clone of WF1 dark | the chat screen as Phase 1 ships it: no live screen, Phase 1 message kinds, bot panel |
| WF11 Chat states (dark) | (1540, 4000), 1440 wide | new, with parts cloned from the frame above | every message, card, row, composer, rail and dialog state |
| WF5 Team room (dark) | (1540, 2000) | clone of WF1 Phase 1 | the room: bot-to-bot posts, @mentions, members |
| WF6 Exchange view (dark) | (3080, 2000) | clone of WF1 Phase 1 | an opened bot-to-bot exchange |
| WF7 Routine editor (dark) | (0, 3000) | clone of WF1 Phase 1 plus a dialog | creating and editing a routine, with run history |
| WF3 New bot, Phase 1 (dark) | (0, 2000) | clone of WF3 | the New bot screen with Phase 1 settings rows |
| WF8 Bot settings (dark) | (1540, 3000) | clone of WF3 Phase 1 | editing, pausing and deleting a bot |
| WF9 First-run setup (dark) | (3080, 3000) | clone of WF1 Phase 1, emptied | signing Tjommie's own Claude and Codex accounts in |
| WF10 Settings: Bots (dark) | (0, 4000) | clone of WF4 | the Phase 1 Settings page |

The existing frames stay where they are. Row 1 (y = 0) holds WF1 dark and light; row 2 (y = 1000) holds WF2, WF3 and WF4. The six "Bot avatar / …" components sit at x = −320.

## Global Constraints

- **The user's hard rule:** do the design work live in the running OpenPencil app through the `mcp__open-pencil__*` tools, so it appears in the app as it's built.
  - Don't write OpenPencil JSX, don't use the `render` tool, and don't use the headless scripts in `design/tools/`.
  - If the MCP reports that no app is attached (health `no_app`), stop and ask the user to restart OpenPencil.
- **Target:** document `tab-1`, page "Wireframes" (`0:328`). Call `list_documents` first and confirm both.
- **One drawing agent at a time.** Exports break when two agents draw on different pages at once.
- **Coordinates:** "(x, y) in `<layer>`" in this plan means an offset from that layer's top-left corner.
  - **`create_shape` and `create_vector`** take PAGE coordinates, even with `parent_id`. Add the container's page origin (from `node_bounds`) to the offset.
  - **`node_move`** takes parent-relative coordinates, so the offset can be passed as-is.
  - **Example:** a card at (16, 56) in a panel whose page origin is (4060, 0) is created at page (4076, 56).
- **Order inside auto-layout frames** (tested on 2026-09-25):
  - A new child, whether created or cloned, always lands at the END of its parent.
  - Reparenting a node into its own parent does nothing.
  - To move a child to the end, reparent it to any other frame, then back into the auto-layout frame.
  - So to insert new rows mid-list, add them first (they land at the end). Then move out and back in, in the final order, every existing child that must come after the first new row.
- **Reading and editing clones:** find a clone's layers with `get_page_tree` (`root_id` set to the clone, a small `depth`) or `query_nodes`, then edit them by layer name.
  - Cloned frames often don't clip their children, so keep everything inside its container.
  - New frames start white; set their fill.
  - `#00000000` renders black.
- **Colors:** use only these tokens. Every one comes from `docs/design/brief.md` or WF1 to WF4, so the light theme maps mechanically.

  | Token | Value |
  |---|---|
  | Canvas | `#070707` |
  | Rail | `#111111` |
  | Card | `#111111` |
  | Card stroke | `#1C1C1C` |
  | Drawer | `#0B0B0B` |
  | Divider | `#252525` |
  | Bot bubble | `#262626` |
  | Secondary pill | `#262626` |
  | Header chip | `#181818` |
  | Composer | `#2F2F2F` |
  | Selected | `#303030` |
  | Popover | `#1A1A1A` |
  | Input stroke | `#2A2A2A` |
  | Text | `#F5F5F5` |
  | Title | `#FCFCFC` |
  | Muted body | `#BDBDBD` |
  | Secondary | `#989898` |
  | Captions | `#909090` |
  | Section label | `#9C9C9C` |
  | Placeholder | `#6E6E6E` |
  | Icon | `#ACACAC` |
  | Accent (waiting, links, focus) | `#459FFF` |
  | Working and success | `#3FA37A` |
  | Destructive only | `#EA4144` |
  | Waiting card | fill `#101722`, stroke `#1E3A5F` |
  | Mention in a light bubble | `#1D6FE0` |

  Bot colors appear only inside avatars.
- **Type:** Inter only. Sizes 11 (captions on the states sheet), 12, 13, 14, 15, 17 and 20. Weights 400, 500 and 600.
- **Radius:** bubbles 18; cards 12; large cards 14; dialogs 16; inputs and rows 10; pills and chips fully round.
- **Icons:** `insert_icon` with `lucide:*` names, a size and a color. Then reparent and `node_move` them.
- **Avatars:** never resize an avatar instance, because resizing doesn't scale its contents. Instead:
  - **40 px:** `create_instance` of a component, then reparent it immediately (it lands at page level). The components are Chief of Staff `0:500`, Pnet apply `0:575`, CV Writer `0:600`, Inbox Triage `0:595`, Night Shift `0:605` and Careers24 scout `0:610`.
  - **28 px:** clone Pnet apply `0:3293`, CV Writer `0:4059`, Inbox Triage `0:4066`, Chief of Staff `0:4076`, Careers24 scout `0:4680` or Night Shift `0:4695`.
  - **24 px:** clone Chief of Staff `0:5138`, Pnet apply `0:5147`, CV Writer `0:5154`, Inbox Triage `0:5161`, Night Shift `0:5172` or Careers24 scout `0:5180`.
  - **18 px:** clone Pnet apply `0:1132`, CV Writer `0:1137`, Chief of Staff `0:1301`, Inbox Triage `0:4203` or Careers24 scout `0:4047`. There's no 18 px Night Shift, so don't design one.
- **Copy:** use the sample roster and story from `docs/design/brief.md` §6. It's sample content, the same as WF1 to WF4. No em dashes or en dashes, no emoji, and plain language. Times look like "9:31 AM".
- **Phase 1 leaves these out:** the store icon, the Share icon, the mic and voice buttons, the live screen, Take over, Pause and Stop computer controls, the This PC / Virtual machine switch, the Chrome profile card's controls, logins, and usage numbers.
- **Exports:** use `export_image` with `ids: [<frame id>]` and a `path` under `docs/wireframes/`.
  - **1x:** `maxEdge` equal to the frame's longer edge.
  - **2x:** `scale: 2` and `maxEdge` twice the longer edge.
  - `switch_page` to "Wireframes" first if the app shows another page.
- **Saving:** `save_file` with `path: "design/tjommie-wireframes.fig"` at the end of every task.
- **Commits:** one commit per task. Imperative sentence case, then a blank line and `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Never push.

## Review Focus

These are the five things most likely to hurt plan 1D if the frames skip them. Each one has a place in a task.

1. **Long text.** A long bot reply wraps inside the 600 px bubble, and a long handoff row truncates with an ellipsis instead of pushing the chevron off the row. *Covered in Task 3.*
2. **Empty states:** an empty chat, an empty Team room, no routines and no recent activity. *Covered in Task 3.*
3. **Needs setup and Limited states:**
   - The rail badges (Tasks 2 and 3).
   - The chat banners (Task 3).
   - The member list states (Task 4).
   - The header chip "Limited until 3:45 PM" (Task 3).
4. **Keyboard focus.** A focused input and a focused button show a visible 2 px `#459FFF` ring. *Covered in Tasks 3 and 6.*
5. **Only tokens.** Every task's check includes "no color outside the token table", so the light theme stays a mechanical swap.

---

### Task 1: Save the wireframe document into the repo

The live document has never been saved; it shows as "Untitled". Saving it first protects WF1 to WF4 and gives every later task a file to commit.

**Files:**
- Create: `design/tjommie-wireframes.fig`

**Interfaces:**
- Consumes: the live document `tab-1`.
- Produces: `design/tjommie-wireframes.fig`, which every later task re-saves.

- [ ] **Step 1: Check the connection**

Call `mcp__open-pencil__list_documents`.
Expected: a document with `id: "tab-1"`, containing a page `{ id: "0:328", name: "Wireframes" }`. If the call fails or reports no app, ask the user to restart OpenPencil and wait.

- [ ] **Step 2: Save**

Call `mcp__open-pencil__save_file` with `document_id: "tab-1"` and `path: "design/tjommie-wireframes.fig"`.
Expected: success. If it answers that the path is outside the MCP root, ask the user where the MCP root points; don't guess another path.

- [ ] **Step 3: Check the file**

Run (PowerShell, repo root): `Get-Item design/tjommie-wireframes.fig | Select-Object Length`
Expected: the file exists and is larger than 100 KB.

- [ ] **Step 4: Commit**

```bash
git add design/tjommie-wireframes.fig
git commit -m "Save the wireframe document into the repo

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: WF1 Main chat, Phase 1

The Phase 1 version of the main screen. It shows a bot mid-routine, with the message kinds Phase 1 has, and a right panel without the live screen.

**Files:**
- Modify: `design/tjommie-wireframes.fig`
- Create: `docs/wireframes/wf1-phase1-main-chat-dark.png`, `docs/wireframes/wf1-phase1-main-chat-dark@2x.png`

**Interfaces:**
- Consumes: frame "WF1 Main chat + computer (dark)" (`0:716`), WF2's "Chip / Working" (`0:3797`), and component `0:575`.
- Produces: frame "WF1 Main chat, Phase 1 (dark)". Later tasks clone these layers from it: "Rail", "Header chip", "Panel toggle (on)", "More", "Messages", "Routine label", "Activity row (collapsed)", "Approval card", "Composer", "Stop", "Bot panel", "Bot summary", "Browser card", "Recent activity".

- [ ] **Step 1: Clone and place**

`clone_node` `0:716`. Then `node_move` the clone to page (3080, 0) and rename it "WF1 Main chat, Phase 1 (dark)".

- [ ] **Step 2: Rail**

In the clone's "Rail":
1. Delete "Icon / lucide:store". Move "Icon / lucide:plus" and "Icon / lucide:layout-grid" each 44 px down, so the bottom cluster stays evenly spaced.
2. Keep "Live dot" (green, on Pnet apply). In Phase 1 it means Working. Keep "Waiting dot" (blue, on Inbox Triage). Keep Night Shift at 50% opacity (Paused).
3. **Limited badge on Careers24 scout:**
   - Set the Careers24 scout avatar's opacity to 0.5.
   - Add an ELLIPSE named "Limited badge", 16 × 16, fill `#262626`, with a 2 px `#111111` stroke. Place its center 3 px in from the avatar's bottom-right corner.
   - Insert `lucide:hourglass` (size 10, `#ACACAC`) and center it in the badge.

- [ ] **Step 3: Header**

In "Chat":
1. **Header chip:** set "Live label" to "Working" (keep the green dot and color) and rename "Live status" to "State". If the chip doesn't grow to fit, widen it by the text's extra width, then center it again in the 892 px column.
2. **"Computer toggle (on)":** delete its icon, insert `lucide:panel-right` at the old icon's size and color and center it, and rename the layer "Panel toggle (on)".
3. **"Share":** delete its icon, insert `lucide:ellipsis` the same way, and rename the layer "More".

- [ ] **Step 4: Messages**

1. Delete "Earlier messages".
2. **Routine label:** in "Messages" (a vertical auto-layout, gap 12), insert a new horizontal auto-layout frame named "Routine label" directly after "Timestamp 9:14".
   - Height 20, gap 6, fill `#070707`.
   - Inside: `lucide:clock` (14, `#3FA37A`), then the text "Routine: Pnet apply 9am" (Inter 12, weight 500, `#909090`).
3. **Activity row:** insert a new horizontal auto-layout frame named "Activity row (collapsed)" directly after "Bot bubble 1".
   - Height 32, padding 0 12, gap 8, radius 10, fill `#070707`.
   - Inside: `lucide:list-checks` (14, `#909090`), the text "4 steps: Opened the Sabenza IT listing on pnet.co.za" (Inter 13, `#989898`), then `lucide:chevron-down` (14, `#6E6E6E`).
4. **Approval card:** rename "Waiting card" to "Approval card".
   - Set "Body" to "Pnet apply wants to save CV-Sabenza-Angular.pdf in C:\Users\tyrel\Documents\Job applications, outside its own folder."
   - After "Body", insert a text named "Detail": "Write · C:\Users\tyrel\Documents\Job applications\CV-Sabenza-Angular.pdf" (Inter 12, `#989898`, fill width).
   - In "Actions":
     - "Button / Take over": delete its hand icon, set its label to "Allow once", and rename it "Button / Allow once". Keep the white pill.
     - "Button / Reply with code": set its label to "Always allow" and rename it "Button / Always allow".
     - Clone "Button / Always allow", set the clone's label to "Deny", rename it "Button / Deny", and keep it last in "Actions".
5. **Put the rows in order.** "Routine label" and "Activity row (collapsed)" landed at the end of "Messages". Move these children out to "Chat" and back into "Messages", one at a time, in this order:
   1. "Bot bubble 1"
   2. "Activity row (collapsed)"
   3. "Handoff row / Pnet apply to CV Writer"
   4. "Attachment card"
   5. "Bot bubble 2"
   6. "User bubble"
   7. "Handoff row / Chief of Staff"
   8. "Timestamp 10:02"
   9. "Approval card"

   The final order is: Timestamp 9:14, Routine label, Bot bubble 1, Activity row, handoff row, Attachment card, Bot bubble 2, User bubble, handoff row, Timestamp 10:02, Approval card.

   Inside "Approval card", "Detail" landed after "Actions", so move "Actions" out and back in.
6. **Re-anchor the list:** read the new height of "Messages" (`get_node`), then set its y in "Chat" to 816 minus that height. That keeps its bottom 24 px above the composer.

- [ ] **Step 5: Composer**

In "Composer":
1. Delete "Icon / lucide:mic".
2. Rename "Voice" to "Stop" and keep its 32 px white circle.
3. Delete its `audio-lines` icon, then insert `lucide:square` (12, `#070707`) and center it.

The bot is Working and the field is empty, so the round button stops the turn. Its other states are in WF11.

- [ ] **Step 6: Right panel**

Rename "Computer panel" to "Bot panel". Keep "Panel edge", the settings and chevrons icons, and the three caption buttons. Delete "Live screen", "Screen caption", "Target switch", "Controls", "Status" and "Logins card".

Then build these, with positions in "Bot panel":

1. **"Bot summary"** frame at (16, 56), 428 × 124, fill `#111111`, radius 12. Contents:
   - A 40 px Pnet apply instance (component `0:575`) at (16, 16).
   - The text "Pnet apply" (Inter 17, 600, `#FCFCFC`) at (68, 16).
   - A clone of "Chip / Working" (`0:3797`), right-aligned 16 px from the card's right edge, with its top at y 18.
   - `lucide:sparkles` (14, `#989898`) at (68, 49), then "Claude · Sonnet (latest)" (Inter 13, `#BDBDBD`) at (88, 46).
   - `lucide:shield` (14, `#989898`) at (68, 73), then "Own folder: works freely in its folder, asks before anything else" (Inter 13, `#989898`, width 336) at (88, 70).
   - `lucide:folder` (14, `#989898`) at (68, 97), then "Home folder" (Inter 13, `#989898`) at (88, 94).
   - "Open" (Inter 13, `#459FFF`), right-aligned 16 px from the right edge, at y 94.
2. **"Browser card":** rename "Chrome profile card" to "Browser card" and `node_move` it to (16, 196).
   - Set "Title" to "Browser".
   - Set "Detail" to "Its own Chrome window, separate from yours. Sites you sign in to there stay signed in." Phase 1 doesn't count signed-in sites; that arrives with Logins in Phase 4.
   - Delete its chevron icon. In its place add the text "Show window" (Inter 13, `#459FFF`), right-aligned 16 px from the card's right edge, on the same line as "Title". It opens the bot's browser profile (plan 1B's `bots.showBrowser`).
3. **Routines:** `node_move` "Routines" to (16, 277). Add a text "Add" (Inter 13, `#459FFF`) right-aligned to x 444, at y 277.
4. **"Recent activity"** frame (vertical auto-layout, gap 12, width 428, fill `#070707`) at (16, 460).
   - The text "Recent activity" (Inter 13, 600, `#9C9C9C`).
   - Then four rows. Each row is a horizontal frame, width 428 and height 20, with an icon (14) at x 0, a text (Inter 13, `#BDBDBD`) at x 24, and a time (Inter 12, `#909090`) right-aligned to x 428:
     - `lucide:loader` `#ACACAC`, "Filling the Apply form on pnet.co.za", "now"
     - `lucide:check` `#3FA37A`, "Asked CV Writer for a tailored CV", "9:31 AM"
     - `lucide:check` `#3FA37A`, "Read listing PN-48213", "9:20 AM"
     - `lucide:check` `#3FA37A`, "Searched pnet.co.za for Full Stack roles", "9:15 AM"

- [ ] **Step 7: Export and check**

Export the frame at 1x (`maxEdge: 1440`) to `docs/wireframes/wf1-phase1-main-chat-dark.png`, and at 2x (`scale: 2`, `maxEdge: 2880`) to `docs/wireframes/wf1-phase1-main-chat-dark@2x.png`. Open the 1x PNG with the Read tool and check:

- [ ] No text is clipped or overflows its container; nothing overlaps the composer.
- [ ] The rail bottom cluster (grid, plus, settings, user) is evenly spaced, with no store icon.
- [ ] The Careers24 scout badge sits on the avatar's bottom-right edge and reads as a separate badge.
- [ ] The header chip says "Working" with a green dot, and the header has no Share icon.
- [ ] Messages read top to bottom:
  - Timestamp, routine label, bubble, activity row, handoff row and file card.
  - Bubble, user bubble, handoff row, timestamp, then the approval card with Allow once, Always allow and Deny.
- [ ] The composer has no mic, and the right button shows a square.
- [ ] The right panel reads top to bottom: summary card, Browser card with "Show window", Routines with "Add", Recent activity. Nothing is left over from the live screen.
- [ ] Every color comes from the token table.

Fix anything that fails, then export again.

- [ ] **Step 8: Save and commit**

`save_file` (`path: "design/tjommie-wireframes.fig"`), then:

```bash
git add design/tjommie-wireframes.fig docs/wireframes/wf1-phase1-main-chat-dark.png docs/wireframes/wf1-phase1-main-chat-dark@2x.png
git commit -m "Design the Phase 1 main chat screen

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: WF11 Chat states sheet

One reference frame with every state plan 1D has to build. It isn't a screen; it's labeled examples arranged in two columns.

**Files:**
- Modify: `design/tjommie-wireframes.fig`
- Create: `docs/wireframes/wf11-chat-states-dark.png`, `docs/wireframes/wf11-chat-states-dark@2x.png`

**Interfaces:**
- Consumes: "WF1 Main chat, Phase 1 (dark)" (Task 2) and its layers, the WF1 layers "User bubble", "Bot bubble 1", "Handoff row / Pnet apply to CV Writer" and "Attachment card", WF2's chips, and the avatar ids.
- Produces: frame "WF11 Chat states (dark)" containing the named items "Mention popover", "Model picker", "Full auto warning", "Delete bot dialog", "Handoff rows", "Composer states" and "Rail states". Tasks 4, 5 and 7 clone some of them.

- [ ] **Step 1: Frame and columns**

Create a FRAME named "WF11 Chat states (dark)" at page (1540, 4000), 1440 × 1800, fill `#070707`. Inside it:

- The text "WF11 Chat states, Phase 1" (Inter 20, 600, `#FCFCFC`) at (40, 32).
- The text "Every message, card, row and control state the Phase 1 chat needs. Sample content." (Inter 13, `#989898`) at (40, 64).
- "Column A": a vertical auto-layout frame, gap 28, width 640, fill `#070707`, at (40, 110).
- "Column B": a vertical auto-layout frame, gap 28, width 660, fill `#070707`, at (740, 110).

Every item below is a vertical auto-layout frame named "Item / <caption>", gap 8, fill `#070707`. It holds the caption (Inter 11, 600, `#6E6E6E`), then the example.

- [ ] **Step 2: Column A items (in order)**

1. **"Your message, queued":** a clone of "User bubble" with its text set to "Also check careers24.com". Under it, right-aligned, the text "Queued" (Inter 11, `#909090`).
2. **"Your message, sent":** a clone of "User bubble", text "Skip senior roles for the rest of today."
3. **"Bot reply, streaming":** a clone of "Bot bubble 1" with its text set to "Looking at the six listings now. Two of them want Angular, so I'll ask CV Writer". Right after the text, inside the bubble, add a RECTANGLE 2 × 16 `#F5F5F5` as the typing caret.
4. **"Long reply wraps":** a clone of "Bot bubble 1" with its text set to "Applied to Full Stack Developer at Sabenza IT in Pretoria. Pnet showed the confirmation page and sent a copy to tyrel.jobs@gmail.com. The next listing on my list is Senior React Engineer at Takealot, which I skipped because you asked me to leave senior roles alone today." It must wrap onto several lines inside the 600 px bubble.
5. **"Activity row, collapsed":** a clone of "Activity row (collapsed)".
6. **"Activity row, expanded":** a vertical auto-layout frame, width 560, padding 12, gap 6, radius 10, fill `#070707`, 1 px `#252525` stroke.
   - First a clone of "Activity row (collapsed)" with its chevron changed to `lucide:chevron-up`.
   - Then four rows, each height 24 with an icon (14) plus text (Inter 13) and gap 8:
     - `lucide:check` `#3FA37A`, "Opened pnet.co.za/jobs/full-stack-developer-sabenza" `#BDBDBD`
     - `lucide:check` `#3FA37A`, "Read the listing requirements" `#BDBDBD`
     - `lucide:loader` `#ACACAC`, "Filling the Email field" `#F5F5F5`
     - `lucide:x` `#989898`, "Uploading the CV failed: the file picker closed" `#989898`
7. **"Routine labels":** a clone of "Routine label". Below it, a second label with `lucide:clock` (14, `#6E6E6E`) and "Routine: Night Shift digest · Missed, the PC was off" (Inter 12, `#909090`).
8. **"Notices":** three centered rows, each with an icon (12, `#909090`) and Inter 12 `#909090` text:
   - `lucide:square`, "Stopped."
   - `lucide:timer-off`, "Stopped at the time limit (3 hours)."
   - `lucide:hourglass`, "Pnet apply hit its usage limit. It will pick up again at 3:45 PM on Thursday."
9. **"Errors with Retry":** two cards, each width 560, padding 12, radius 12, fill `#111111`, 1 px `#252525` stroke, horizontal with gap 10.
   - Contents: `lucide:circle-alert` (16, `#989898`), the text (Inter 13, `#F5F5F5`, fill width), then a "Retry" pill (fill `#262626`, height 28, padding 0 12, `lucide:rotate-ccw` 14 `#F5F5F5` + "Retry" Inter 13 `#F5F5F5`).
   - Texts: "Claude stopped unexpectedly." and "Tjommie restarted while this was running."
10. **"Needs setup banner":** a card, width 560, padding 12, radius 12, fill `#101722`, 1 px `#1E3A5F` stroke. It holds `lucide:plug` (16, `#459FFF`), "Claude isn't signed in for Tjommie yet." (Inter 13, `#F5F5F5`), and a white "Sign in" pill (height 28, Inter 13, 600, `#070707`).
11. **"Limited header chip":** a clone of "Header chip".
    - Replace its green dot with `lucide:hourglass` (12, `#989898`).
    - Set its state text to "Limited until 3:45 PM" in `#989898`.
12. **"Empty chat":** a centered stack, width 640.
    - A 40 px Pnet apply instance at opacity 0.35.
    - Under it, "Say hi to Pnet apply. It works in its own folder and asks before touching anything else." (Inter 13, `#989898`, width 360, centered).

- [ ] **Step 3: Column B items (in order)**

1. **"Approval card, waiting":**
   - A clone of "Approval card" from Task 2.
   - Under its buttons, add "Always allow saves: Write, for Pnet apply only" (Inter 12, `#6E6E6E`).
2. **"Approval card, answered":** two rows. Each row: height 36, padding 0 12, gap 8, radius 10, 1 px `#252525` stroke, fill `#070707`.
   - `lucide:check` (14, `#3FA37A`) with "Allowed once: save CV-Sabenza-Angular.pdf in Job applications" (Inter 13, `#989898`).
   - `lucide:x` (14, `#989898`) with "Denied: delete the old-cvs folder" (Inter 13, `#989898`).
3. **"Routine proposal card":** a clone of "Approval card".
   - Body: "Pnet apply wants to add a routine: Morning check, weekdays at 8:00 AM, stop after 1 hour."
   - Detail: "It will: check pnet.co.za for new Full Stack roles and apply to the ones that fit."
   - Buttons: keep "Allow once" relabelled "Add routine", delete "Always allow", and relabel "Deny" to "Dismiss".
4. **"Handoff rows":** a vertical auto-layout frame named "Handoff rows", gap 8. Each row is a clone of "Handoff row / Pnet apply to CV Writer", edited:
   - **Pending:** text "Pnet apply asked CV Writer to tailor your CV for Sabenza IT". Replace "2 messages" with `lucide:loader` (12, `#ACACAC`) plus "waiting for a reply" (Inter 13, `#909090`).
   - **Replied:** unchanged ("… 2 messages").
   - **Failed:** swap the second avatar for Careers24 scout (18 px, `0:4047`). Text "Pnet apply asked Careers24 scout for new React roles"; trailing text "no reply" `#909090`.
   - **Incoming** (the target bot's chat): delete the arrow and second avatar. Text "Pnet apply asked: Tailor the CV for listing #PN-48213"; trailing "1 message".
   - **Room answer:** replace both avatars and the arrow with `lucide:users` (14, `#909090`). Text "Answered in the Team room"; delete the trailing count.
   - **Long, truncated:** text "Pnet apply asked Chief of Staff to add the Sabenza IT application and the two Careers24 roles to the 5pm digest, and skip senior roles". Fix the row width at 565 and let the text truncate with an ellipsis, so the chevron stays visible.
5. **"File card":** a clone of "Attachment card".
6. **"Team room hints":** two centered texts (Inter 12, `#909090`):
   - "No bot was mentioned, so nobody will reply. Type @ to ask a bot."
   - "No posts yet. @mention a bot to ask it something."
7. **"Composer states":** a vertical auto-layout frame named "Composer states", gap 12, width 660. Each composer is a clone of Task 2's "Composer", width 660:
   - **Empty:** the round button is a 32 px circle `#3A3A3A` holding `lucide:arrow-up` (16, `#6E6E6E`) (disabled send).
   - **Typing:** the placeholder is replaced by "Skip senior roles today" (Inter 14, `#F5F5F5`). The round button is white `#F5F5F5` holding `lucide:arrow-up` (16, `#070707`).
   - **Bot working, empty:** the "Stop" button from Task 2 (white, with a square).
   - **With attachments:** above an empty composer, one chip: fill `#262626`, fully round, height 28, padding 0 10. It holds `lucide:paperclip` (12, `#ACACAC`), "CV final.pdf" (Inter 12, `#F5F5F5`), and `lucide:x` (12, `#989898`).
8. **"Rail states":** a horizontal frame named "Rail states", gap 32, fill `#111111`, padding 16, radius 12. It holds six 40 px avatars, each with a caption under it (Inter 11, `#6E6E6E`):
   - Chief of Staff, "Idle".
   - Pnet apply with a green "Live dot" clone, "Working".
   - Inbox Triage with a blue "Waiting dot" clone, "Waiting for you".
   - Night Shift at 0.5 opacity, "Paused".
   - Careers24 scout at 0.5 with a "Limited badge" clone, "Limited".
   - CV Writer with a "Needs setup badge": the same badge as Limited, but holding `lucide:plug` (10, `#ACACAC`). Caption "Needs setup".
9. **"Dialogs":** a horizontal auto-layout frame, gap 20, wrapping onto two rows of two. Each dialog: width 320, padding 20, radius 16, fill `#111111`, 1 px `#252525` stroke, vertical with gap 12.
   - **"Full auto warning":**
     - Title "Let Pnet apply work without asking?" (Inter 17, 600, `#FCFCFC`, width 280).
     - Body "In Full auto, Pnet apply can run any command and change any file your Windows account can, without checking with you first. Only choose it for bots you trust with that." (Inter 14, `#BDBDBD`, width 280).
     - A right-aligned button row: "Cancel" (`#262626` pill, height 32) and "Use Full auto" (white pill, height 32).
   - **"Delete bot dialog":**
     - Title "Delete Pnet apply?"
     - Body "Its chat, routines and home folder (notes, inbox and Chrome profile) are deleted. Attached project folders aren't touched."
     - Buttons: "Cancel", and "Delete bot" (`#262626` pill with `#EA4144` text and `lucide:trash-2` 14 `#EA4144`).
   - **"Model picker":** width 320, fill `#1A1A1A`, padding 8.
     - A segmented control at the top (height 30, `#111111`, fully round): "Claude" selected (`#303030` pill, Inter 13, 600, `#F5F5F5`) and "Codex" (Inter 13, `#989898`).
     - Three rows (height 36, radius 8, padding 0 10):
       - "Sonnet (latest)" (Inter 14, `#F5F5F5`) with `lucide:check` (14, `#F5F5F5`) at the right, row fill `#262626`.
       - "Opus (latest)" (Inter 14, `#BDBDBD`).
       - "Haiku (latest)" (Inter 14, `#BDBDBD`).
     - Caption "Latest versions are picked automatically." (Inter 11, `#6E6E6E`).
   - **"Mention popover":** width 280, fill `#1A1A1A`, padding 6. Three rows (height 36, radius 8, padding 0 10, gap 8):
     - The 18 px CV Writer (`0:1137`), "CV Writer" (Inter 13, `#F5F5F5`) and "Working" (Inter 12, `#3FA37A`, right-aligned), row fill `#262626` (highlighted).
     - The 18 px Chief of Staff (`0:1301`), "Chief of Staff" and "Idle" (`#989898`).
     - `lucide:users` (14, `#ACACAC`), "@everyone" and "All 6 bots" (`#989898`).
10. **"Focus states":**
    - An input, width 320, height 44, radius 10, fill `#111111`, with a 2 px `#459FFF` stroke, holding "Pnet apply" (Inter 14, `#F5F5F5`).
    - Next to it, a white "Save" pill (height 32) wrapped in a 2 px `#459FFF` ring 2 px outside the pill. Build the ring as a slightly larger rounded frame behind the pill, with a stroke and no fill.

- [ ] **Step 4: Fit the frame**

Read both columns' heights. Set the frame's height to the taller column's bottom plus 60, and use that height as the frame's longer edge for the exports.

- [ ] **Step 5: Export and check**

Export at 1x (`maxEdge` equal to the frame height) to `docs/wireframes/wf11-chat-states-dark.png`, and at 2x to `docs/wireframes/wf11-chat-states-dark@2x.png`. Read the 1x PNG and check:

- [ ] Every item has its caption, and there's no overlap between items or columns.
- [ ] The long reply wraps inside its bubble.
- [ ] The long handoff row ends in an ellipsis with its chevron visible.
- [ ] Rail states: each badge or dot is readable at 40 px, and "Limited" and "Needs setup" look different.
- [ ] The dialogs' buttons fit on one line.
- [ ] The focus rings are clearly visible.
- [ ] Every color comes from the token table.

- [ ] **Step 6: Save and commit**

`save_file`, then:

```bash
git add design/tjommie-wireframes.fig docs/wireframes/wf11-chat-states-dark.png docs/wireframes/wf11-chat-states-dark@2x.png
git commit -m "Design the Phase 1 chat states sheet

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: WF5 Team room

The room where bot-to-bot exchanges show up as posts and the person asks bots things with @mentions.

**Files:**
- Modify: `design/tjommie-wireframes.fig`
- Create: `docs/wireframes/wf5-team-room-dark.png`, `docs/wireframes/wf5-team-room-dark@2x.png`

**Interfaces:**
- Consumes: "WF1 Main chat, Phase 1 (dark)" (Task 2), and "Mention popover" and "Composer states" from WF11 (Task 3).
- Produces: frame "WF5 Team room (dark)", with layers "Post / <bot>" and "Room panel".

- [ ] **Step 1: Clone and place**

Clone "WF1 Main chat, Phase 1 (dark)", `node_move` it to page (1540, 2000), and rename it "WF5 Team room (dark)".

- [ ] **Step 2: Rail**

In "Rail", `node_move` "Selected bg" so it sits behind "Team room": the same x, and y equal to Team room's y minus 6. Pnet apply is no longer selected.

- [ ] **Step 3: Header**

In "Header chip":
- Delete the avatar instance. In its place, add a 24 px circle frame (fill `#262626`, fully round) holding `lucide:users` (14, `#ACACAC`), centered.
- Set "Bot name" to "Team room".
- In "State", delete the dot and set its label to "6 bots" (`#989898`).

- [ ] **Step 4: Posts**

In "Messages", delete everything except "Timestamp 9:14", and set its text to "Today 9:31 AM". Build the rest in reading order, so every new child lands at the end in the right place. For the second timestamp, clone "Timestamp 9:14" when you reach it; the clone lands at the end.

**Post layout.** A bot post is a horizontal frame "Post / <bot>" (gap 10, fill `#070707`):
- The bot's 28 px avatar.
- A vertical frame (gap 4) holding:
  - A header row (gap 6): the bot name (Inter 13, 600, `#F5F5F5`), an optional "to <bot>" (Inter 13, `#989898`), and the time (Inter 12, `#909090`).
  - A bubble: a clone of the Task 2 bot bubble style, max width 560.

Build these, in order, in "Messages":

1. "Today 9:31 AM".
2. **"Post / Pnet apply":** Pnet apply 28 px (`0:3293`), "to CV Writer", "9:31 AM". Bubble: "Sabenza IT wants Angular and Azure up front. Can you tailor the CV for listing #PN-48213?" Under the bubble, a link row: "Open exchange" (Inter 12, `#459FFF`) and `lucide:chevron-right` (12, `#459FFF`).
3. **"Post / CV Writer":** CV Writer 28 px (`0:4059`), "to Pnet apply", "9:33 AM". Bubble: "Done. CV-Sabenza-Angular.pdf, 2 pages. Moved Azure DevOps to the top." Under it, a clone of "Attachment card".
4. "Today 10:06 AM".
5. **User post:** a clone of "User bubble" (right-aligned), text "@Chief of Staff add the Sabenza IT application to the 5pm digest. @everyone skip senior roles today." Use `set_font_range` to color "@Chief of Staff" and "@everyone" `#1D6FE0` at weight 600.
6. **"Post / Chief of Staff":** Chief of Staff 28 px (`0:4076`), no "to", "10:06 AM". Bubble: "Added to the 5pm digest."
7. **"Post / Careers24 scout":** Careers24 scout 28 px (`0:4680`), "10:07 AM". Bubble: "Noted. I'll skip senior roles for the rest of today."

Re-anchor "Messages" as in Task 2 Step 4 (bottom at 816).

- [ ] **Step 5: Composer and mention popover**

- In "Composer", replace the "Stop" button with the "Typing" round send button from WF11 "Composer states" (white, arrow-up).
- Replace "Placeholder" with the text "Can someone check careers24.com for new React roles? @CV" (Inter 14, `#F5F5F5`).
- Clone WF11 "Mention popover" into "Chat" and place it with its left edge at x 16 and its bottom 8 px above the composer.

- [ ] **Step 6: Room panel**

Rename "Bot panel" to "Room panel". Delete "Bot summary", "Browser card", "Routines", the "Add" text and "Recent activity". Then add, with positions in "Room panel":

1. "Members" (Inter 13, 600, `#9C9C9C`) at (16, 60).
2. **Six rows from y 88, 44 px apart.** Each row has the bot's 24 px avatar at x 16 (ids `0:5138`–`0:5180`), its name (Inter 14, `#F5F5F5`) at x 52, and its state (Inter 12) right-aligned to x 444:

   | Bot | State | State color |
   |---|---|---|
   | Chief of Staff | Idle | `#989898` |
   | Pnet apply | Working | `#3FA37A` |
   | CV Writer | Working | `#3FA37A` |
   | Inbox Triage | Waiting for you | `#459FFF` |
   | Night Shift | Paused | `#989898` |
   | Careers24 scout | Limited until 3:45 PM | `#989898` |

3. At (16, 368): `lucide:info` (14, `#6E6E6E`), then "Bots can ask each other up to 5 times per chain." (Inter 12, `#989898`), then "Change" (Inter 12, `#459FFF`).

- [ ] **Step 7: Export and check**

Export to `docs/wireframes/wf5-team-room-dark.png` (1x, `maxEdge` 1440) and `@2x` (`scale` 2, `maxEdge` 2880). Read the 1x PNG and check:

- [ ] The Team room is selected in the rail.
- [ ] Posts show the avatar, name, "to <bot>" and time, and bubbles stay 560 px or narrower.
- [ ] The two mentions are blue and bold inside the light user bubble.
- [ ] The mention popover sits above the composer without covering the last post's text.
- [ ] The member states use the right colors, and "Limited until 3:45 PM" fits.
- [ ] Every color comes from the token table.

- [ ] **Step 8: Save and commit**

```bash
git add design/tjommie-wireframes.fig docs/wireframes/wf5-team-room-dark.png docs/wireframes/wf5-team-room-dark@2x.png
git commit -m "Design the Team room

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: WF6 Exchange view

What opens when the person clicks a handoff row: the whole exchange between two bots, in the right panel, with the chat still visible.

**Files:**
- Modify: `design/tjommie-wireframes.fig`
- Create: `docs/wireframes/wf6-exchange-view-dark.png`, `docs/wireframes/wf6-exchange-view-dark@2x.png`

**Interfaces:**
- Consumes: "WF1 Main chat, Phase 1 (dark)" (Task 2), the WF11 composer states (Task 3), and the avatar ids.
- Produces: frame "WF6 Exchange view (dark)", with layer "Exchange panel".

- [ ] **Step 1: Clone and place**

Clone "WF1 Main chat, Phase 1 (dark)", `node_move` it to page (3080, 2000), and rename it "WF6 Exchange view (dark)".

- [ ] **Step 2: Selected handoff row**

In "Messages", give "Handoff row / Pnet apply to CV Writer" fill `#111111` and a `#303030` stroke. That's the selected row.

- [ ] **Step 3: Exchange panel**

Rename "Bot panel" to "Exchange panel". Delete "Bot summary", "Browser card", "Routines", the "Add" text and "Recent activity". Then build, with positions in "Exchange panel":

1. **Header:**
   - Pnet apply 24 px (`0:5147`) at (16, 60).
   - `lucide:arrow-right` (14, `#909090`) at (46, 65).
   - CV Writer 24 px (`0:5154`) at (66, 60).
   - "Pnet apply asked CV Writer" (Inter 15, 600, `#FCFCFC`) at (100, 56).
   - "Today 9:31 AM · Replied after 2 minutes" (Inter 12, `#989898`) at (100, 78).
   - `lucide:x` (16, `#ACACAC`) at (428, 62).
2. A divider: RECTANGLE 428 × 1, `#252525`, at (16, 104).
3. **"Exchange messages":** a vertical auto-layout frame, gap 12, width 428, fill `#070707`, at (16, 120). In order:
   - An author row (gap 6): the 18 px Pnet apply (`0:1132`) and "Pnet apply" (Inter 12, 600, `#989898`).
   - A bubble (`#262626`, radius 18, padding 12 14, max width 400, Inter 14/21 `#F5F5F5`): "Sabenza IT wants Angular and Azure up front. Can you tailor the CV for listing #PN-48213?"
   - A clone of "Activity row (collapsed)" with the text "3 steps: Wrote CV-Sabenza-Angular.docx and saved a PDF".
   - An author row with the 18 px CV Writer (`0:1137`) and "CV Writer".
   - A bubble: "Done. CV-Sabenza-Angular.pdf, 2 pages. Moved Azure DevOps to the top."
   - A clone of "Attachment card".
   - A caption (Inter 12, `#909090`): "Sent back to Pnet apply. The PDF is in Pnet apply's inbox."
4. **Panel composer** at (16, 840): a clone of "Composer", resized to width 428.
   - Replace its "Placeholder" with two chips, then the text "Reply in the Team room" (Inter 14, `#6E6E6E`). Each chip: fill `#262626`, fully round, height 26, padding 0 8, gap 6, holding the 18 px avatar plus the name in Inter 12 `#F5F5F5`. The chips are "@Pnet apply" and "@CV Writer".
   - The round button is the disabled send style from WF11.

- [ ] **Step 4: Export and check**

Export to `docs/wireframes/wf6-exchange-view-dark.png` and `@2x`. Read the 1x PNG and check:

- [ ] The selected handoff row stands out from the other handoff row.
- [ ] The panel header, conversation and composer fit inside the 460 px panel with 16 px margins.
- [ ] The two mention chips and the placeholder share one line.
- [ ] Every color comes from the token table.

- [ ] **Step 5: Save and commit**

```bash
git add design/tjommie-wireframes.fig docs/wireframes/wf6-exchange-view-dark.png docs/wireframes/wf6-exchange-view-dark@2x.png
git commit -m "Design the exchange view

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: WF7 Routine editor

A dialog for creating and editing a routine, with the routine's recent runs.

**Files:**
- Modify: `design/tjommie-wireframes.fig`
- Create: `docs/wireframes/wf7-routine-editor-dark.png`, `docs/wireframes/wf7-routine-editor-dark@2x.png`

**Interfaces:**
- Consumes: "WF1 Main chat, Phase 1 (dark)" (Task 2).
- Produces: frame "WF7 Routine editor (dark)", with layer "Routine editor".

- [ ] **Step 1: Clone, place and dim**

1. Clone "WF1 Main chat, Phase 1 (dark)", `node_move` it to page (0, 3000), and rename it "WF7 Routine editor (dark)".
2. Add a RECTANGLE "Scrim", 1440 × 900 at (0, 0) in the frame, fill `#000000`, opacity 0.6, on top of everything.

- [ ] **Step 2: Dialog shell**

Add a FRAME "Routine editor" at (440, 90) in the frame: 560 × 720, fill `#111111`, radius 16, 1 px `#252525` stroke, on top of the scrim. Positions below are in "Routine editor".

- [ ] **Step 3: Dialog contents**

1. **Header:**
   - "Edit routine" (Inter 17, 600, `#FCFCFC`) at (24, 24).
   - "On" (Inter 13, `#BDBDBD`) at (432, 26).
   - An on switch at (456, 24): a 36 × 20 frame, fill `#F5F5F5`, fully round, holding a 16 px `#070707` knob at its right (2 px inset).
   - `lucide:x` (16, `#ACACAC`) at (520, 26).
2. **Name:**
   - The label "Name" (Inter 13, `#989898`) at (24, 64).
   - An input at (24, 86), 512 × 44: radius 10, fill `#070707`, and a **2 px `#459FFF` focus stroke** (the Review Focus item).
   - The text "Pnet apply 9am" (Inter 14, `#F5F5F5`) at a 16 px inset.
3. **What to do:**
   - The label "What to do" at (24, 146).
   - A textarea at (24, 168), 512 × 96: radius 10, fill `#070707`, 1 px `#2A2A2A` stroke.
   - The text (Inter 14/21, `#F5F5F5`, width 480, 16 px inset): "Check pnet.co.za for new Full Stack and React roles in Johannesburg or remote, and apply to the ones that fit. Skip senior roles."
4. **Days:**
   - The label "Days" at (24, 280).
   - Seven chips at y 302 from x 24, each 40 × 32, fully round, 8 apart. Labels: M, T, W, T, F, S, S.
   - Monday to Friday are selected: fill `#F5F5F5`, Inter 13, 600, `#070707`.
   - Saturday and Sunday are not: fill `#262626`, Inter 13, `#989898`.
5. **Times:**
   - The label "Times" at (24, 350).
   - A chip "8:48 AM" at (24, 372): fill `#262626`, fully round, height 32, padding 0 12, gap 6, Inter 13 `#F5F5F5` plus `lucide:x` (12, `#989898`).
   - After it, 8 px apart, an "Add time" chip: fill `#070707`, 1 px `#303030` stroke, fully round, height 32, `lucide:plus` (12, `#BDBDBD`) plus "Add time" (Inter 13, `#BDBDBD`).
6. **Time zone:** at (24, 416), `lucide:globe` (12, `#6E6E6E`), then "Times are in South Africa Standard Time (Africa/Johannesburg)." (Inter 12, `#989898`).
7. **Stop after:**
   - The label "Stop after" at (24, 446).
   - A segmented control at (24, 468): height 32, fill `#1A1A1A`, fully round. Options: "No limit", "1 hour", "2 hours" and "3 hours".
   - "3 hours" is selected: a `#303030` pill with Inter 13, 600, `#F5F5F5`. The others are Inter 13 `#989898`.
8. **Divider:** a RECTANGLE 512 × 1, `#252525`, at (24, 520).
9. **Recent runs header:** "Recent runs" (Inter 13, 600, `#9C9C9C`) at (24, 536). "Last 20 runs are kept" (Inter 12, `#6E6E6E`) right-aligned to x 536 at y 537.
10. **Five run rows**, from y 566 and 26 apart. Each row has an icon (14) at x 24, the date (Inter 13, `#BDBDBD`) at x 48, the status (Inter 13) at x 200, and the duration (Inter 12, `#909090`) right-aligned to x 536:

    | Icon | Date | Status | Status color | Duration |
    |---|---|---|---|---|
    | `lucide:check` `#3FA37A` | Today 8:48 AM | Done | `#BDBDBD` | 2h 51m |
    | `lucide:timer-off` `#989898` | Yesterday 8:48 AM | Stopped at the time limit | `#989898` | 3h |
    | `lucide:circle-alert` `#989898` | Mon 8:48 AM | Failed: Claude stopped unexpectedly | `#989898` | 4m |
    | `lucide:circle-dashed` `#6E6E6E` | Fri 8:48 AM | Missed: the PC was off | `#6E6E6E` | (none) |
    | `lucide:pause` `#6E6E6E` | Thu 8:48 AM | Skipped: Pnet apply was paused | `#6E6E6E` | (none) |

11. **Footer:**
    - A divider RECTANGLE 512 × 1, `#252525`, at (24, 664).
    - `lucide:trash-2` (14, `#EA4144`) plus "Delete routine" (Inter 13, `#EA4144`) at (24, 684).
    - "Cancel" (`#262626` pill, height 32) and "Save" (white pill, height 32, Inter 13, 600, `#070707`), right-aligned to x 536 at y 676, 8 apart.

- [ ] **Step 4: Export and check**

Export to `docs/wireframes/wf7-routine-editor-dark.png` and `@2x`. Read the 1x PNG and check:

- [ ] The dialog is centered, fully inside the frame, and on top of the scrim.
- [ ] Every row fits its 512 px content width; the long run statuses don't collide with the durations.
- [ ] The focused Name input shows a clear blue ring.
- [ ] Selected and unselected days and times are easy to tell apart.
- [ ] Every color comes from the token table.

- [ ] **Step 5: Save and commit**

```bash
git add design/tjommie-wireframes.fig docs/wireframes/wf7-routine-editor-dark.png docs/wireframes/wf7-routine-editor-dark@2x.png
git commit -m "Design the routine editor

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: WF3 New bot, Phase 1

The New bot screen with the settings rows Phase 1 actually has: model, folder, autonomy, browser, who it can message, and routines.

**Files:**
- Modify: `design/tjommie-wireframes.fig`
- Create: `docs/wireframes/wf3-phase1-new-bot-dark.png`, `docs/wireframes/wf3-phase1-new-bot-dark@2x.png`

**Interfaces:**
- Consumes: frame "WF3 New bot (dark)" (`0:2507`) and its "Form" rows (`Setting / …`).
- Produces: frame "WF3 New bot, Phase 1 (dark)", with rows "Setting / Model", "Setting / Folder", "Setting / Autonomy (expanded)", "Setting / Browser", "Setting / Can message" and "Setting / Routines". Task 8 clones this frame.

- [ ] **Step 1: Clone and place**

Clone `0:2507`, `node_move` it to page (0, 2000), and rename it "WF3 New bot, Phase 1 (dark)".

- [ ] **Step 2: Rail**

In "Rail", delete "Icon / lucide:store", then move "Icon / lucide:plus", its "Selected bg" and "Icon / lucide:layout-grid" 44 px down.

- [ ] **Step 3: Rows**

The rows sit at absolute positions inside "Form" (Form-relative y): Model 300, Chrome profile 364, Logins 428, Computer access (expanded, 128 tall) 492, Can message 628, Routines 692. Confirm these with `get_node` before moving anything. Then:

1. **"Setting / Model":** set its value text to "Claude · Sonnet (latest)".
2. **"Setting / Chrome profile":** rename it "Setting / Folder". Swap its icon for `lucide:folder` (18, `#ACACAC`), set the title to "Folder" and the value to "Its own folder". Keep the chevron. It stays at y 364.
3. **"Setting / Computer access (expanded)":** rename it "Setting / Autonomy (expanded)" and `node_move` it to y 428.
   - Swap its icon for `lucide:shield` and set its title to "Autonomy".
   - In its segmented control, set the three labels to "Always ask", "Own folder" and "Full auto". Move the selected pill behind "Own folder", and swap the text styles so "Own folder" is `#F5F5F5` and the others `#989898`.
   - Set the helper text to "Own folder: works freely inside its own folder and asks before anything else. Claude bots also ask before each shell command."
4. **"Setting / Logins":** rename it "Setting / Browser" and `node_move` it to y 564.
   - Swap its icon for `lucide:globe` and set its title to "Browser".
   - Delete the "Choose" link and the site text.
   - At the right, add "Its own Chrome profile" (Inter 13, `#989898`), then an on switch 12 px after it (36 × 20, fill `#F5F5F5`, fully round, with a 16 px `#070707` knob at the right).
5. "Setting / Can message" (628) and "Setting / Routines" (692) stay as they are.

- [ ] **Step 4: Export and check**

Export to `docs/wireframes/wf3-phase1-new-bot-dark.png` and `@2x`. Read the 1x PNG and check:

- [ ] The rows read, in order: Model, Folder, Autonomy (expanded), Browser, Can message, Routines. Each is 8 px from the next, with no overlaps.
- [ ] There's no Chrome profile row, Logins row, "Computer access" or "This PC / Virtual machine" left anywhere.
- [ ] The helper text fits inside the expanded row.
- [ ] The rail cluster has no store icon.
- [ ] Every color comes from the token table.

- [ ] **Step 5: Save and commit**

```bash
git add design/tjommie-wireframes.fig docs/wireframes/wf3-phase1-new-bot-dark.png docs/wireframes/wf3-phase1-new-bot-dark@2x.png
git commit -m "Design the Phase 1 New bot screen

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: WF8 Bot settings

Editing an existing bot: the same rows as New bot, plus pause, delete and a view of its folder and notes.

**Files:**
- Modify: `design/tjommie-wireframes.fig`
- Create: `docs/wireframes/wf8-bot-settings-dark.png`, `docs/wireframes/wf8-bot-settings-dark@2x.png`

**Interfaces:**
- Consumes: "WF3 New bot, Phase 1 (dark)" (Task 7) and avatar ids.
- Produces: frame "WF8 Bot settings (dark)", with layers "About this bot" and "Action bar".

- [ ] **Step 1: Clone and place**

Clone "WF3 New bot, Phase 1 (dark)", `node_move` it to page (1540, 3000), and rename it "WF8 Bot settings (dark)".

- [ ] **Step 2: Rail**

`node_move` "Selected bg" behind the rail's Pnet apply avatar (6 px outside it on each side). Nothing else in the rail is selected.

- [ ] **Step 3: Header and identity**

1. **"Header chip":** delete its preview avatar and put in its place a 24 px Pnet apply (`0:5147`). Set its name text to "Pnet apply", then add a state group after it: a 6 px `#3FA37A` dot and "Working" (Inter 13, `#3FA37A`), gap 6.
2. **Avatar tile:** in "Form" > "Avatar stage", delete the 96 px placeholder blob. Put a 40 px Pnet apply instance (component `0:575`) in the center of the tile.
3. **Pickers:**
   - In "Colors", move the selection ring from the blue swatch to the orange swatch.
   - In "Shapes", move the selected-tile style (`#303030` fill and 1 px `#459FFF` stroke) to the cloud tile.
4. **"Name input":** keep "Pnet apply".

- [ ] **Step 4: About this bot**

In "Main":
- Set "Suggestions label" to "About this bot".
- Delete the three suggestion cards inside "Suggestions". Replace them with one card: width 340, padding 16, gap 12, radius 14, fill `#111111`, 1 px `#1C1C1C` stroke, vertical. Its rows are:
  1. "Created" (Inter 12, `#989898`) above "Monday 22 September" (Inter 13, `#F5F5F5`).
  2. "Home folder" (Inter 12, `#989898`), then a row with `lucide:folder` (14, `#ACACAC`) and "Open folder" (Inter 13, `#459FFF`).
  3. "Notes" (Inter 12, `#989898`).
  4. A notes preview: fill `#070707`, radius 10, padding 12, holding Inter 12/18 `#BDBDBD` text, width 284:

     ```
     # Pnet apply's notes
     - Skip senior roles.
     - Prefers Johannesburg or remote.
     - CV Writer tailors the CV when a listing wants Angular or Azure.
     ```

  5. "Open notes.md" (Inter 13, `#459FFF`).

- [ ] **Step 5: Action bar**

In "Action bar":
- At the left, 24 px in: `lucide:trash-2` (14, `#EA4144`) plus "Delete bot" (Inter 13, `#EA4144`).
- 24 px to the right of that: a `#262626` pill (height 32, padding 0 14) holding `lucide:pause` (14, `#F5F5F5`) plus "Pause bot" (Inter 13, `#F5F5F5`).
- At the right: keep "Cancel". Relabel the white "Create bot" button to "Save changes".

- [ ] **Step 6: Export and check**

Export to `docs/wireframes/wf8-bot-settings-dark.png` and `@2x`. Read the 1x PNG and check:

- [ ] The header chip reads "Pnet apply" and "Working".
- [ ] The orange swatch and the cloud tile are selected.
- [ ] The "About this bot" card fits within its 340 px column, and the notes preview wraps cleanly.
- [ ] The action bar has "Delete bot" and "Pause bot" at the left and "Cancel" and "Save changes" at the right, with nothing overlapping.
- [ ] Every color comes from the token table.

- [ ] **Step 7: Save and commit**

```bash
git add design/tjommie-wireframes.fig docs/wireframes/wf8-bot-settings-dark.png docs/wireframes/wf8-bot-settings-dark@2x.png
git commit -m "Design the bot settings screen

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: WF9 First-run setup

The first thing a new user sees: signing Tjommie's own Claude and Codex accounts in. It shows every state a provider card can be in.

**Files:**
- Modify: `design/tjommie-wireframes.fig`
- Create: `docs/wireframes/wf9-first-run-dark.png`, `docs/wireframes/wf9-first-run-dark@2x.png`

**Interfaces:**
- Consumes: "WF1 Main chat, Phase 1 (dark)" (Task 2), for its rail and caption buttons.
- Produces: frame "WF9 First-run setup (dark)", with layers "Card / Claude signed in", "Card / Codex signing in", "Card / Codex not signed in" and "Card / Claude not installed".

- [ ] **Step 1: Clone, place and empty**

1. Clone "WF1 Main chat, Phase 1 (dark)", `node_move` it to page (3080, 3000), and rename it "WF9 First-run setup (dark)".
2. **Caption buttons:** reparent the three caption-button icons ("Icon / lucide:minus", "Icon / lucide:square" and "Icon / lucide:x") from "Bot panel" to the frame itself, keeping their page positions.
3. Delete "Chat" and "Bot panel".
4. **Rail:**
   - Delete the six bot avatars, "Live dot", "Waiting dot", "Limited badge" and "Selected bg".
   - Set "Team room" to opacity 0.4. There are no bots yet, so it's inactive.
   - Keep the divider and the bottom cluster.

- [ ] **Step 2: Intro**

With positions in the frame:
- "Set up Tjommie" (Inter 20, 600, `#FCFCFC`) at (384, 120).
- At (384, 156), width 760, Inter 14/21 `#989898`: "Tjommie runs your bots with Claude Code and Codex on this PC. Sign in to each one once. Tjommie keeps its own sign-in, separate from how you use these tools yourself, and never sees your password."

- [ ] **Step 3: Provider cards**

**Card shell.** A card is 370 wide, with padding 20, radius 14, fill `#111111`, and a 1 px `#1C1C1C` stroke. Its header row holds:
- A 36 px icon tile (fill `#262626`, radius 10), holding a centered icon (18, `#F5F5F5`).
- 16 px to the right, the name (Inter 15, 600, `#FCFCFC`), with the version (Inter 12, `#989898`) under it.

Positions below are in each card.

1. **"Card / Claude signed in"** at (384, 224), 370 × 200.
   - Icon `lucide:sparkles`; name "Claude Code"; version "Version 2.1.280".
   - At (20, 88): `lucide:circle-check` (16, `#3FA37A`), then "Signed in as tyrel.jobs@gmail.com" (Inter 13, `#F5F5F5`) at x 44, and "Max plan" (Inter 12, `#989898`) at (44, 110).
   - "Use a different account" (Inter 13, `#459FFF`) at (20, 156).
2. **"Card / Codex signing in"** at (774, 224), 370 × 200.
   - Icon `lucide:terminal`; name "Codex"; version "Version 0.155.1".
   - At (20, 88): `lucide:loader` (16, `#ACACAC`), then "Finish signing in in your browser" (Inter 13, `#F5F5F5`) at x 44, and "Tjommie notices as soon as you're done." (Inter 12, `#989898`) at (44, 110).
   - At (20, 148): "Open the sign-in page again" (`#262626` pill, height 32, Inter 13 `#F5F5F5`), then 16 px after it "Cancel" (Inter 13, `#989898`).
3. "Other states" (Inter 13, 600, `#9C9C9C`) at (384, 452).
4. **"Card / Codex not signed in"** at (384, 480), 370 × 150.
   - Icon `lucide:terminal`; name "Codex"; version "Version 0.155.1".
   - "Opens OpenAI's sign-in page in your browser." (Inter 12, `#989898`) at (20, 76).
   - A white pill "Sign in to Codex" (height 32, Inter 13, 600, `#070707`) at (20, 102).
5. **"Card / Claude not installed"** at (774, 480), 370 × 150.
   - Icon `lucide:sparkles`; name "Claude Code"; "Not installed" (Inter 12, `#989898`) in place of the version.
   - "Install Claude Code from claude.com/claude-code, then check again." (Inter 12, `#989898`, width 330) at (20, 76), with "claude.com/claude-code" colored `#459FFF` (`set_font_range`).
   - A "Check again" `#262626` pill (height 32) at (20, 102).

- [ ] **Step 4: Footer**

- A white pill "Create your first bot" (height 40, padding 0 18, Inter 14, 600, `#070707`) at (384, 670).
- "You can sign in to the other one later in Settings." (Inter 12, `#989898`) at (384, 722).

- [ ] **Step 5: Export and check**

Export to `docs/wireframes/wf9-first-run-dark.png` and `@2x`. Read the 1x PNG and check:

- [ ] The four cards line up in a 2 × 2 grid, with the same card widths and header positions.
- [ ] Each card's state is obvious at a glance: signed in (green check), signing in (spinner), not signed in (white button) and not installed (link plus Check again).
- [ ] The rail shows no bots, an inactive Team room, and the bottom cluster.
- [ ] The caption buttons are at the top right.
- [ ] Every color comes from the token table.

- [ ] **Step 6: Save and commit**

```bash
git add design/tjommie-wireframes.fig docs/wireframes/wf9-first-run-dark.png docs/wireframes/wf9-first-run-dark@2x.png
git commit -m "Design the first-run sign-in screen

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: WF10 Settings: Bots

The Phase 1 Settings page, which has one real setting so far: how many times bots may ask each other per chain.

**Files:**
- Modify: `design/tjommie-wireframes.fig`
- Create: `docs/wireframes/wf10-settings-bots-dark.png`, `docs/wireframes/wf10-settings-bots-dark@2x.png`

**Interfaces:**
- Consumes: frame "WF4 Logins (dark)" (`0:2800`).
- Produces: frame "WF10 Settings: Bots (dark)", with layer "Chain limit".

- [ ] **Step 1: Clone and place**

Clone `0:2800`, `node_move` it to page (0, 4000), and rename it "WF10 Settings: Bots (dark)".

- [ ] **Step 2: Rail and navigation**

1. In "Rail", delete "Icon / lucide:store" and move "Icon / lucide:plus" and "Icon / lucide:layout-grid" 44 px down. Settings stays selected.
2. In "Settings nav" > "Items":
   - Delete the items Logins, Chrome profiles, Models, Notifications and Privacy.
   - Move the selected-item fill (`#1F1F1F`) from Logins to Bots, and give "Bots" text `#F5F5F5` and "General" text `#BDBDBD`.

- [ ] **Step 3: Content**

1. Delete "Detail drawer", "Actions", "Search" and "Logins list". Keep "Caption buttons".
2. In "Header", set the title to "Bots" and delete the security line and its icon.
3. At (332, 112) in the frame: "Bots asking each other" (Inter 13, 600, `#9C9C9C`).
4. **"Chain limit"** card at (332, 138) in the frame: 700 × 108, fill `#111111`, radius 12. Positions below are in the card.
   - "Requests between bots per chain" (Inter 14, `#F5F5F5`) at (16, 16).
   - At (16, 42), width 520, Inter 12/18 `#989898`: "A chain starts with each message you send and each routine run. When a chain reaches this many bot-to-bot requests, the next request stops and the bot asks you instead."
   - A stepper at (564, 16): 120 × 32, fill `#1A1A1A`, fully round. It holds `lucide:minus` (14, `#ACACAC`) at the left, "5" (Inter 14, 600, `#F5F5F5`) centered, and `lucide:plus` (14, `#ACACAC`) at the right.

The General page isn't drawn. It has one row, "Appearance", with a segmented control of Dark, Light and System, styled like the Stop after control in WF7. Plan 1D builds it from that description.

- [ ] **Step 4: Export and check**

Export to `docs/wireframes/wf10-settings-bots-dark.png` and `@2x`. Read the 1x PNG and check:

- [ ] The nav shows only General and Bots, with Bots selected.
- [ ] The card's helper text wraps within 520 px without touching the stepper.
- [ ] Nothing from the Logins screen is left.
- [ ] Every color comes from the token table.

- [ ] **Step 5: Save and commit**

```bash
git add design/tjommie-wireframes.fig docs/wireframes/wf10-settings-bots-dark.png docs/wireframes/wf10-settings-bots-dark@2x.png
git commit -m "Design the Phase 1 Settings page

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: Write the Phase 1 screens into the wireframe spec

Plan 1D reads `docs/design/wireframe-spec.md`, so the new frames and their rules go there too.

**Files:**
- Modify: `docs/design/wireframe-spec.md` (append a section)

**Interfaces:**
- Consumes: the frames and exports from Tasks 2 to 10.
- Produces: the section "Phase 1 screens (plan 1C)" in `docs/design/wireframe-spec.md`.

- [ ] **Step 1: Append the section**

Append this to the end of `docs/design/wireframe-spec.md`:

````markdown
## Phase 1 screens (plan 1C, 2026-09-25)

Frames on the "Wireframes" page of `design/tjommie-wireframes.fig`, exported to `docs/wireframes/`:

| Frame | Export | What it shows |
|---|---|---|
| WF1 Main chat, Phase 1 (dark) | wf1-phase1-main-chat-dark.png | Phase 1 chat: routine label, activity row, approval card (Allow once, Always allow, Deny), Stop button, bot panel (summary, Browser with Show window, Routines with Add, Recent activity) |
| WF3 New bot, Phase 1 (dark) | wf3-phase1-new-bot-dark.png | Settings rows: Model, Folder, Autonomy (Always ask, Own folder, Full auto), Browser, Can message, Routines |
| WF5 Team room (dark) | wf5-team-room-dark.png | Bot-to-bot posts ("to <bot>", Open exchange), @mentions in the person's bubble, mention popover, members with states |
| WF6 Exchange view (dark) | wf6-exchange-view-dark.png | An opened exchange in the right panel; the composer replies in the Team room with both bots mentioned |
| WF7 Routine editor (dark) | wf7-routine-editor-dark.png | Name, what to do, days, times, time zone, stop after, on switch, last runs (Done, Stopped, Failed, Missed, Skipped) |
| WF8 Bot settings (dark) | wf8-bot-settings-dark.png | Edit rows, About this bot (created, home folder, notes.md preview), Delete bot, Pause bot, Save changes |
| WF9 First-run setup (dark) | wf9-first-run-dark.png | Provider cards: signed in, signing in, not signed in, not installed; Create your first bot |
| WF10 Settings: Bots (dark) | wf10-settings-bots-dark.png | Chain limit stepper (default 5); General holds Appearance (Dark, Light, System) |
| WF11 Chat states (dark) | wf11-chat-states-dark.png | Every message, card, row, composer, rail and dialog state |

Rules these frames settle:
- Rail states:
  - Working: green dot, bottom right.
  - Waiting for you: blue dot, top right.
  - Paused: 50% opacity.
  - Limited: 50% opacity plus an hourglass badge.
  - Needs setup: a plug badge.
- The header chip shows the state in words ("Working", "Limited until 3:45 PM").
- The composer's round button:
  - Disabled send (`#3A3A3A`) when the field is empty and the bot is idle.
  - White send when there's text. Sending while the bot works steers it.
  - White Stop when the field is empty and the bot works.
- Approval cards offer Allow once, Always allow and Deny. Routine proposals offer Add routine and Dismiss.
- Answered approvals collapse to one line.
- Handoff rows: pending (spinner), replied (message count), failed (no reply), incoming ("<bot> asked: …"), room answer ("Answered in the Team room"). Long rows truncate before the chevron.
- Focus: a 2 px `#459FFF` ring on inputs and buttons.
- Phase 1 hides: store icon, Share, mic and voice, live screen and computer controls, logins, usage numbers.
````

- [ ] **Step 2: Final save and a look at everything**

1. `save_file` (`path: "design/tjommie-wireframes.fig"`).
2. Export the whole page once with `export_image` (no `ids`, `maxEdge` 2400), look at it, and check that no new frame overlaps another.

- [ ] **Step 3: Commit**

```bash
git add docs/design/wireframe-spec.md design/tjommie-wireframes.fig
git commit -m "Document the Phase 1 screens in the wireframe spec

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```
