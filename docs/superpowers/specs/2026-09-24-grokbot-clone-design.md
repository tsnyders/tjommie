# Grokbot Clone: Plan (revision 2)

> **Partly superseded (2026-09-25).** The app is now called Tjommie. Its architecture, AI providers (Claude Code and Codex CLIs, not AI SDK / AI Gateway), phases and open questions are in `2026-09-25-tjommie-design.md`. Sections 2 (reference tokens), 3 (the three designs) and 6 (motion) still apply.

Draft, 2026-09-24. Revision 2 replaces the first draft, which assumed xAI's Grok web UI. Your screenshots show a different product: a bot app with blob avatars, per-bot chats, a right panel with each bot's screen, and Routines. This plan follows those screenshots and adds the remote desktop / Chrome control feature.

Waiting on: your pick of design A, B or C (wireframes in `design/`), then approval of this plan.

## 1. What we're building

**What you asked for**
- A clone of the Grokbot desktop app as shown in your three screenshots (`docs/reference/ref-*.png`), built with Next.js, shadcn/ui and Tailwind.
- Bots (agents) that talk to each other, and a visible list of bots.
- A computer panel like Codex's: a side panel showing the bot's live screen while it controls this PC through a Chrome profile made just for the bot, with saved and imported logins (emails, passwords).
- Three designs in OpenPencil to choose from. Taste and Emil skills for design; GSAP for motion; Nano Banana for bot profile images.

**Assumptions (correct anything wrong)**
1. Windows first (your machine, and screenshot 3 is the Windows build). Dark theme first; light theme is a token swap (light tokens are already measured).
2. Single user, local-first: data stays on this PC.
3. v1 computer control = a dedicated Chrome profile per bot, driven by Playwright, streamed into the panel. Full-desktop control and a virtual machine target come later (the UI already has the "This PC / Virtual machine" switch).

**Done means**
- Side by side with screenshot 3 at the same size, layout, spacing, type and color match.
- A bot can message another bot and get a reply; you can watch and join.
- The computer panel shows the bot's Chrome live, with Take over, Pause and Stop working.
- Logins import from a Chrome CSV export, are stored encrypted by Windows, and get filled into pages without the password ever reaching the model.

## 2. Reference tokens (measured from your screenshots)

Dark: rail `#111111`, canvas and right panel `#070707`, dividers `#252525`, bot bubble `#262626`, header chip `#181818`, composer `#2F2F2F`, raised surface `#282828`, selected rail item `#303030`, text `#F5F5F5`, secondary `#989898`, timestamps `#909090`, placeholder `#6E6E6E`, icons `#ACACAC`, accent `#459FFF`, live `#3FA37A`.

Light: sidebar `#F7F7F7`, canvas `#FCFCFC`, selected `#E1E1E1`, search `#EBEBEB`, divider `#E4E4E4`, user bubble `#070707` with white text, bot bubble `#EFEFEF`, input border `#D6D6D6`, accent `#3B82F6`.

Bot palette: brown `#8D6541`, red `#EA4144`, orange `#EC712E`, amber `#F19C3F`, green `#5BC57A`, teal `#57B8A7`, blue `#3B82F6`, purple `#8C5BF3`, pink `#EC4698`, gray `#777777`. Eight blob shapes (circle, blob, rounded square, pill, triangle, hexagon, cloud, droplet), each with two white eye strokes.

Type: the app uses the OS font (Segoe UI on Windows, SF Pro on macOS), so the build uses the `system-ui` stack. Wireframes use Inter because the headless renderer only ships Inter.

Full measurements: `docs/design/brief.md`.

## 3. Three designs

Rendered in `design/grokbot-designs.fig` (open it in OpenPencil) with PNGs in `design/out/*/png/`. All three keep the reference look. They differ in where bots live and how the computer panel works.

- **A. Classic.** Screenshot 3's layout: icon rail, chat, right panel with the live screen, controls, Chrome profile, Routines and a Logins summary. Bot-to-bot talk shows as quiet handoff rows inside the chat. The panel can expand into a large computer view with a Logins drawer.
- **B. Team room.** Chat list sidebar (like screenshot 2) with a pinned Team room where several bots post and you @mention them. Right panel has `Screen | Routines | Logins` tabs.
- **C. Computer-first.** Codex style: narrow rail, compact chat, a large live computer pane with a browser toolbar and an activity timeline mixing bot actions with bot-to-bot messages. Plus an Agents board: every bot as a card with a mini live screen, state and next routine.

## 4. Architecture

```
Electron shell (Windows window, tray, auto-start)
 ├─ Renderer: Next.js App Router UI (shadcn/ui + Tailwind v4 + GSAP)
 └─ Main process (Node 24)
     ├─ Next.js standalone server (route handlers: chat streaming, bot APIs)
     ├─ Bot runtime: AI SDK 7 agents, tools, bot-to-bot message bus
     ├─ Computer service: Playwright + one persistent Chrome profile per bot
     │    screen stream via CDP screencast -> panel; input forwarding for Take over
     ├─ Vault: logins encrypted with Electron safeStorage (Windows DPAPI)
     ├─ Scheduler: routines (cron expressions), survives restarts
     └─ Store: SQLite (bots, chats, messages, routines, vault metadata)
```

Why Electron and not a plain web app: the app has to launch Chrome, control this PC, and encrypt secrets with the OS. A browser tab can't do any of that. Electron keeps the whole stack in Node and TypeScript, and Next.js still renders the UI. Tauri was the alternative; its Rust core would need a Node sidecar for Playwright anyway.

