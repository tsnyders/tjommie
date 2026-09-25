# Grokbot design brief (wireframe round 1)

Three distinct design directions for a desktop app where AI "bots" (agents) run tasks, talk to each other, and control a computer. Each direction is 2 frames, 1440x900, dark theme (Windows 11). The user picks one.

## 1. References (look at these first, with the Read tool)

- `docs/reference/ref-3-pnet-apply-dark.png` (1600x848): the user's own app on Windows, dark. Icon rail of bot avatars (left), chat with one bot (center), right panel with the bot's screen preview and its Routines. THIS IS THE PRIMARY REFERENCE.
- `docs/reference/ref-2-chief-of-staff-light.png` (1346x1360): macOS light marketing shot. Chat list sidebar, "Chief of Staff" chat (user bubbles black, bot bubbles light gray, PDF attachment card), right panel "Chief of Staff's screen" + Routines. Phone shows the list of bots.
- `docs/reference/ref-1-new-bot-light.png` (1600x1234): macOS light "New Bot" creator. Blob avatar picker (10 colors x 8 shapes, each blob has two small white eye strokes), name field, Suggestions cards.

The visual language to keep: calm, monochrome chrome, rounded bubbles, friendly blob avatars as the only color, generous whitespace, small muted labels. It must feel like the same product, extended.

## 2. What the app must show (the new features)

1. **Agents listed.** All bots visible with avatar, name, and state (running a routine, idle, waiting for you, paused).
2. **Agents talk to each other.** Bots delegate and report to each other (e.g. Chief of Staff asks Pnet apply for status; Pnet apply asks CV Writer for a tailored CV). The user can see those exchanges and join in.
3. **Computer control panel (like Codex / ChatGPT agent).** A side panel shows the bot's live screen: it controls this PC (or a virtual machine) through a dedicated Chrome profile made just for the bot. Needed controls and info:
   - live screen with the bot's cursor, status ("Live", "Paused", "Waiting for you")
   - Take over (user drives), Pause, Stop (destructive, red only here)
   - target: "This PC" or "Virtual machine"
   - Chrome profile card: profile name (e.g. "Pnet apply"), isolated from the user's personal Chrome, sites signed in
   - **Logins (credential vault):** saved emails/usernames + masked passwords per site, "Import from Chrome", "Import CSV", "Add login". Copy promise: "Stored in Windows Credential Manager. Bots fill logins without seeing your passwords."
   - human-in-the-loop moment: "Waiting for you: enter the OTP sent to your phone" style card with a Take over action
4. **Routines** (from the reference): scheduled runs with status icons.

## 3. Tokens (measured from the references)

