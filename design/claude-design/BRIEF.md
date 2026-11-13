# Tjommie design directions: shared brief (Rust rewrite, Phase 0)

Three agents each build one design direction as a small design system. The owner compares them and picks one, then uploads it to Claude Design with `/design-sync`. Read `docs/superpowers/specs/2026-11-09-tjommie-rust-rewrite-spec.md` §5 "Phase 0" and §6 (parity matrix) first. For what the app does today, read `packages/ui/components/*` (chat, agents, exchange, room, routines, settings, rail, host-switcher) and `packages/ui/components/bot-avatar.tsx`. Look at today's app for features and content, not for style.

## 1. What Tjommie is
A Windows desktop app plus an Android phone app. You run a team of AI bots (Claude Code and Codex) on your own PC. Bots chat with you, use their own browser, ask before risky actions, talk to each other in rooms, and work on project teams. The phone pairs with the PC over LAN or a relay and shows the same bots. The desktop UI will be built in GPUI (Zed's GPU UI framework, fallback: Electron restyled), the phone UI in Jetpack Compose. The phone must look like the unmistakable sibling of the desktop.

## 2. Deliverable (in `design/claude-design/<your-slug>/`)
- `README.md`: direction name, the idea in one paragraph, the rendering table (below), the fonts with their licences, a motion note (what animates, duration, easing, reduced-motion fallback).
- `tokens.json`: colour for light AND dark themes, type scale, spacing scale, radius scale, elevation (shadows), motion durations and easings. Include the 10 bot colours (keys from `BotColor`). You may retune their values for your direction but keep each hue recognisable.
- `tokens.css`: the same tokens as CSS custom properties. Every HTML file uses only these variables.
- `components/*.html`: one self-contained HTML file per component group. The FIRST line of each file is `<!-- @dsCard group="<Group>" -->` (groups: Foundations, Navigation, Bots, Chat, Browser, Files, Overlays, Forms, Projects). Required components: colour and type specimen (light and dark side by side), app shell and navigation, host switcher, bot avatar (all 8 shapes x a few colours, sizes 20/28/40/64) with state badge, status pill for every `BotState`, message, thinking row, tool row, approval card, exchange (bot-to-bot) card, composer (idle, bot working = steer/queue hint, dictating with level meter, with attachment chip), browser panel chrome (tabs, address bar, Take control / Return to bot, logins menu, live indicator, private-mode blanked state), file card and preview frame, dialog, phone bottom sheet, toast, settings row, task card (project board), quota bar.
- `screens/desktop-chat.html` (1440x900), `screens/desktop-agents.html` (1440x900), `screens/phone-chat.html` (412x892), `screens/phone-agents.html` (412x892), all in your direction's primary theme, plus `screens/desktop-chat-alt.html` = desktop chat in the other theme.
- `shots/*.png`: a screenshot of every screen (desktop at 1x, phone at 2x). Render with headless Edge, for example:
  `"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe" --headless=new --disable-gpu --hide-scrollbars --virtual-time-budget=5000 --window-size=1440,900 --screenshot="<abs path>.png" "file:///<abs path>.html"` (phone: `--window-size=412,892 --force-device-scale-factor=2`).
  Look at every PNG with the Read tool and fix what you see (clipping, overflow, misalignment, weak contrast) before you finish.

## 3. Hard constraints
- **Renderable natively.** Use only what both GPUI and Compose draw: solid fills, linear gradients, borders, per-corner radii, box shadows, opacity, SVG paths, images, bundled fonts. No `backdrop-filter`/blur-behind, CSS `filter`, blend modes, conic or radial gradients, text clipping effects, noise or grain. The README rendering table maps every visual effect you used to a GPUI primitive and a Compose primitive; anything without both is removed.
- **Bot avatars keep their shapes.** Use the exact body and eye paths from `packages/ui/components/bot-avatar.tsx` (8 shapes, 40x40 box, body filled with the bot colour, two white eye strokes). You may change size, framing, status badges and surroundings, not the shapes.
- **Real states and names.** `BotState`: idle, working, waiting (label "Needs you"), paused, limited (usage limit hit), needs_setup. `Autonomy`: ask, folder, full. Providers: Claude Code, Codex. `ProjectRole`: lead, developer, reviewer, tester. Phone push kinds: approval, limit, routine_failed, done, failed.
- **Windows desktop window**: draw your own title bar with Windows caption buttons (minimise, maximise, close) top right.
- Fonts: OFL or Apache licensed (the app bundles them). Loading them from Google Fonts in the HTML is fine. Icons: Lucide, inline SVG, one family.
- Contrast: body text at least 4.5:1, large text and UI glyphs at least 3:1, in both themes. Visible focus ring. Phone touch targets at least 48 dp; desktop icon buttons at least 28 px.
- Copy: no em or en dashes in visible text, no emoji, no lorem ipsum, no "Section 01" labels, no decorative dots (a dot only when it shows real state). Use the scenario below word for word so the three directions compare like for like.
- Motion (note only, static frames): UI transitions under 300 ms, ease-out, nothing animates on keyboard actions, live pulse only while live, reduced-motion fallback.

## 4. Shared scenario (use in every screen)
Hosts: "This PC" (connected, LAN) and "Studio PC" (paired, via relay, online).

| Bot | Provider | Shape, colour | State | Activity line |
|---|---|---|---|---|
| Forge | Codex | hexagon, blue | waiting ("Needs you") | Wants to run git push |
| Scout | Claude Code | blob, teal | working | Researching GPUI docks |
| Quill | Claude Code | droplet, purple | working | Reviewing task 14 |
| Ledger | Codex | rounded-square, amber | idle | Last active 18 min ago |
| Pixel | Claude Code | cloud, pink | limited | 5-hour limit, resets 23:40 |
| Atlas | Codex | triangle, green | paused | Paused by you |
| Nova | Claude Code | circle, gray | needs_setup | Sign in to finish setup |

Rooms: "Everyone" (7 bots), "Launch crew" (Forge, Scout, Quill). Project: "tjommie-rs" (Scout lead, Forge developer, Quill reviewer, Ledger tester), 14 tasks, 9 done. Usage today: $4.12 of a $20.00 cap; Codex 5-hour window 62% used, Claude 7-day window 38% used.

**Chat with Forge** (desktop-chat, phone-chat shows the last part):
1. You, 10:42: "Port the migrations array from packages/daemon/src/db.ts to the Rust daemon. Keep the SQL byte-identical and add the fixture test."
2. Forge: thinking row, collapsed, "Thought for 12s".
3. Tool rows, all done: "Read packages/daemon/src/db.ts"; "Edited tjommie-rs/crates/daemon/src/db.rs" (+184, no removals); "Ran cargo test -p daemon" with "42 passed".
4. Forge message: "Ported all 12 migrations. `open_db` now runs each one in a transaction and bumps `user_version`." then this code block:
```rust
for (i, sql) in MIGRATIONS.iter().enumerate().skip(version) {
    let tx = conn.transaction()?;
    tx.execute_batch(sql)?;
    tx.pragma_update(None, "user_version", i + 1)?;
    tx.commit()?;
}
```
5. Exchange card: Forge asked Scout "Does the bundled SQLite in rusqlite match the version node:sqlite ships?", Scout replied "Yes, both are 3.50. Safe to compare sqlite_master output." (2 messages, "View thread").
6. Approval card, pending: "Forge wants to run a command" `git push origin rust-rewrite`, autonomy "Ask", buttons Deny / Approve.
7. Composer: placeholder "Message Forge", hint "Forge is waiting for your approval"; attach, dictate, @mention, send.

**Desktop browser panel** (right side of desktop-chat): panel tabs Browser, Details, Routines, Files (Browser selected). Two tabs: "rusqlite::Transaction - docs.rs" and "Actions - tjommie-rs". Address `docs.rs/rusqlite/latest/rusqlite/struct.Transaction.html`. Banner "Forge is using this browser" with "Take control". Live indicator "Live, 58 fps". Logins menu button. Draw the page as a simple, plausible docs page.

**Desktop agents overview**: all 7 bots as cards or rows with avatar, name, provider, state, activity line, a usage hint where relevant, and quick actions. A "Needs you" area (Forge approval, Nova setup). "New bot" action. Usage summary (today's spend, the two windows).

**Phone agents home**: host pill "This PC, LAN", "Needs you" section (Forge, Nova), bot list with states, bottom navigation: Agents, Rooms, Projects, Settings.

**Phone chat**: Forge, top bar with back, avatar, name, state, host pill; the tool rows (compact), the message with code, the exchange card, the pending approval, composer with dictate; an entry point to view Forge's browser live.

## 5. Finish
End with a short report: the direction in two sentences, the files written, anything you could not render natively and removed, and the absolute paths of the shots.
