# Grokbot wireframes: spec

**Browser interaction update (owner request, 2026-10-22):** The Phase 1 external "Show window" action below is superseded by an interactive browser inside the right panel. The panel uses the existing app tokens and provides tabs, address/navigation controls, Take control / Resume bot, and the live page. Closing it returns control to the bot without closing its persistent browser. The vault and general desktop controls remain later work. See `docs/superpowers/plans/2026-10-22-phase1-gaps-and-embedded-browser.md` and `docs/screenshots/phase1-embedded-browser.png`; older exported wireframes remain historical references.

Decisions from the user (2026-09-25):
- Navigation: icon rail of bots (like `docs/reference/ref-3-pnet-apply-dark.png`), Team room pinned at the TOP of the rail.
- Computer panel: thumbnail + details in the right panel (live screen, Take over / Pause / Stop, target, Chrome profile, Routines, Logins summary). Click the screen to expand.
- Bot-to-bot: BOTH. One-line handoff rows inside each bot's chat that open the full exchange, plus the Team room thread.
- Screens this round: WF1 Main chat + computer, WF2 Agents overview, WF3 New bot creator, WF4 Logins + Chrome profile.
- Fidelity: mid-fi (real layout, real copy, app colors, bot avatars; simple shapes stand in for images).
- Theme: both. WF1 in dark and light; WF2 to WF4 in dark.
- Window: Windows 11, caption buttons (minimize, maximize, close) at the top right. Frames 1440x900.
- Icons for bots: the new icon system is still being explored. Use the current blob avatars as placeholders, built as components ("Bot avatar / <bot>") so they can be swapped later.

Tokens, measurements, roster, copy and blob shapes: `docs/design/brief.md` (sections 3 to 6). Font: Inter.

## WF1 Main chat + computer (dark)

Frame "WF1 Main chat + computer (dark)", 1440x900, fill #070707.