**Bots talking to each other.** Every bot has its own chat. A bot can call `message_bot(botId, text)`; the message lands in the target bot's chat marked "from Pnet apply", the target runs a turn, and its reply routes back as a tool result. The Team room (design B) is a shared thread that all selected bots read. Loop guard: at most 3 bot-to-bot hops per user message unless a routine allows more.

**Computer control.**
- Each bot gets its own Chrome user-data folder (`%LOCALAPPDATA%\Grokbot\profiles\<bot-id>`). It's launched with Playwright `launchPersistentContext(dir, { channel: 'chrome' })`, isolated from your personal Chrome.
- The bot acts through accessibility-snapshot tools (navigate, click by element ref, type, read page), the same approach as Playwright MCP. That's cheaper and more reliable than clicking on pixels.
- The panel shows a live CDP screencast of the bot's page with the bot's cursor.
- **Take over** pauses the bot and forwards your mouse and keyboard to the page (or brings the real Chrome window forward). **Pause** holds the bot at its next step. **Stop** ends the run and closes the profile's tabs.
- "Waiting for you" states (one-time PINs, CAPTCHAs) pause the bot and ask you to take over. The bot never tries to get around them.
- Later phases: whole-desktop control (screen capture plus OS input) and a virtual machine target (Windows Sandbox or Hyper-V, viewed over RDP or VNC).

**Logins vault.**
- Import from Chrome's password export (CSV), add by hand, or save when you log in during Take over.
- Stored encrypted with `safeStorage` (Windows DPAPI, bound to your Windows account). Passwords never enter prompts, logs or model context.
- The bot calls `fill_login(site)`. The main process looks up the login and types it into the page, and the model only learns "filled" or "no login for this site".
- Delete and export actions need confirmation. Every fill is written to the bot's activity log.

**AI.** AI SDK 7 via Vercel AI Gateway (one key). Each bot has its own model string. Default `anthropic/claude-sonnet-5`, with the slug confirmed at setup. v7 APIs get checked against `node_modules/ai/docs`.

## 5. Bot profile images (Nano Banana via `agy`)

The UI keeps the vector blobs, because they stay crisp at 20px, can be recolored and match your screenshots. Nano Banana makes the larger profile images: 3D clay-style renders of each bot's blob (same color and shape, with the two eyes) at 1024x1024. They're shown on the bot profile, the Agents board (design C) and in the New Bot creator. `agy` is installed. The nanobanana MCP server failed to connect, but the skill doesn't need it.

## 6. Motion (Emil + GSAP)

- **No animation:** anything triggered from the keyboard or repeated all day (switching bots, sending, Ctrl+K). These stay instant.
- **CSS transitions:** 150-250ms `cubic-bezier(0.23, 1, 0.32, 1)` for the panel open/close, tab switch and menus. Buttons get `scale(0.97)` on press.
- **GSAP (`@gsap/react`, `useGSAP` with scope):**
  - New bot-to-bot handoff rows fade and rise 8px in 220ms.
  - The computer panel expands 280ms with ease-out.
  - The Live indicator breathes only while a bot is live.
- **Reduced motion:** opacity only, no movement, no pulses.

## 7. Build phases (each ends with something to approve)

0. **Design pick.** You choose A, B or C. I finalize that direction, including a light-theme frame and the New Bot creator. *Gate.*
1. **Profile images.** Nano Banana clay renders of the roster's blobs, reviewed at 96 and 256px. *Gate.*
2. **Scaffold.**
   - `git init`, Next.js, shadcn init, Tailwind tokens from section 2.
   - Electron shell loading Next, then SQLite, then the Playwright install.
3. **UI shell.** Rail or list, chat, bubbles, composer and right panel matched to screenshot 3, checked with screenshot comparison.
4. **Bot runtime.** Chats, streaming, the bot-to-bot message bus with hop limit, routines scheduler. Tests for the bus and the scheduler.
5. **Computer service.** Per-bot Chrome profiles, screencast into the panel, Take over, Pause, Stop, Waiting for you. Tested against a local page.
6. **Logins vault.** CSV import, safeStorage encryption, `fill_login` tool, audit log. A test proves a password never appears in model input.
7. **Motion and QA.** GSAP pass, reduced motion, keyboard and screen reader pass, contrast, Windows installer build.

## 8. Risks

- **Controlling your real PC is powerful.** v1 limits bots to their own Chrome profiles. Full-desktop control comes later, behind an explicit per-bot permission.
- **Site terms.** Automating applications on job sites may conflict with some sites' terms. Bots stop at logins, PINs and CAPTCHAs and hand over to you.
- **Password safety.** DPAPI protects against other Windows users, not against malware running as you. The vault design keeps passwords out of the model and the logs.
- **Cost.** Bot-to-bot hops multiply model calls. The hop limit bounds it.
- **Fonts.** Wireframes use Inter; the app uses Segoe UI. Expect small metric differences between wireframes and the build.

## 9. Open questions (defaults in bold)

1. Which design: A, B or C? (My recommendation comes with the wireframes.)
2. Shell: **Electron**, Tauri, or browser plus a local runner?
3. Computer control in v1: **per-bot Chrome profile only**, or full PC control from the start?
4. Profile images: **3D clay renders of the blobs**, or something else (photoreal, illustrated)?
5. Models: **Claude Sonnet 5 via AI Gateway**, or another provider?
