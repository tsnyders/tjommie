# Grokbot wireframes: spec

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