**Rail** (x 0, w 88, full height, fill #111111, right edge 1px #080808):
- Top padding 16. Team room: 40px rounded-square (radius 12) fill #262626 with three 14px mini blobs overlapping inside (purple circle, orange cloud, green rounded square). Under it a 32px-wide 1px #252525 divider.
- Bots, 40px avatars, 12px apart, centered: Chief of Staff (purple circle), Pnet apply (orange cloud, SELECTED: inside a 52px #303030 radius-12 square; small green 10px live dot with 2px #111111 ring at bottom-right), CV Writer (green rounded square), Inbox Triage (pink cloud; 10px #459FFF waiting dot top-right with #111111 ring), Night Shift (amber hexagon at 50% opacity = paused), Careers24 scout (teal droplet).
- Bottom: "+" (lucide:plus, #ACACAC, 20px) for new bot, store (lucide:store), then the user avatar (32px circle #777777 with "T" in white Inter 14 semibold).

**Chat column** (x 88 to 980, w 892):
- Header (h 56): centered chip (#181818, full radius, h 40, px 14): 24px Pnet apply avatar + "Pnet apply" (Inter 15 semibold #FCFCFC) + 8px gap + "Live" in #3FA37A 13px with a 6px green dot. Right side of header: lucide:monitor (toggle computer panel, #ACACAC, highlighted state since panel is open) and lucide:share (#ACACAC), 32px hit areas.
- Messages, left padding 24, bubbles max width 600, 12px vertical rhythm:
  1. Timestamp centered "Today 9:14 AM" (Inter 12 #909090).
  2. Bot bubble (#262626, radius 18, p 14x16, text Inter 14/21 #F5F5F5): "Morning window started. 6 new listings match Full Stack or React in Johannesburg and remote."
  3. Handoff row (no bubble; container radius 10, 1px #252525 stroke, px 12 py 8, h 40): two 18px mini avatars (Pnet apply then CV Writer) with a lucide:arrow-right 12px #909090 between them, text "Pnet apply asked CV Writer to tailor your CV for Sabenza IT" (Inter 13 #BDBDBD), then "2 messages" (#909090) and lucide:chevron-right.
  4. Attachment card (#262626, radius 12, p 12, w 320): 36px rounded square #3A2020 with lucide:file-text #EA4144, "CV-Sabenza-Angular.pdf" (Inter 14 medium #F5F5F5), "2 pages, 184 KB" (Inter 12 #989898), lucide:download #ACACAC at right.
  5. Bot bubble: "Applied to Full Stack Developer at Sabenza IT (Pretoria). Pnet showed the confirmation page."
  6. User bubble, RIGHT aligned (#F5F5F5, radius 18, text #070707): "Nice. Skip senior roles for the rest of today."
  7. Handoff row: Chief of Staff mini avatar + "Chief of Staff added this to your 5pm digest".
  8. Timestamp "Today 10:02 AM".
  9. Waiting card (radius 14, fill #101722, 1px #1E3A5F stroke, p 16, w 600): label "Waiting for you" (Inter 12 semibold #459FFF, lucide:circle-alert 14px), body "Pnet sent a one-time PIN to your phone. Take over to enter it, or reply with the code." (Inter 14 #F5F5F5), buttons: "Take over" (white pill #F5F5F5, text #070707, h 32, px 14, lucide:hand 14px) and "Reply with code" (#262626 pill, text #F5F5F5).
- Composer at the bottom (y 900 - 16 - 44): x 16 to 876 inside the column, h 44, radius 22, fill #2F2F2F: left 28px circle "+" (#3A3A3A, lucide:plus #ACACAC), placeholder "Message Pnet apply" (Inter 14 #6E6E6E), right lucide:mic #ACACAC and a 32px white circle with lucide:audio-lines #070707.

**Right panel** (x 980 to 1440, w 460, fill #070707, left 1px #252525):
- Top bar (h 44): lucide:settings and lucide:chevrons-right (#ACACAC) at the left of the bar area, Windows caption buttons at the far right: lucide:minus, lucide:square (14px), lucide:x, #ACACAC, 46px apart.
- Live screen (x 16, w 428, h 268, radius 10, fill #282828, overflow hidden): a simplified Chrome window: tab strip (h 28, #1C1C1C) with one tab "Full Stack Developer | Pnet" and a 14px profile circle (orange) at the right; address bar (h 28, #262626 pill inside a #202020 toolbar) with lucide:lock 11px and "pnet.co.za/jobs/full-stack-developer-sabenza" (Inter 11 #BDBDBD); page body (#F4F5F7): a dark blue top strip (#0E2A47, h 22), job title "Full Stack Developer" (Inter 13 semibold #111), "Sabenza IT, Pretoria" (Inter 11 #555), two input boxes (white, 1px #D0D5DD, radius 4, h 24) labelled "Email" and "Mobile", and a blue "Apply now" button (#1D6FE0, radius 4). The bot cursor: lucide:mouse-pointer-2 16px (#070707 with white outline feel) near the Apply button plus a small orange (#EC712E) pill tag "Pnet apply" (Inter 10 semibold white).
- Under the screen, caption row (h 32): left "Pnet apply's screen" (Inter 12 #909090); right a segmented control (h 26, #1A1A1A, radius 13): "This PC" selected (#303030 pill, #F5F5F5 text 12) and "Virtual machine" (#989898 12).
- Status + controls row: left "Live" with a 8px #3FA37A dot and "Controlling Chrome" (Inter 13 #F5F5F5 / #989898); right three pills h 32: "Take over" (white pill, lucide:hand), "Pause" (#262626, lucide:pause), "Stop" (transparent, 1px #EA4144 stroke, #EA4144 text, lucide:square 12px).
- Chrome profile row (card #111111, radius 12, p 12): Chrome logo (logos:chrome 20px) + "Chrome profile: Pnet apply" (Inter 13 semibold) + "Isolated from your personal Chrome. Signed in to 3 sites." (Inter 12 #989898) + lucide:chevron-right.
- "Routines" label (Inter 13 semibold #9C9C9C), then three rows (lucide:clock 16px #3FA37A, title Inter 13 #FCFCFC, schedule Inter 12 #989898): "Pnet apply 9am (3h)" / "Weekdays at 8:48 AM"; "Pnet apply 1pm (3h)" / "Weekdays at 12:47 PM"; "Pnet apply 5pm (3h)" / "Weekdays at 4:45 PM" (this one uses lucide:pause #989898 and "Paused").
- Logins row (card #111111, radius 12): lucide:key-round #ACACAC + "4 saved logins" (Inter 13) + "Manage" (Inter 13 #459FFF) at right.

## WF1 light (later, after the dark version is approved)

Same layout with light tokens from brief section 3: rail #F7F7F7, canvas #FCFCFC, bot bubble #EFEFEF, user bubble #070707 with white text, dividers #E4E4E4, composer white with 1px #EAEAEA and a soft shadow, selected rail #E1E1E1, accent #3B82F6.

## Round 2 decisions (2026-09-25)

- Agents overview: card grid. Card content (priority order): live screen thumbnail (dimmed avatar placeholder when not live), state + current task, next routine, "talking with" mini avatars.
- New bot: full page with sections (like ref 1) plus Suggestions.
- Logins: one shared vault, per-bot access per login.

Shared rail for WF2 to WF4 = the WF1 rail, plus a bottom cluster (top to bottom): lucide:layout-grid (Agents overview), lucide:plus (New bot), lucide:store, lucide:settings, then the user avatar. The active destination gets the 52px #303030 radius-12 square behind its icon; no bot is selected on these screens.

## WF2 Agents overview (dark)

Frame "WF2 Agents overview (dark)", 1440x900, fill #070707. Rail as above with layout-grid active.

Main area x 88 to 1440 (padding 24):
- Top bar (h 44) with Windows caption buttons at the far right, as in WF1.
- Header row (h 56): "Agents" (Inter 20 semibold #FCFCFC) and under/after it "6 bots, 2 live, 1 waiting for you" (Inter 13 #989898). Right side: segmented filter (h 32, #1A1A1A radius 16): "All" (selected #303030), "Live", "Waiting", "Paused"; a lucide:search 32px icon button; "New bot" white pill (h 32, lucide:plus).
- Grid: 4 columns, gap 16, card width (1352 - 48 - 48) / 4 = 314, card height 368, radius 14, fill #111111, 1px #1C1C1C stroke, padding 12. Eight cells: six bots + "New bot" card + "Team room" card.
  - Thumbnail 290x181 (16:10) radius 10:
    - Pnet apply (Live): mini Chrome window on pnet.co.za (as WF1 screen, simplified), orange cursor tag.
    - CV Writer (Working): a document page (white #F4F5F7 page with gray text lines, a heading bar) in a mini window titled "CV-Sabenza-Angular.docx".
    - Inbox Triage (Waiting for you): mini Gmail-like list (white rows) with a #459FFF 2px inner border on the thumbnail.
    - Chief of Staff (Idle), Careers24 scout (Idle): #1A1A1A with the bot avatar 56px at 35% opacity centered and "Idle" Inter 12 #6E6E6E under it.
    - Night Shift (Paused): #1A1A1A, avatar 50% opacity, lucide:pause 16px #989898 and "Paused".
  - Title row: 28px avatar, name (Inter 15 semibold #FCFCFC), state chip at right (h 22, radius 11, Inter 12): Live (#3FA37A text, 6px dot, fill #12261D), Working (#F5F5F5 on #262626 with lucide:loader 12px), Waiting for you (#459FFF on #101722), Idle (#989898 on #1A1A1A), Paused (#989898 on #1A1A1A with lucide:pause 12px).
  - Task line (Inter 13 #BDBDBD, one line, truncate):
    - Pnet apply: "Applying: Full Stack Developer at Sabenza IT"
    - CV Writer: "Tailoring CV for listing PN-48213"
    - Inbox Triage: "2 replies need your approval"
    - Chief of Staff: "Morning briefing sent at 7:00 AM"
    - Night Shift: "Paused by you yesterday"
    - Careers24 scout: "Last run found 4 roles"
  - Next routine (lucide:clock 14px #989898 + Inter 12 #989898):
    - "Next: Pnet apply 1pm, in 2h 10m"
    - "No routine"
    - "Next: Inbox sweep 12:00 PM"
    - "Next: Evening digest 5:00 PM"
    - "Paused"
    - "Next: Careers24 sweep 3:00 PM"
  - Footer: "Talking with" (Inter 12 #6E6E6E) + overlapping 18px mini avatars (Pnet apply: CV Writer + Chief of Staff; CV Writer: Pnet apply; Inbox Triage: Chief of Staff; Chief of Staff: Pnet apply + Inbox Triage; Night Shift and Careers24 scout: "No bot chats today"). Card menu lucide:more-horizontal at the title row far right only for hover state on one card (Pnet apply) to show the affordance.
  - Inbox Triage card also has a "Review replies" white pill (h 28) at the bottom right; its card stroke is 1px #1E3A5F.
  - "Team room" card: solid card (same style as bot cards) with the three mini blobs group, "Team room", "Pnet apply and CV Writer talked 4 minutes ago", "Open room" secondary pill.
  - "New bot" card: 1px dashed #303030 stroke (strokeDash if available, else solid #252525), centered lucide:plus 24px in a 48px #1A1A1A circle, "New bot" (Inter 15 semibold), "Start from a suggestion or from scratch" (Inter 12 #989898).

## WF3 New bot (dark)

Frame "WF3 New bot (dark)", 1440x900. Rail with lucide:plus active. Main area x 88 to 1440.
- Top bar with caption buttons; header chip centered like WF1 header: the preview avatar 24px + "New bot".
- Left/center column (x 88 + 120, w 640), top to bottom:
  - Avatar preview 96px (placeholder blob, blue #3B82F6 circle) centered in the column; under it a row of 10 color swatches (28px circles, palette from brief; blue selected with a 2px #F5F5F5 ring offset 3px) and a row of 8 shape/icon options (32px tiles #1A1A1A radius 10, selected tile #303030 with 1px #459FFF stroke). Label above swatches: none (visual).
  - "Name" label (Inter 13 #989898) + input (h 44, radius 10, fill #111111, 1px #2A2A2A stroke, text "Pnet apply" Inter 14 #F5F5F5).
  - "What should this bot do?" label + textarea (h 96, same style): "Apply to Full Stack and React roles on pnet.co.za in Johannesburg or remote. Skip senior roles. Ask CV Writer for a tailored CV when a listing asks for Angular or Azure."
  - Settings sections as stacked rows (each h 56, fill #111111, radius 12, px 16, gap 8; left lucide icon 18px #ACACAC, title Inter 14 #F5F5F5, value Inter 13 #989898 at right, lucide:chevron-right):
    1. lucide:sparkles "Model": "Claude Sonnet 5"
    2. lucide:globe "Chrome profile": "New isolated profile"
    3. lucide:key-round "Logins": "pnet.co.za" (+ "Choose")
    4. EXPANDED lucide:monitor "Computer access": row header plus body (fill #111111 continues, h ~96): segmented control "Browser only" (selected) | "This PC" | "Virtual machine", and helper text "Browser only: the bot works inside its own Chrome profile and cannot touch other apps." (Inter 12 #989898).
    5. lucide:message-circle "Can message": 18px avatars of CV Writer and Chief of Staff + "2 bots"
    6. lucide:clock "Routines": "Weekdays 8:48 AM, 12:47 PM, 4:45 PM"
- Right column (x 1440 - 24 - 340, w 340): "Suggestions" (Inter 13 semibold #9C9C9C) then three cards (radius 14, fill #111111, 1px #1C1C1C, p 16, h ~92) like ref 1: 40px avatar + title (Inter 15 semibold) + one-line description (Inter 13 #989898):
  - "Night Shift": "Works overnight and preps your morning digest" (amber hexagon)
  - "Inbox Triage": "Sorts your email and drafts replies in your voice" (pink cloud)
  - "Job hunter": "Applies to roles that match your CV every weekday" (orange cloud)
- Bottom action bar (h 64, full main width, top 1px #1C1C1C, fill #070707): right-aligned "Cancel" (#262626 pill) and "Create bot" (white pill, text #070707).

## WF4 Logins (dark)

Frame "WF4 Logins (dark)", 1440x900. Rail with lucide:settings active.
- Settings sub-nav column (x 88, w 220, fill #0B0B0B, right 1px #1C1C1C): "Settings" (Inter 15 semibold) then items (h 36, radius 8, Inter 14): General, Bots, Logins (selected: #1F1F1F fill, #F5F5F5 text), Chrome profiles, Models, Notifications, Privacy. Icons lucide 16px #989898 per item (settings-2, bot, key-round, globe, sparkles, bell, shield).
- Main (x 308 to 1440 - 380):
  - Top bar with caption buttons spans to the right edge of the window (right panel included).
  - Header: "Logins" (Inter 20 semibold), under it "Stored in Windows Credential Manager. Bots fill logins without seeing your passwords." (Inter 13 #989898, lucide:shield-check 14px #3FA37A before it).
  - Actions row: search input (w 280, h 36), and at right "Import from Chrome" (#262626 pill with logos:chrome 14px), "Import CSV" (#262626 pill, lucide:upload), "Add login" (white pill, lucide:plus).
  - List (rows h 60, divider 1px #1C1C1C between rows only): columns: site (28px rounded-square favicon tile with the first letter or lucide:globe on #1A1A1A, site Inter 14 #F5F5F5), username (Inter 13 #BDBDBD), password "••••••••••" (Inter 13 #989898), "Bots" (overlapping 18px mini avatars), last used (Inter 12 #989898), lucide:more-horizontal. Column header row (Inter 12 #6E6E6E): Site, Username, Password, Bots with access, Last used.
    - pnet.co.za, tyrel.jobs@gmail.com, Pnet apply, "2 min ago" (row SELECTED: fill #111111, radius 10)
    - careers24.com, tyrel.jobs@gmail.com, Careers24 scout + Pnet apply, "Yesterday"
    - linkedin.com, tyrel.jobs@gmail.com, Pnet apply + Careers24 scout, "3 days ago"
    - mail.google.com, tyrel.jobs@gmail.com, Inbox Triage + Chief of Staff, "Today"
- Detail drawer (x 1060, w 380, fill #0B0B0B, left 1px #1C1C1C): favicon tile 40px + "pnet.co.za" (Inter 17 semibold) + lucide:x close; fields (label Inter 12 #989898, value Inter 14): Username "tyrel.jobs@gmail.com" with lucide:copy; Password "••••••••••" with "Show" (Inter 13 #459FFF) and helper "Asks for Windows Hello first" (Inter 12 #6E6E6E).
  - "Bots that can use this login" (Inter 13 semibold #9C9C9C): list of all six bots, each row h 40: 24px avatar, name, toggle switch at right (on = #F5F5F5 track with #070707 knob; off = #303030 track with #989898 knob). On: Pnet apply. Off: the others.
  - "Signed in on" : "Pnet apply's Chrome profile" with logos:chrome 14px.
  - "Last used by Pnet apply, 2 minutes ago" (Inter 12 #989898).
  - Bottom: "Delete login" (Inter 13 #EA4144, lucide:trash-2) left-aligned.

## Phase 1 screens (plan 1C, 2026-09-25)

Frames on the "Wireframes" page of `design/tjommie-wireframes.fig`, exported to `docs/wireframes/`. Find frames and layers by name: node ids change every time the file is reopened.

| Frame | Export | What it shows |
|---|---|---|
| WF1 Main chat, Phase 1 (dark) | wf1-phase1-main-chat-dark.png | Phase 1 chat: header chip "Working", routine label, activity row, handoff rows, file card, approval card (Allow once, Always allow, Deny), Stop button, bot panel (summary, Browser with Show window, Routines with Add, Recent activity). Careers24 scout is Limited in the rail |
| WF2 Agents overview, Phase 1 (dark) | wf2-phase1-agents-overview-dark.png | The overview without live thumbnails: a 56 px avatar placeholder with a state line ("Working", "Waiting for you", "Idle", "Paused"); header "6 bots, 2 working, 1 waiting for you"; filter All, Working, Waiting, Paused; no store icon (ruling 1C-01) |
| WF3 New bot, Phase 1 (dark) | wf3-phase1-new-bot-dark.png | Settings rows: Model, Folder, Autonomy (Always ask, Own folder, Full auto), Browser (with an on switch), Can message, Routines |
| WF5 Team room (dark) | wf5-team-room-dark.png | Bot-to-bot posts ("to <bot>", Open exchange), @mentions in the person's bubble, the mention popover filtered to the typed "@Ca", members with states, the chain limit line |
| WF6 Exchange view (dark) | wf6-exchange-view-dark.png | An opened exchange in the right panel, with its handoff row selected in the chat. The panel composer replies in the Team room with both bots mentioned as chips |
| WF7 Routine editor (dark) | wf7-routine-editor-dark.png | A dialog over the dimmed chat: on switch, name (focused), what to do, days, times, time zone, stop after, recent runs (Done, Stopped, Failed, Missed, Skipped), Delete routine, Cancel, Save |
| WF8 Bot settings (dark) | wf8-bot-settings-dark.png | Header chip with the bot's state, the 96 px avatar preview (orange and cloud selected), the New bot rows, About this bot (created, home folder, notes.md preview), Delete bot, Pause bot, Cancel, Save changes |
| WF9 First-run setup (dark) | wf9-first-run-dark.png | Provider cards: signed in, signing in, not signed in, not installed; Create your first bot. The rail has no bots yet |
| WF10 Settings: Bots (dark) | wf10-settings-bots-dark.png | Settings nav (General, Bots) and the Chain limit stepper (default 5) |
| WF10 Settings: General (dark) | wf10-settings-general-dark.png | The Appearance row with Dark, Light and System (the WF7 "Stop after" control style) (ruling 1C-08) |
| WF11 Chat states (dark) | wf11-chat-states-dark.png | Every message, card, row, composer, rail and dialog state, plus the empty, long-text and focus states. 1440x2274 |

- Every export also has a 2x file with "@2x" before ".png" (2880x1800). WF11's is 2594x4096, because exports cap the long edge at 4096.
- For Phase 1, these frames replace WF1 dark, WF2 and WF3. WF1 light and WF4 Logins have no Phase 1 version: the light theme maps from the tokens, and logins arrive in Phase 4.

### Colors and type

The Phase 1 frames use only these tokens (from `docs/design/brief.md` and WF1 to WF4) plus the approved exceptions below, so the light theme is a mechanical swap:

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

Approved exceptions (ruling 1C-04 and the inherited-color rulings):
- `#3A3A3A`: the disabled send button; also the inherited Attach button.
- `#000000` at 60%: the routine editor scrim.
- `#1F1F1F`: the selected Settings nav item.
- Radius 8 for popover rows (model picker, mention popover). The Settings nav rows and the file tile also use 8, inherited.
- Colors inherited unchanged from WF1 to WF4:
  - `#777777`: the user avatar.
  - `#080808`: the rail's right edge.
  - `#3A2020` with `#EA4144`: the file tile and its PDF icon.
  - The Chrome logo's brand colors (`#1A73E8`, `#229342`, `#E33B2E`, `#FBC116`) in the Browser card.
  - `#8A8A8A`: WF2's "Talking with" and "No bot chats today" labels, and the Team room card's message headers.
- Bot colors appear only inside avatars and the avatar color and shape pickers.

Type: Inter only.
- Sizes 12, 13, 14, 15, 17 and 20. Size 11 only for the smallest labels: "Queued", the model picker caption, WF2's Team room card and WF11's captions.
- Weights 400, 500 and 600.
- Line heights: 14/21 for bubbles and body text, 12/18 for helper text.

Radius: bubbles 18; cards 12; large cards 14; dialogs and popovers 16; inputs and rows 10; popover rows 8; pills and chips fully round.

### Avatars

Every avatar is an instance of a component drawn at its own size, never a scaled instance: a save and reopen resets a scaled instance's children.
- 40 px masters: "Bot avatar / <bot>" for all six bots.
- Size components, "Bot avatar / <bot> / <size>", in the component column left of the frames:

  | Bot | Sizes (px) |
  |---|---|
  | Chief of Staff | 9.8, 14, 18, 24, 28, 56 |
  | Pnet apply | 9.8, 14, 18, 24, 28, 56, 96 |
  | CV Writer | 9.8, 14, 18, 24, 28, 56 |
  | Inbox Triage | 18, 24, 28, 56 |
  | Night Shift | 24, 28, 56 |
  | Careers24 scout | 18, 24, 28, 56 |

1D renders each bot from one SVG, at any size. The sizes in use:

| Size | Where |
|---|---|
| 9.8 | Only inside the WF2 Team room card icon |
| 14 | The Team room cluster in the rail |
| 18 | Handoff rows, mention rows, exchange author rows and composer chips, "Can message", "Talking with" |
| 24 | Header chips, member rows, the exchange header |
| 28 | Team room posts, WF2 card titles |
| 40 | Rail, bot summary, Suggestions, empty chat |
| 56 | WF2 overview placeholders |
| 96 | The New bot and Bot settings preview, in its 156 px tile. WF3 Phase 1 draws it as loose vectors; WF8 uses "Bot avatar / Pnet apply / 96" |

Dimmed avatars: 35% for Idle placeholders and the empty chat, 50% for Paused and Limited, 40% for the inactive Team room tile in WF9.

### Rules these frames settle

**Rail and header**
- Rail states:
  - Working: green dot, bottom right.
  - Waiting for you: blue dot, top right.
  - Paused: 50% opacity.
  - Limited: 50% opacity plus an hourglass badge.
  - Needs setup: a plug badge, at full opacity.
  - Idle: no mark.
- Dots are 10 px with a 2 px ring in the rail color (`#111111`). On the selected bot the ring is `#303030`, the selected square's color. Badges are 16 px `#262626` circles with a 2 px `#111111` ring and a 10 px `#ACACAC` icon, centered 3 px in from the avatar's bottom-right corner.
- 50% opacity in the rail means Limited (or Night Shift's Paused), never "not selected". The roster's Careers24 scout is Idle. Only frames whose story makes it Limited dim it: WF1 Phase 1, its clones WF6 and WF7, WF5 and WF11.
- The selected item (a bot, the Team room, or a bottom-cluster destination) sits on a 52 px `#303030` square, radius 12.
- The Phase 1 bottom cluster is Agents overview, New bot and Settings, 44 px apart, then the user avatar 50 px below Settings, anchored to the bottom.
- With no bots yet (WF9), the rail shows only the Team room at 40%, the divider and the bottom cluster.
- The header chip shows the state in words ("Working", "Limited until 3:45 PM"):
  - Working: a green dot and green text.
  - Limited: an hourglass and `#989898` text.
  - The Team room's chip: a users icon, "Team room" and "6 bots".
- The header's right icons are panel-right (shows or hides the right panel) and ellipsis (More).
- The frames show "Working" in four styles, by place:
  - Header chip: green dot and green text.
  - Bot summary and WF2 card titles: a `#262626` chip with a loader icon and `#F5F5F5` text.
  - Member list and mention popover: green text.
  - WF2 placeholders: a loader and "Working" in `#BDBDBD`.

**Chat**
- The composer's round button:
  - Disabled send (`#3A3A3A`) when the field is empty and the bot is idle.
  - White send when there's text. Sending while the bot works steers it.
  - White Stop when the field is empty and the bot works.
- Attachments sit above the composer as chips: `#262626`, fully round, height 28, with a paperclip, the file name and an x. Files go with a message: send stays disabled until there's text, even with files attached (the daemon's `chat.send` requires non-empty text).
- Approval cards offer Allow once, Always allow and Deny. Under the buttons, a note says what Always allow saves ("Always allow saves: Write, for Pnet apply only"). WF11 shows it; WF1 Phase 1 and its clones leave it out, and 1D shows it everywhere. Routine proposals offer Add routine and Dismiss.
- Answered approvals collapse to one line: a check or an x, then "Allowed once: ..." or "Denied: ...".
- Handoff rows: pending (spinner, "waiting for a reply"), replied (message count), failed ("no reply"), incoming ("<bot> asked: …", one avatar, no arrow), room answer ("Answered in the Team room", a users icon, no count). In long rows the text truncates; the count and the chevron stay visible.
- The handoff row whose exchange is open (WF6) is selected: fill `#111111`, 1 px `#303030` stroke.
- Activity rows:
  - Collapsed: list-checks, "N steps: <latest step>", chevron-down.
  - Expanded: a card with one row per step (check for done, loader for running, x for failed).
  - They sit with the bot that did the work, in time order.
  - Padding 0 12, and the summary truncates.
- A run starts with a routine label: a green clock and "Routine: <name>". A missed run has a gray clock and "Missed, the PC was off".
- Bubbles wrap long text. Main-chat bot bubbles go up to 600 px (WF1 Phase 1 draws its two-line bubbles at the full 600). Team room bot bubbles hug their text up to 560 px, and the person's Team room post is also capped at 560. Exchange bubbles go up to 400 px.
- WF11 also settles: streaming (a 2x16 caret after the text), queued ("Queued" under the bubble), the notices, errors with Retry, the Needs setup banner (waiting-card colors, a plug icon, a white "Sign in" pill) and the empty chat (40 px avatar at 35%, a centered hint).
- Bot panel order: summary card, Browser card with Show window, Routines with Add, Recent activity. There are 16 px between cards and 24 px above a section label. Empty states: "No routines yet." and "No recent activity."
- Dialogs: 320 wide, padding 20, radius 16, `#111111` with a `#252525` stroke, and the buttons right-aligned on one line. The Full auto warning confirms Full auto. The Delete bot dialog confirms a delete, and its button is a `#262626` pill with red text and icon.
- The model picker and the mention popover: `#1A1A1A`, radius 16, `#252525` stroke, 36 px rows (radius 8, gap 4), highlighted row `#262626`.
- Focus: a 2 px `#459FFF` ring on inputs and buttons. On an input it replaces the border, inside the box. On a button it sits 2 px outside the pill.

**Team room and exchanges**
- A post: the 28 px avatar, then a header line with the name (13/600), an optional "to <bot>" (13, `#989898`) and the time (12, `#909090`), then the bubble. Bot-to-bot posts end with "Open exchange" and a chevron (12, `#459FFF`).
- The person's post is a right-aligned user bubble with no header. Its @mentions are `#1D6FE0` at weight 600.
- Members: a 24 px avatar, the name, and the state in words at the right. Working is green, Waiting for you is blue, and Idle, Paused and "Limited until 3:45 PM" are `#989898`. Under the list, the chain limit line has a "Change" link (the setting lives in Settings: Bots).
- The mention popover:
  - It opens above the composer: left edge at 16 px, bottom 8 px above the composer.
  - It filters by the typed prefix. A bare "@" lists every bot plus "@everyone" ("All 6 bots"). WF11 shows a short sample: CV Writer, Chief of Staff, @everyone.
  - Its rows use the short state ("Limited"); the member list uses the full one ("Limited until 3:45 PM").
  - While it's open, the room list gets bottom padding equal to the popover's height, so the newest post stays visible.
- The exchange view:
  - It opens in the right panel, in place of the bot panel, and the chat stays.
  - Header: both 24 px avatars with an arrow, "<bot> asked <bot>", a meta line ("Today 9:31 AM · Replied after 2 minutes") and a close x.
  - Order: Author / Pnet apply, request bubble, Author / CV Writer, activity row, reply bubble, attachment card, caption. Author rows are an 18 px avatar and the name (12/600, `#989898`).
  - The composer has no Attach button. Both bots are pre-mentioned as chips, and the placeholder ("Reply in Team room") fills the remaining width and truncates. Send is disabled while the field is empty.

**Bot screens**
- New bot and Bot settings share the rows Model, Folder, Autonomy, Browser, Can message and Routines.
  - Autonomy expands in place: Always ask, Own folder, Full auto, and a helper line.
  - Browser shows "Its own Chrome profile" and an on switch.
- The avatar preview is 96 px in the 156 px tile on both screens. The selected color swatch has a white ring. The selected shape tile is `#303030` with a 1 px `#459FFF` stroke. The shape tiles preview in the bot's color.
- Bot settings:
  - The header chip shows the bot and its state.
  - About this bot: Created (sample "Monday 21 September"), Home folder with "Open folder", Notes with a notes.md preview (12/18 `#BDBDBD` on `#070707`, radius 10, padding 12), then "Open notes.md".
  - The action bar has Delete bot (red icon and text, no pill) and Pause bot (a `#262626` pill) at the left, and Cancel and Save changes at the right.
- Switches: 36x20. On is a `#F5F5F5` track with a 16 px `#070707` knob at the right, 2 px inset. Off (WF4) is a `#303030` track with a `#989898` knob.
- Segmented controls (Autonomy, Stop after, Appearance, the model picker's provider switch): a `#1A1A1A` track. The selected option is a `#303030` pill with a `#F5F5F5` label at weight 600 (as in WF7 and WF10); the others are `#989898`. The Autonomy row in WF3 Phase 1 and WF8 draws its selected label at 500: that's a drawing inconsistency, so use 600 there too.
- The routine editor:
  - A 560 px dialog over a 60% black scrim, with the title, the On switch and a close x in its header.
  - Days are seven round chips; selected ones are white with a dark bold letter. Times are chips with an x, then an outlined "Add time" chip. The time zone sits on its own line.
  - Stop after: No limit, 1 hour, 2 hours, 3 hours.
  - Recent runs: icon, date, status, duration, and "Last 20 runs are kept". Statuses: Done, Stopped at the time limit, Failed: <reason>, Missed: the PC was off, Skipped: <bot> was paused. Missed and Skipped have no duration.
  - The footer has Delete routine (red) at the left, and Cancel and Save at the right.
- The agents overview:
  - Working and waiting bots show their avatar at full opacity with a state line. Idle is at 35%. Paused is at 50%, with a pause icon.
  - The waiting card has a 2 px `#459FFF` inner border on its placeholder, a `#1E3A5F` card stroke and a "Review replies" pill.
  - The Pnet apply card is drawn hovered: it shows the "..." menu and a `#2A2A2A` stroke.

**Settings and first run**
- The Settings nav has General and Bots only. The selected item is `#1F1F1F` with a `#F5F5F5` label (500) and a white icon. The others have a `#BDBDBD` label and a `#989898` icon.
- Setting cards are `#111111`, radius 12, with 16 px padding, and they hug their content.
- Chain limit: a 120x32 stepper with minus and plus, default 5.
- First run: one card per provider (Claude Code, Codex) side by side. WF9 adds an "Other states" row below, showing the not-signed-in and not-installed cards as specimens. Each card has a 36 px icon tile, the name, and the version (or "Not installed"). Then "Create your first bot", with "You can sign in to the other one later in Settings." under it.

**Copy**
- Routine names are the person's own free text, so a name like "Pnet apply 9am" is fine as typed.
- Times the app formats itself (message times, the next run, "Limited until") use the "9:00 AM" style.

**Phase 1 hides**
- The store icon, Share, mic and voice, the live screen and computer controls (Take over, Pause, Stop, the This PC / Virtual machine switch), the Chrome profile card's controls, logins and usage numbers.
- The "Live" state: Working replaces it.

### Notes for 1D (OpenPencil workarounds, not design intent)

- **Truncation:** long rows use a typed "..." because OpenPencil's truncate mode is broken. Build them with CSS `text-overflow: ellipsis`, and keep the trailing element (time, chevron, count) fixed.
- **Line height:** OpenPencil can't set line height, so "14/21" and "12/18" texts render at default leading. Use the stated line heights, and let card heights follow from the content. For example, the Chain limit card hugs its content with 16 px padding: about 94 px with 12/18 leading, not the drawn 88 or the brief's 108.
- **Fixed sizes:** these containers are FIXED in the file only because auto-layout wouldn't re-flow in this OpenPencil session. Build them as hugging, content-sized containers:
  - The 14 containers fixed during the avatar repair in WF1 dark, WF1 light, WF1 Phase 1 and WF3 (handoff rows, header chips, Messages lists, the "Can message" avatars).
  - The WF2 Phase 1 header filter.
  - The WF10 header, nav list, segmented control and selected nav labels.
  - WF5's header chip, Messages list and "Draft" (the draft fills the composer).
  - WF6's author names and chip labels.
  - WF8's header chip, buttons, Save changes, About this bot, Delete bot and Pause bot.
- **Drawn positions, not rules:**
  - WF3 Phase 1's Autonomy row places its switch and helper at 48 and 88 in a fixed 128 px row. Build it as padding plus content.
  - WF1 Phase 1's bot summary uses absolute positions and a 336 px Access line. Build it as a column of icon and text rows.
  - "Show window" is part of the Browser card's title row.
  - Recent activity rows are icon, then text (fills and truncates), then time (hugs).
  - WF6's activity row has 0 10 padding only to fit the drawing; use 0 12.
- **Fills:** the layout-only containers filled `#111111` in WF7 (Day chips, Time chips, Time zone, run rows, Delete routine) are transparent in the UI. The fill only hides OpenPencil's white default. WF2 Phase 1's Inbox Triage thumbnail also carries an inherited 0.1 px `#459FFF` stroke; build only the 2 px inner border.
- **Focus ring:** build it in CSS, not as a wrapper element (WF11 draws the button ring as an emptied icon frame). Buttons: `outline: 2px solid #459FFF; outline-offset: 2px`. Inputs: the ring replaces the border, for example `border-color: #459FFF` at 2 px, or `outline-offset: -2px`, so the box size doesn't change.
- **Card action rows** (WF9's pills) are bottom-anchored with 16 px bottom padding, not a fixed gap under the text.
- **Layer names:**
  - The rail's green "Live dot" in every Phase 1 frame is the Working dot.
  - The typed draft in "Composer / Typing" is a draft, not placeholder text.
  - WF9's bottom cards list Actions before Detail in the layer tree, but read top to bottom as header, detail, actions.
- **Icons:** after a file reopen, OpenPencil resets Lucide stroke caps, so the .fig can show `circle-alert` without its "!" and with thinner strokes. The PNGs are the reference; 1D uses the Lucide icons by name.