Dark (primary, from ref 3):
| Role | Hex |
|---|---|
| App rail / sidebar bg | `#111111` |
| Main canvas + right panel bg | `#070707` |
| Dividers | `#252525` |
| Bot bubble | `#262626` |
| User bubble (dark theme) | `#F5F5F5` bg, `#070707` text (mirror of light theme's black bubble) |
| Header chip (bot name) | `#181818` |
| Composer | `#2F2F2F` |
| Screen placeholder / raised surface | `#282828` |
| Selected rail item bg | `#303030` |
| Text primary | `#F5F5F5` (titles `#FCFCFC`) |
| Text secondary | `#989898` |
| Timestamps / captions | `#909090` |
| Placeholder | `#6E6E6E` |
| Icon default | `#ACACAC` |
| Accent (links, "new messages" pill, focus, unread) | `#459FFF` |
| Live / active routine | `#3FA37A` |
| Destructive (Stop only) | `#EA4144` |

Light (for reference; not needed this round): sidebar `#F7F7F7`, canvas `#FCFCFC`, selected `#E1E1E1`, search `#EBEBEB`, divider `#E4E4E4`, user bubble `#070707`/white text, bot bubble `#EFEFEF`, input border `#D6D6D6`, accent `#3B82F6`.

Bot colors (the ONLY decorative color in the UI): brown `#8D6541`, red `#EA4144`, orange `#EC712E`, amber `#F19C3F`, green `#5BC57A`, teal `#57B8A7`, blue `#3B82F6`, purple `#8C5BF3`, pink `#EC4698`, gray `#777777`.

## 4. Measurements (from ref 3 at 1600px, scale to 1440)

- Icon rail 88-92px wide, avatars 36-40px, selected item = 52px rounded square (radius 12) `#303030`.
- Chat header: centered chip with 24px avatar + bot name (15px semibold), chip bg `#181818`, full radius, 40px tall. Share icon top right of chat column.
- Bot bubbles: left aligned, max width ~640px, radius 16-18, padding 14px 16px, text 14px / 21px, paragraph gap 10px.
- Timestamps: centered, 12px, e.g. "Today 9:14 AM".
- Composer: full width of chat column minus 16px sides, 44px tall, radius 22, bg `#2F2F2F`, "+" icon left, mic icon + white 32px circle voice button right, placeholder "Message Pnet apply".
- Right panel ~480px wide: top bar icons (settings, collapse ">>", then Windows caption buttons minimize/maximize/close at the far right of the window), screen preview 16:10 radius 10, caption "Pnet apply's screen" centered 12px, "Routines" label 13px semibold muted, rows: 16px green clock icon + title 13px + schedule 12px muted.
- Windows 11 caption buttons at top right of the window: `lucide:minus`, `lucide:square`, `lucide:x`, 16px, `#ACACAC`.

Type: **Inter only** (the headless renderer can only draw Inter; other fonts render as nothing). Sizes: 12 (meta), 13 (labels, list preview), 14 (body), 15 (titles), 20 (panel headings, rare). Weights: regular, medium, semibold. The real app will use the OS system font.

Radius system (lock it): bubbles 18, cards/panels 12, screen 10, pills/buttons full, inputs 10.

Icons: Iconify `lucide:*` names via `<Icon name="lucide:settings" size={16} color="#ACACAC" />`. Useful: plus, search, settings, chevrons-right, share, monitor, mic, audio-lines, clock, pause, play, square (stop), mouse-pointer-2, hand, key-round, lock, shield-check, globe, upload, download, file-text, arrow-up, x, minus, maximize-2, users, at-sign, check, circle-alert, refresh-cw, chrome (may not exist: use `logos:chrome` or `simple-icons:googlechrome`).

## 5. Blob avatars (use these snippets, keep them consistent)

All shapes use `viewBox="0 0 40 40"`; set `size` to the pixel size you want (20, 24, 32, 40, 56). Replace FILL with a bot color. Eyes are two short white slanted strokes.

```jsx
// circle
<svg viewBox="0 0 40 40" size={32}><path d="M20 2 C29.94 2 38 10.06 38 20 C38 29.94 29.94 38 20 38 C10.06 38 2 29.94 2 20 C2 10.06 10.06 2 20 2 Z" fill="FILL" /><path d="M17 14 L16 20" stroke="#FFFFFF" strokeWidth={3} fill="none" /><path d="M24.5 14 L23.5 20" stroke="#FFFFFF" strokeWidth={3} fill="none" /></svg>
// blob (wide)
<svg viewBox="0 0 40 40" size={32}><path d="M4 22 C4 12 11.5 6 21 6 C30.5 6 37 12.5 37 21 C37 29.5 30.5 35 21 35 C11 35 4 30.5 4 22 Z" fill="FILL" /><path d="M18 15 L17 21" stroke="#FFFFFF" strokeWidth={3} fill="none" /><path d="M25.5 15 L24.5 21" stroke="#FFFFFF" strokeWidth={3} fill="none" /></svg>
// rounded square
<svg viewBox="0 0 40 40" size={32}><path d="M11 4 H29 C32.9 4 36 7.1 36 11 V29 C36 32.9 32.9 36 29 36 H11 C7.1 36 4 32.9 4 29 V11 C4 7.1 7.1 4 11 4 Z" fill="FILL" /><path d="M17 14 L16 20" stroke="#FFFFFF" strokeWidth={3} fill="none" /><path d="M24.5 14 L23.5 20" stroke="#FFFFFF" strokeWidth={3} fill="none" /></svg>
// pill
<svg viewBox="0 0 40 40" size={32}><path d="M12 10 H28 C33.5 10 38 14.5 38 20 C38 25.5 33.5 30 28 30 H12 C6.5 30 2 25.5 2 20 C2 14.5 6.5 10 12 10 Z" fill="FILL" /><path d="M18 15.5 L17.3 21.5" stroke="#FFFFFF" strokeWidth={3} fill="none" /><path d="M24.5 15.5 L23.8 21.5" stroke="#FFFFFF" strokeWidth={3} fill="none" /></svg>
// triangle
<svg viewBox="0 0 40 40" size={32}><path d="M20 4 C21.6 4 22.8 4.8 23.7 6.3 L37 29.8 C38.6 32.6 36.6 35 33.5 35 H6.5 C3.4 35 1.4 32.6 3 29.8 L16.3 6.3 C17.2 4.8 18.4 4 20 4 Z" fill="FILL" /><path d="M17.8 20 L17.1 26" stroke="#FFFFFF" strokeWidth={3} fill="none" /><path d="M23.8 20 L23.1 26" stroke="#FFFFFF" strokeWidth={3} fill="none" /></svg>
// hexagon
<svg viewBox="0 0 40 40" size={32}><path d="M17 3.2 C18.9 2.1 21.1 2.1 23 3.2 L33.5 9.3 C35.4 10.4 36.5 12.4 36.5 14.6 V25.4 C36.5 27.6 35.4 29.6 33.5 30.7 L23 36.8 C21.1 37.9 18.9 37.9 17 36.8 L6.5 30.7 C4.6 29.6 3.5 27.6 3.5 25.4 V14.6 C3.5 12.4 4.6 10.4 6.5 9.3 Z" fill="FILL" /><path d="M17 15 L16 21" stroke="#FFFFFF" strokeWidth={3} fill="none" /><path d="M24.5 15 L23.5 21" stroke="#FFFFFF" strokeWidth={3} fill="none" /></svg>
// cloud
<svg viewBox="0 0 40 40" size={32}><path d="M11 34 C5 34 2 29.5 3.5 25 C2 19 7.5 15 12.5 16.5 C14 10 21 7 26 11 C32 9.5 37 14 35.5 20 C39 23.5 37 32 30.5 34 Z" fill="FILL" /><path d="M17.5 19 L16.7 25" stroke="#FFFFFF" strokeWidth={3} fill="none" /><path d="M24.5 19 L23.7 25" stroke="#FFFFFF" strokeWidth={3} fill="none" /></svg>
// droplet
<svg viewBox="0 0 40 40" size={32}><path d="M20 3 C20 3 33 16 33 25 C33 32.2 27.2 37 20 37 C12.8 37 7 32.2 7 25 C7 16 20 3 20 3 Z" fill="FILL" /><path d="M17.5 21 L16.8 27" stroke="#FFFFFF" strokeWidth={3} fill="none" /><path d="M24 21 L23.3 27" stroke="#FFFFFF" strokeWidth={3} fill="none" /></svg>
```

## 6. Roster and scenario (use the same data in all three directions)

User: Tyrel (initials avatar "T", gray). South African job hunt plus admin.

| Bot | Shape + color | Job | State in the frames |
|---|---|---|---|
| Chief of Staff | circle, purple | Coordinates the other bots, morning briefing | Idle |
| Pnet apply | cloud, orange | Applies to jobs on pnet.co.za in its own Chrome profile | Live, controlling Chrome |
| CV Writer | rounded square, green | Tailors CVs and cover letters per listing | Working (asked by Pnet apply) |
| Inbox Triage | cloud, pink | Sorts Gmail, drafts replies | Waiting for you (needs a reply approved) |
| Night Shift | hexagon, amber | Overnight research, preps morning digest | Paused |
| Careers24 scout | droplet, teal | Finds roles on careers24.com | Idle |

Conversation beats to show (pick what fits your layout; keep copy natural, no em-dashes):
- Today 9:14 AM. Pnet apply: "Morning window started. 6 new listings match Full Stack or React in Johannesburg and remote."
- Pnet apply to CV Writer: "Sabenza IT wants Angular and Azure up front. Can you tailor the CV for listing #PN-48213?"
- CV Writer to Pnet apply: "Done. CV-Sabenza-Angular.pdf, 2 pages. Moved Azure DevOps to the top."
- Pnet apply: "Applied to Full Stack Developer at Sabenza IT (Pretoria). Pnet showed the confirmation page."
- Chief of Staff to Pnet apply: "Add this to the 5pm digest and skip senior roles."
- Waiting for you (Pnet apply): "Pnet sent a one-time PIN to your phone. Take over to enter it, or reply with the code."
- Attachments: "CV-Sabenza-Angular.pdf, 2 pages, 184 KB".

Remote screen content: a Chrome window on `pnet.co.za/jobs/full-stack-developer-sabenza` with the Apply form (you may draw a simplified browser: tab strip, address bar with a lock icon, page header, 2-3 form fields, blue Apply button) and the bot's cursor with a small name tag. Profile chip in Chrome: "Pnet apply".

Logins (vault) rows:
| Site | Login | Last used |
|---|---|---|
| pnet.co.za | tyrel.jobs@gmail.com | 2 min ago |
| careers24.com | tyrel.jobs@gmail.com | Yesterday |
| linkedin.com | tyrel.jobs@gmail.com | 3 days ago |
| mail.google.com | tyrel.jobs@gmail.com | Today |
Passwords always masked as 10 dots.

Routines (Pnet apply): "Pnet apply 9am (3h)" Weekdays at 8:48 AM; "Pnet apply 1pm (3h)" Weekdays at 12:47 PM; "Pnet apply 5pm (3h)" Weekdays at 4:45 PM (paused one uses a pause icon).

## 7. The three directions

Each direction = 2 frames, both 1440x900, dark, with Windows caption buttons top right. Name frames exactly as given.

### A. "Classic" (closest to the reference)
- Frame `A1 Chat + computer`: layout of ref 3: 88px icon rail (bots as blobs, selected bot highlighted, "+" and a store icon near the bottom, user avatar at the very bottom) | chat with Pnet apply | right panel (~480px): live screen preview at the top (status "Live" + Take over / Pause / Stop row under it), Chrome profile line, Routines, Logins summary ("4 saved logins" + Manage).
- Agent-to-agent talk appears INLINE in the chat as compact handoff rows between bubbles: two mini avatars + "Pnet apply asked CV Writer" + one-line summary, expandable. Keep it quiet and secondary to the bubbles.
- Frame `A2 Computer expanded`: the right panel widened into a large computer view (screen ~920px wide) with the control bar (Take over, Pause, Stop, target switch This PC / Virtual machine), and a Logins drawer listing the vault rows + Import buttons. Chat column compressed to the left.

### B. "Team room"
- Frame `B1 Team thread`: left sidebar is a chat LIST like ref 2 (search + "+", rows with avatar, name, time, last message preview, status: "Live", "Waiting for you" with accent, "Paused"), a pinned "Team" room at the top of the list. Center: the Team room thread where several bots post (each bot bubble carries its avatar + name + "to Pnet apply" style addressee) and the user can @mention bots; composer placeholder "Message the team, @ to mention a bot". Right panel with a segmented control `Screen | Routines | Logins`, Screen selected: live screen, status, controls.
- Frame `B2 Logins`: same shell with the right panel on `Logins`: Chrome profile card (profile name, "Isolated from your personal Chrome", target This PC), vault list (rows above), Import from Chrome / Import CSV / Add login, security note. Center shows a 1:1 chat with Inbox Triage in the "Waiting for you" state so the list shows varied states.

### C. "Computer-first"
- Frame `C1 Live computer`: Codex-style. Narrow 72px icon rail | chat column ~420px | large computer pane taking the rest: browser-like toolbar above the screen (profile chip "Pnet apply", address bar, Live status, Take over button, Pause, Stop), big screen, and under it an Activity timeline that interleaves bot actions ("Opened listing", "Filled email from Logins", "Clicked Apply") with agent-to-agent messages (avatar to avatar).
- Frame `C2 Agents`: an Agents overview board: every bot as a card with its mini live screen thumbnail (or a placeholder when idle), state, current task, next routine, and "Talking with" avatars. A clear "New bot" card. This is the "lists the agents" view. Include the "Waiting for you" card treatment for Inbox Triage.

## 8. Rules (from the taste and Emil skills, apply them)

- Chrome stays monochrome; bot blobs are the only decorative color. Semantic colors only for state: accent blue, live green, destructive red (Stop only).
- One radius system (section 4). One icon family (lucide). Inter only.
- No em-dashes or en-dashes anywhere in visible text. No emoji. No "Section 01" style labels, no decorative dots (a status dot is fine only when it shows real state).
- Realistic copy from section 6; nothing like "Acme", "John Doe", "Lorem".
- Contrast: body text at least 4.5:1 on its background; muted text no darker than `#8A8A8A` on `#070707`.
- Buttons: labels on one line; primary = white pill with near-black text; secondary = `#262626` pill; destructive = red text or red outline pill.
- Every interactive control looks interactive; icon-only buttons are 32px hit areas.
- Motion is not visible in a static frame, but add a short `motion` note in your summary per direction (what animates, how long, which easing), following Emil: nothing on keyboard actions, UI transitions under 300ms, ease-out `cubic-bezier(0.23, 1, 0.32, 1)`, press scale 0.97, live-status pulse only while live, reduced-motion fallback.

## 9. How to author and verify

- JSX dialect reference: `docs/design/openpencil-jsx-reference.md`. Key props: `Frame` with `flex="row"|"col"`, `w`/`h` (px, "hug", "fill"), `gap`, `p`/`px`/`py`/`pt`/`pr`/`pb`/`pl`, `bg`, `stroke`, `strokeWidth`, `rounded`, `items`, `justify`, `grow`, `overflow="hidden"`, `position="absolute"` with `x`/`y` for overlays; `Text` with `size`, `weight`, `color`, `lineHeight`, `w="fill"` for wrapping; `Icon` with `name`, `size`, `color`; inline `<svg viewBox size>` with `<path d fill stroke strokeWidth>`.
- Always set `font="Inter"` or omit `font`. Never use other fonts.
- Render + rasterize (headless, no app needed):
  `node design/tools/render.mjs design/out/<id>/<id>.fig "<Direction name>=design/jsx/<id>-1.jsx,design/jsx/<id>-2.jsx"`
  PNGs land in `design/out/<id>/png/`. Look at every PNG with the Read tool and fix what you see (clipped or overflowing text, misalignment, wrong spacing, unreadable contrast) before finishing.
- One JSX file = one root `Frame` 1440x900 with `overflow="hidden"`.
