# Tjommie Phase 1E: Screens, Installer and Demo Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Tjommie gets every other Phase 1 screen, a Windows installer, screenshot checks and the Phase 1 demo:
- **Screens:**
  - New bot (with Suggestions) and Bot settings;
  - the Team room with @mentions;
  - the exchange view;
  - the routine editor;
  - first-run setup, including setting up Codex's sandbox;
  - Settings;
  - the Agents overview.
- **Screenshot checks:** screenshots of the running app, taken with real bots, are checked against the wireframes.
- **Phase 1 demo:** it runs on the installed app.

**Architecture:**
- **UI pages** (`packages/ui`): they sit on plan 1D's app state, daemon link and chat components. Pure helpers in `lib/` hold the logic and carry the unit tests; components stay thin.
- **Three small backend changes:**
  - The daemon gains `bots.notes`, which reads a bot's `notes.md`.
  - The desktop bridge gains a folder picker and a notes opener.
  - The @mention matcher moves from the daemon into `@tjommie/protocol`, so the UI's hints and highlights match the daemon exactly.
- **Packaging:**
  - `npm run package` bundles the Electron main process and the daemon with rolldown, then builds an NSIS installer with electron-builder.
  - The daemon, the UI export, the icons and Playwright MCP ship as plain files under `resources`.

**Tech Stack:** as plan 1D, plus:
- electron-builder 26 and rolldown 1.2;
- Base UI 1.8's Dialog, Switch, RadioGroup and Popover.

**Spec:** `docs/superpowers/specs/2026-09-25-tjommie-design.md`:
- §2 how people use it; §3 the installer; §4.1 a bot;
- §5.4 autonomy, and what "Own folder" protects; §5.8 the setup check;
- §6.2 the Team room; §7 routines and Suggestions;
- §12.1 and §12.2 the screens and the Phase 1 demo; §14 testing.

The screens are in `docs/design/wireframe-spec.md` ("Phase 1 screens (plan 1C)") and `docs/wireframes/`.

## Where this plan sits

| Plan | Delivers | Status |
|---|---|---|
| 1A Daemon core | Bots, chat, both CLIs, steering, Stop, approvals, sign-in | Built |
| 1B Bots working together | MCP server, exchanges, Team room, routines, browser | Built |
| 1C Screen designs | Every Phase 1 frame, in `docs/wireframes/` | Done |
| 1D Desktop shell and chat | Electron shell, daemon link, rail, the chat, the bot panel, tray and notifications | Written; runs before this plan |
| **1E Screens, installer and demo (this plan)** | Every other Phase 1 screen, the installer, screenshot checks, the Phase 1 demo | |

This plan builds on plan 1D's finished code on branch `implement-phase-1-multi-agent`. Paths are relative to the repo root.

## Facts this plan relies on

Checked on 2026-09-27 against the branch, its installed packages and the npm registry.

**The daemon's API for these screens**
- **The Team room:**
  - `room.post({ text })` posts as you, and each bot the text @mentions answers in the room.
  - A mention is `@` plus a whole bot name, in any case, longest names first. `@everyone` mentions every bot.
  - A mention can't touch letters, digits or `_` on either side, or follow `.`, `-`, `+` or `%`, so email addresses don't count. This is `findMentions` in `packages/daemon/src/team.ts`.
- **Exchanges:**
  - `exchanges.get({ id })` returns `{ exchange, messages, hasMore }`: the newest 200 messages, oldest first, with no paging.
  - The exchange's own conversation holds the request, then the asked bot's activity rows, reply and file cards.
  - The room gets two posts: the request (data `{ exchangeId, toBotId }`) and the reply, whose data adds `files` (`{ name, path }`, no size). A failed exchange puts a notice in the room instead.
  - Both bots' handoff rows are updated with the message count and status.
- **Routines:**
  - The calls are `routines.create`, `update`, `delete` and `runs`. The time zone defaults to the host's.
  - Run notes include "Stopped at the time limit.", "Stopped by you." and "<bot> was paused.". A failed run's note is its error.
- **Bots:**
  - `bots.create` takes `BotInput`, and autonomy defaults to "folder".
  - It rejects a name already used, in any case, with `name_taken`.
  - It rejects a project folder with `bad_params` if the folder is missing, inside Tjommie's bots folder, or contains the data folder.
  - `bots.update` needs `model` whenever `provider` changes.
  - A new bot's home folder gets `notes.md` with `# <name>'s notes`.
- **Models:** `models.list({ provider })`.
  - Claude: `sonnet`, `opus` and `haiku`, labeled "Claude Sonnet (latest)" and so on.
  - Codex: its app-server supplies the list, which takes a few seconds.
- **Setup:**
  - `ProviderSetup` has `installed`, `version`, `signedIn`, `account`, `plan` and `error`, plus `sandbox` for Codex.
  - What `error` holds:

    | Case | `error` |
    |---|---|
    | The CLI is missing | "Claude Code is not installed" (or Codex) |
    | A check failed | A short reason |
    | Signed out | `null` |

  - `setup.signIn` itself opens Claude's console window or Codex's browser page. The daemon broadcasts `setup.changed` when sign-in finishes.
  - `setup.prepareCodexSandbox` runs Codex's own setup, which shows a Windows admin prompt.
- **Suggestions** live in the UI. The New bot screen calls `bots.create`, then `routines.create` (per the final review's spec-drift notes).

**Packaging**
- **Installer tooling:** spec §3 names electron-builder, which is at 26.15.3. rolldown 1.2.11 is already installed (Vite depends on it); it exports `rolldown()`, which takes `platform: "node"` and writes ESM.
- **SQLite:** Electron 44.4.5's own Node is 24.21.0 (run with `ELECTRON_RUN_AS_NODE=1`). It has `node:sqlite` (SQLite 3.53.4), so the packaged daemon needs no native build.
- **Child processes:** inside Electron, the daemon already starts the CLIs and Playwright MCP through `process.execPath`, with `ELECTRON_RUN_AS_NODE=1`.
- **Playwright MCP:**
  - The daemon finds it at runtime: `createRequire(import.meta.url).resolve("@playwright/mcp/package.json")`.
  - So it must sit on disk next to the daemon's bundle, with `playwright` and `playwright-core`: about 19 MB together.
  - The daemon has no dynamic imports.
- **The Electron binary:** this worktree lacks `node_modules/electron/dist`. Run `node node_modules/electron/install.js` once, as plan 1D says.

**Base UI 1.8** (`node_modules/@base-ui/react`)
- **Dialog:** Root, Portal, Backdrop, Popup and Title. Popup takes `initialFocus`, a ref.
- **Switch:** Root and Thumb, with `data-checked`.
- **RadioGroup** takes `value` and `onValueChange`, with `Radio.Root` (`data-checked`).
- **Popover:** Root, Trigger, Portal, Positioner and Popup.
- **Menu:** as in plan 1D.

**lucide-react 1.48** has every icon used here, including SlidersHorizontal, Bot, Terminal, MessageCircle, CircleDashed, CircleCheck, Trash2, Minus, ChevronRight, Search, Users, Info and TimerOff.

## Global Constraints

- **Plan 1D's constraints still apply:** tokens and the light theme, motion, copy, renderer security, the two desktop modes, lifetimes and the test commands.
- **Dependencies:** add exactly `electron-builder@^26.15.3` and `rolldown@^1.2.11` as root dev dependencies, nothing else.
- **Colors:**
  - Two tokens join plan 1D's table: `drawer` (`#0B0B0B` dark, `#F7F7F7` light) for the Settings nav, and `nav-selected` (`#1F1F1F`, `#EBEBEB`).
  - The dialog scrim is black at 60% in both themes.
  - WF2's `#8A8A8A` labels use the `caption` token (`#909090`); the difference doesn't show.
- **Routes:**
  - Pages: `/`, `/chat/?bot=`, `/bots/new/`, `/bots/settings/?bot=`, `/room/`, `/agents/` and `/settings/` (plus `?section=bots`).
  - The exchange view has no route. It opens in the right panel of a chat or the Team room.
- **Daemon calls:** 1E's code calls the daemon through `request()` (Task 3), which turns a missing connection into a rejected promise. A page opened while Tjommie reconnects then shows an error instead of crashing.
- **Installer:**
  - It's per user, one click, with app id `com.tsnyders.tjommie`.
  - It's unsigned, so Windows SmartScreen warns once; the README says so.
  - Uninstalling keeps `%LOCALAPPDATA%\Tjommie`, so bots survive a reinstall.
- **Screenshots:**
  - They come from the running app with real bots only (the user's choice), with no fake data and no injected messages.
  - A state the real bots can't be made to show (Limited, a failed run) is listed with the test that covers it.

## Review Focus

These are the five things most likely to go wrong for a person using these screens, most likely first. Each has a test in the task that owns the code.

1. **The Team room's hint and the daemon disagree about a mention.** The hint says nobody will reply, then a bot answers, or the other way round. *Test: Task 1, "the UI and the daemon read mentions the same way".*
2. **Save changes overwrites something changed elsewhere.** Bot settings must send only what the person changed, never the provider or Can message list the form opened with. *Test: Task 5, "sends only what the person changed".*
3. **A Suggestion that fails validation after its bot exists.** Create bot would leave a half-made bot. *Test: Task 5, "every suggestion is a valid bot with valid routines".*
4. **The installed app can't find its daemon, UI or Playwright MCP.** Packaged paths differ from development. *Test: Task 14, "the packaged app starts its own daemon and shows first-run setup".*
5. **The window asks the desktop app to open a file that isn't a bot's notes.** *Test: Task 2, "opens only a bot's own notes.md".*

---

### Task 1: One mention matcher for the daemon and the UI

**Files:**
- Create: `packages/protocol/src/mentions.ts`, `packages/protocol/src/mentions.test.ts`
- Modify: `packages/protocol/src/index.ts`, `packages/daemon/src/team.ts`, `packages/daemon/src/team.test.ts`

**Interfaces:**
- Consumes: `findMentions` from `packages/daemon/src/team.ts`, as it is today.
- Produces, from `@tjommie/protocol`:
  - `mentionRanges(text, names): MentionRange[]`, where `MentionRange` is `{ start, end, name }` and `name` is null for `@everyone`.
  - `findMentions(text, bots): string[]`: the mentioned bots' ids, in the order given.

- [ ] **Step 1: Write the failing tests**

Create `packages/protocol/src/mentions.test.ts`. The first test is the daemon's `findMentions` test, moved here unchanged:

```ts
import { describe, expect, it } from "vitest";
import { findMentions, mentionRanges } from "./mentions.ts";

describe("mentions", () => {
  const bots = [
    { id: "w", name: "Writer" },
    { id: "cv", name: "CV Writer" },
    { id: "b", name: "Bot" },
    { id: "b2", name: "Bot 2" },
  ];

  it("matches whole names, longest first, and ignores email addresses (the UI and the daemon read mentions the same way)", () => {
    expect(findMentions("@CV Writer can you help?", bots)).toEqual(["cv"]);
    expect(findMentions("@writer and @cv writer!", bots)).toEqual(["w", "cv"]);
    expect(findMentions("@Bot 2 and @Bot 2 again", bots)).toEqual(["b2"]);
    expect(findMentions("mail me@writer.com or @Writers", bots)).toEqual([]);
    expect(findMentions("mail me_@writer.com or me+@writer.com or @Writer_extra", bots)).toEqual([]);
    expect(findMentions("@everyone stop", bots)).toEqual(["w", "cv", "b", "b2"]);
    expect(findMentions("@everyone stop", [...bots, { id: "all", name: "Everyone" }])).toEqual(["w", "cv", "b", "b2", "all"]);
  });

  it("finds where each mention is, for highlighting", () => {
    expect(mentionRanges("@CV Writer add it. @everyone skip seniors.", ["Writer", "CV Writer"])).toEqual([
      { start: 0, end: 10, name: "CV Writer" },
      { start: 19, end: 28, name: null },
    ]);
    expect(mentionRanges("no mentions here", ["Writer"])).toEqual([]);
  });
});
```

- [ ] **Step 2: Run them to see them fail**

Run: `npx vitest run packages/protocol/src/mentions.test.ts`
Expected: FAIL. `./mentions.ts` can't be found.

- [ ] **Step 3: Write the matcher**

Create `packages/protocol/src/mentions.ts`:

```ts
// @mentions in Team room posts. The daemon uses them to pick who answers, and the UI uses the same rule for its
// hint and its highlighting, so the two never disagree.
const WORD = /[\p{L}\p{N}\p{M}_]/u;
const EMAIL_END = /[.\-+%]/u;

export type MentionRange = { start: number; end: number; name: string | null }; // name null: @everyone

// Each "@everyone" and "@Name" (whole names, any case, longest first) in the text, in order. A mention can't
// touch letters, digits or "_" on either side, or follow an email-like ".", "-", "+" or "%".
export function mentionRanges(text: string, names: string[]): MentionRange[] {
  // ponytail: offsets come from the lowercased text, so they drift after a character whose lowercase is longer.
  let rest = text.toLowerCase();
  const ranges: MentionRange[] = [];
  const take = (tag: string, name: string | null): void => {
    for (let at = rest.indexOf(tag); at !== -1; at = rest.indexOf(tag, at + tag.length)) {
      const before = rest[at - 1] ?? "";
      if (WORD.test(before) || EMAIL_END.test(before) || WORD.test(rest[at + tag.length] ?? "")) continue;
      rest = rest.slice(0, at) + " ".repeat(tag.length) + rest.slice(at + tag.length);
      ranges.push({ start: at, end: at + tag.length, name });
    }
  };
  take("@everyone", null);
  for (const name of [...names].sort((a, b) => b.length - a.length)) take(`@${name.toLowerCase()}`, name);
  return ranges.sort((a, b) => a.start - b.start);
}

// The ids of the bots a post mentions, in the order given. "@everyone" mentions every bot.
export function findMentions(text: string, bots: { id: string; name: string }[]): string[] {
  const ranges = mentionRanges(text, bots.map((bot) => bot.name));
  if (ranges.some((range) => range.name === null)) return bots.map((bot) => bot.id);
  const named = new Set(ranges.map((range) => range.name));
  return bots.filter((bot) => named.has(bot.name)).map((bot) => bot.id);
}
```

In `packages/protocol/src/index.ts`, add the last line:

```ts
export * from "./mentions.ts";
```

- [ ] **Step 4: Point the daemon at it**

In `packages/daemon/src/team.ts`:
1. Add `findMentions` to the value imports from `@tjommie/protocol`. Its first line becomes `ExchangeHandoffData, FileData, RoutineApprovalData, RoutineInput, TEAM_ROOM_ID, findMentions,`.
2. Delete the two constants `WORD` and `EMAIL_END`.
3. Delete the whole `findMentions` function, including its comment ("The bots a Team room post mentions: ..."). `postToRoom` keeps calling `findMentions(text, listStoredBots(this.#db))` unchanged.

In `packages/daemon/src/team.test.ts`:
1. Change the import from `./team.ts` to `import { Team, fileInside, getExchange } from "./team.ts";`.
2. Delete the `describe("findMentions", ...)` block, whose cases now live in the protocol test.

- [ ] **Step 5: Run the tests and checks**

Run: `npx vitest run packages/protocol packages/daemon/src/team.test.ts packages/daemon/src/team.routing.test.ts`
Expected: PASS, including the 2 mention tests.

Run: `npm test` and `npm run typecheck`
Expected: PASS, no errors.

- [ ] **Step 6: Commit**

```bash
git add packages/protocol packages/daemon/src/team.ts packages/daemon/src/team.test.ts
git commit -m "Share the Team room mention matcher between the daemon and the UI

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Notes and folders for the bot screens

**Files:**
- Modify: `packages/protocol/src/wire.ts`, `packages/daemon/src/daemon.ts`, `packages/daemon/src/daemon.test.ts`, `packages/desktop/src/main.ts`, `packages/desktop/src/preload.cjs`, `packages/ui/lib/desktop.ts`
- Create: `packages/desktop/src/bot-files.ts`, `packages/desktop/src/bot-files.test.ts`

**Interfaces:**
- Consumes: the daemon's `getStoredBot`, and plan 1D's `main.ts` (its `dataRoot`, `win` and `registerIpc`).
- Produces:
  - `bots.notes({ id }) → { text }`: the first 4,000 characters of the bot's `notes.md`, or `""` when there isn't one.
  - `notesFile(botsDir, homeFolder): string | null` in `packages/desktop/src/bot-files.ts`.
  - Two calls on the bridge, typed in `lib/desktop.ts`:
    - `pickFolder(): Promise<string | null>`, a folder picker;
    - `openNotes(homeFolder): Promise<void>`, which opens that bot's `notes.md` in the default editor.

- [ ] **Step 1: Write the failing tests**

Add this test at the end of `packages/daemon/src/daemon.test.ts`:

```ts
it("reads the start of a bot's notes.md for Bot settings", async () => {
  const { client } = await harness();
  const bot = await client.call("bots.create", input("Noter"));
  expect(await client.call("bots.notes", { id: bot.id })).toEqual({ text: "# Noter's notes\n" });
  writeFileSync(join(bot.homeFolder, "notes.md"), "x".repeat(5000));
  expect((await client.call("bots.notes", { id: bot.id })).text).toHaveLength(4000);
  rmSync(join(bot.homeFolder, "notes.md"));
  expect(await client.call("bots.notes", { id: bot.id })).toEqual({ text: "" });
  await expect(client.call("bots.notes", { id: "missing" })).rejects.toThrow(/not_found/);
});
```

Create `packages/desktop/src/bot-files.test.ts`:

```ts
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { notesFile } from "./bot-files.ts";

describe("notesFile", () => {
  const root = mkdtempSync(join(tmpdir(), "tjommie notes ñ-"));
  const bots = join(root, "bots");
  const home = join(bots, "b1");
  mkdirSync(join(home, "inbox"), { recursive: true });
  writeFileSync(join(home, "notes.md"), "# b1's notes\n");
  writeFileSync(join(home, "inbox", "notes.md"), "an attachment");
  mkdirSync(join(bots, "b2"), { recursive: true });
  mkdirSync(join(root, "elsewhere"), { recursive: true });
  writeFileSync(join(root, "elsewhere", "notes.md"), "not a bot");

  it("opens only a bot's own notes.md", () => {
    expect(notesFile(bots, home)).toBe(join(home, "notes.md"));
    expect(notesFile(bots, join(home, "inbox"))).toBeNull();
    expect(notesFile(bots, join(root, "elsewhere"))).toBeNull();
    expect(notesFile(bots, join(bots, "..", "elsewhere"))).toBeNull();
    expect(notesFile(bots, join(bots, "b2"))).toBeNull(); // no notes.md there
    expect(notesFile(bots, bots)).toBeNull();
  });
});
```

- [ ] **Step 2: Run them to see them fail**

Run: `npx vitest run packages/daemon/src/daemon.test.ts -t "notes" packages/desktop/src/bot-files.test.ts`
Expected: FAIL. The daemon doesn't know the method `bots.notes`, and `./bot-files.ts` can't be found.

- [ ] **Step 3: Add the daemon method**

In `packages/protocol/src/wire.ts`, add this entry to `methods`, after `"bots.showBrowser"`:

```ts
  "bots.notes": { params: Id, result: z.object({ text: z.string() }) },
```

In `packages/daemon/src/daemon.ts`:
1. Change the `node:fs` import to `import { existsSync, lstatSync, readFileSync, rmSync } from "node:fs";`, and add `import { join } from "node:path";` below it.
2. Under the imports, add `const NOTES_PREVIEW_CHARS = 4_000;`.
3. Add this handler after `"bots.showBrowser"`:

```ts
    // Bot settings shows the start of the bot's own notes.
    "bots.notes": ({ id }) => {
      const file = join(getStoredBot(db, id).homeFolder, "notes.md");
      return { text: existsSync(file) ? readFileSync(file, "utf8").slice(0, NOTES_PREVIEW_CHARS) : "" };
    },
```

- [ ] **Step 4: Add the bridge calls**

Create `packages/desktop/src/bot-files.ts`:

```ts
import { existsSync, statSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";

// The window may ask to open one kind of file: a bot's notes.md, in a home folder directly inside Tjommie's bots
// folder. Anything else is refused.
export function notesFile(botsDir: string, homeFolder: string): string | null {
  const home = resolve(homeFolder);
  if (relative(resolve(botsDir), dirname(home)) !== "") return null;
  const file = join(home, "notes.md");
  return existsSync(file) && statSync(file).isFile() ? file : null;
}
```

In `packages/desktop/src/main.ts`:
1. Add `import { notesFile } from "./bot-files.ts";` with the other local imports.
2. Add these two handlers at the end of `registerIpc()`:

```ts
  ipcMain.handle("folder:pick", async () => {
    const options: Electron.OpenDialogOptions = { properties: ["openDirectory"] };
    const result = win ? await dialog.showOpenDialog(win, options) : await dialog.showOpenDialog(options);
    return result.canceled ? null : (result.filePaths[0] ?? null);
  });
  ipcMain.handle("notes:open", async (_event, homeFolder: unknown) => {
    const file = typeof homeFolder === "string" ? notesFile(join(dataRoot, "bots"), homeFolder) : null;
    if (file) await shell.openPath(file);
  });
```

In `packages/desktop/src/preload.cjs`, add these two lines to the object passed to `exposeInMainWorld`, after `openFolder`:

```js
  pickFolder: () => ipcRenderer.invoke("folder:pick"),
  openNotes: (homeFolder) => ipcRenderer.invoke("notes:open", homeFolder),
```

In `packages/ui/lib/desktop.ts`, add these two lines to `DesktopApi`, after `openFolder`:

```ts
  pickFolder(): Promise<string | null>;
  openNotes(homeFolder: string): Promise<void>;
```

- [ ] **Step 5: Run the tests and checks**

Run: `npx vitest run packages/daemon/src/daemon.test.ts packages/desktop/src`
Expected: PASS, including "reads the start of a bot's notes.md for Bot settings" and "opens only a bot's own notes.md".

Run: `npm test`, `npm run typecheck` and `npm run lint`
Expected: PASS, no errors.

- [ ] **Step 6: Commit**

```bash
git add packages/protocol/src/wire.ts packages/daemon/src/daemon.ts packages/daemon/src/daemon.test.ts packages/desktop/src packages/ui/lib/desktop.ts
git commit -m "Add a notes preview to the daemon and a folder picker and notes opener to the bridge

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Shared pieces for the new screens

**Files:**
- Modify: `packages/ui/lib/format.ts`, `packages/ui/lib/format.test.ts`, `packages/ui/lib/labels.ts`, `packages/ui/lib/connection.ts`, `packages/ui/app/globals.css`, `packages/ui/components/chat/chat-header.tsx`
- Create: `packages/ui/components/controls.tsx`, `packages/ui/components/dialog.tsx`, `packages/ui/components/menu.tsx`

**Interfaces:**
- Consumes: plan 1D's `lib/format.ts`, `lib/labels.ts`, `lib/connection.ts`, `components/pill.tsx` and `components/chat/chat-header.tsx`.
- Produces:
  - **`lib/format.ts`:**
    - `dayName` also returns "Tomorrow", and weekdays up to a week ahead.
    - New: `longDate(value)` ("Monday 21 September"), `daysLabel(days)` ("Weekdays"), `timesLabel(times)` ("8:48 AM, 4:45 PM"), `timeAgo(value, now?)` ("4 minutes ago") and `duration(ms)` ("2h 51m").
    - `schedule` is now built from `daysLabel` and `timesLabel`.
  - **`lib/labels.ts`:** `SHORT_STATE` (for example "Limited") and `STATE_TEXT` (the color class for a state's words).
  - **`lib/connection.ts`:**
    - `request(method, params)`: like `daemon().call`, but rejects instead of throwing when Tjommie isn't connected.
    - `roomHistory(before?)`, and `exchangePage(exchangeId)` for `loadConversation`.
  - **`components/controls.tsx`:**
    - `FIELD`, the input class, and `Label`;
    - `Switch({ label, checked, onChange, disabled? })`;
    - `Segmented({ label, value, options, onChange })`;
    - `Stepper({ label, value, min, max, onChange })`.
  - **`components/dialog.tsx`:** `Dialog({ open, onOpenChange, title, width?, actions?, closable?, initialFocus?, children })` and `ConfirmDialog({ open, onOpenChange, title, body, confirm, danger?, onConfirm })`.
  - **`components/menu.tsx`:** `MENU_POPUP` and `MENU_ITEM`, the class names of plan 1D's More menu.
  - **`chat-header.tsx`:** it now exports `HeaderState({ bot })`.
  - **Tokens:** `bg-drawer`, `bg-nav-selected`. Each theme also sets `color-scheme`, so native controls (the time picker) match.
  - **Drag strips:** radio groups, switches and comboboxes in a header strip stay clickable instead of dragging the window.

- [ ] **Step 1: Write the failing tests**

Add these tests inside the `describe("format", ...)` block in `packages/ui/lib/format.test.ts`:

```ts
  it("names tomorrow and the coming week, and writes long dates", () => {
    expect(dayName(new Date(2026, 8, 28, 9, 0), now)).toBe("Tomorrow");
    expect(dayName(new Date(2026, 8, 30, 9, 0), now)).toBe("Wednesday");
    expect(longDate(new Date(2026, 8, 21, 10, 0))).toBe("Monday 21 September");
  });

  it("says how long ago and how long", () => {
    expect(timeAgo(new Date(2026, 8, 27, 13, 59, 30), now)).toBe("just now");
    expect(timeAgo(new Date(2026, 8, 27, 13, 56), now)).toBe("4 minutes ago");
    expect(timeAgo(new Date(2026, 8, 27, 12, 0), now)).toBe("2 hours ago");
    expect(timeAgo(new Date(2026, 8, 26, 9, 0), now)).toBe("yesterday");
    expect(duration((2 * 60 + 51) * 60_000)).toBe("2h 51m");
    expect(duration(3 * 3_600_000)).toBe("3h");
    expect(duration(4 * 60_000)).toBe("4m");
    expect(duration(20_000)).toBe("under a minute");
    expect(daysLabel([6, 0])).toBe("Weekends");
    expect(timesLabel(["16:45", "08:48", "16:45"])).toBe("8:48 AM, 4:45 PM");
  });
```

Change the file's first import from `./format` to:

```ts
import { clock, dayName, daysLabel, duration, fileSize, longDate, routineTitle, schedule, stamp, timeAgo, timesLabel } from "./format";
```

- [ ] **Step 2: Run them to see them fail**

Run: `npx vitest run packages/ui/lib/format.test.ts`
Expected: FAIL. `longDate`, `timeAgo`, `duration`, `daysLabel` and `timesLabel` aren't exported, and `dayName` doesn't return "Tomorrow".

- [ ] **Step 3: Extend the formatting**

In `packages/ui/lib/format.ts`, replace `dayName` and `schedule` with:

```ts
// "Today", "Yesterday", "Tomorrow", a weekday within a week either way, otherwise "21 September".
export function dayName(value: string | Date, now: Date = new Date()): string {
  const date = new Date(value);
  const days = Math.round((startOfDay(now) - startOfDay(date)) / DAY_MS);
  if (days === 0) return "Today";
  if (days === 1) return "Yesterday";
  if (days === -1) return "Tomorrow";
  if (Math.abs(days) < 7) return WEEKDAY.format(date);
  return DAY_MONTH.format(date);
}

// "Monday 21 September".
export function longDate(value: string | Date): string {
  const date = new Date(value);
  return `${WEEKDAY.format(date)} ${DAY_MONTH.format(date)}`;
}

// "Every day", "Weekdays", "Weekends" or "Mon, Wed".
export function daysLabel(days: number[]): string {
  const sorted = [...new Set(days)].sort((a, b) => a - b);
  const key = sorted.join("");
  if (key === "0123456") return "Every day";
  if (key === "12345") return "Weekdays";
  if (key === "06") return "Weekends";
  return sorted.map((day) => DAY_SHORT[day]).join(", ");
}

// "8:48 AM, 4:45 PM" from "HH:MM" times.
export function timesLabel(times: string[]): string {
  return [...new Set(times)]
    .sort()
    .map((time) => clock(new Date(2000, 0, 1, Number(time.slice(0, 2)), Number(time.slice(3)))))
    .join(", ");
}

// "Weekdays at 8:48 AM", "Every day at 7:00 AM, 5:00 PM", "Mon, Wed at 9:00 AM".
export const schedule = (days: number[], times: string[]): string => `${daysLabel(days)} at ${timesLabel(times)}`;

// "just now", "4 minutes ago", "2 hours ago", "yesterday", or "on Monday".
export function timeAgo(value: string | Date, now: Date = new Date()): string {
  const minutes = Math.floor((now.getTime() - new Date(value).getTime()) / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} ${minutes === 1 ? "minute" : "minutes"} ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} ${hours === 1 ? "hour" : "hours"} ago`;
  const day = dayName(value, now);
  return day === "Yesterday" ? "yesterday" : `on ${day}`;
}

// How long something ran: "2h 51m", "3h", "4m" or "under a minute".
export function duration(ms: number): string {
  const minutes = Math.round(ms / 60_000);
  if (minutes < 1) return "under a minute";
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  if (!hours) return `${rest}m`;
  return rest ? `${hours}h ${rest}m` : `${hours}h`;
}
```

- [ ] **Step 4: Add the labels, the request helper and the tokens**

In `packages/ui/lib/labels.ts`, change the protocol import to `import type { Autonomy, Bot, BotState } from "@tjommie/protocol";` and add:

```ts
// The short state, for tight rows (the mention popover, the Agents overview's chips).
export const SHORT_STATE: Record<BotState, string> = {
  idle: "Idle",
  working: "Working",
  waiting: "Waiting",
  paused: "Paused",
  limited: "Limited",
  needs_setup: "Needs setup",
};

// The color of a state's words in lists (WF5's members, the mention popover).
export const STATE_TEXT: Record<BotState, string> = {
  idle: "text-secondary",
  working: "text-working",
  waiting: "text-accent",
  paused: "text-secondary",
  limited: "text-secondary",
  needs_setup: "text-secondary",
};
```

In `packages/ui/lib/connection.ts`:
1. Change the protocol import to `import type { Message, MethodName, ParamsIn, Result } from "@tjommie/protocol";`.
2. Add, after `daemon()`:

```ts
// Calls the daemon, or rejects when Tjommie isn't connected, so a call made from an effect never throws.
export function request<M extends MethodName>(method: M, params: ParamsIn<M>): Promise<Result<M>> {
  return client ? client.call(method, params) : Promise.reject(new Error("Tjommie isn't connected right now."));
}
```

3. Add, after `botHistory`:

```ts
// The Team room's pages, newest first.
export const roomHistory = (before?: string): Promise<Page> => request("room.history", { before, limit: PAGE });

// An exchange's messages: the newest 200, with no older pages to load.
export const exchangePage =
  (exchangeId: string) =>
  async (): Promise<Page> => {
    const { messages } = await request("exchanges.get", { id: exchangeId });
    return { messages, hasMore: false };
  };
```

In `packages/ui/app/globals.css`:
1. In `:root`, add `--drawer: #f7f7f7;`, `--nav-selected: #ebebeb;` and `color-scheme: light;`.
2. In `.dark`, add `--drawer: #0b0b0b;`, `--nav-selected: #1f1f1f;` and `color-scheme: dark;`.
3. In `@theme inline`, add `--color-drawer: var(--drawer);` and `--color-nav-selected: var(--nav-selected);`.
4. The new controls sit in header strips that drag the window, so they must stay clickable. Replace the `.drag :where(...)` selector with:

```css
.drag :where(a, button, input, textarea, select, [role="button"], [role="menu"], [role="radiogroup"], [role="switch"], [role="combobox"]) {
  -webkit-app-region: no-drag;
}
```

- [ ] **Step 5: Write the controls, the dialog and the menu styles**

Create `packages/ui/components/controls.tsx`:

```tsx
"use client";

import type { ReactNode } from "react";
import { Minus, Plus } from "lucide-react";
import { Radio } from "@base-ui/react/radio";
import { RadioGroup } from "@base-ui/react/radio-group";
import { Switch as BaseSwitch } from "@base-ui/react/switch";

// Text inputs and areas (WF3): radius 10 on the card fill, with a 1 px stroke. Add h-11 for one line. The global
// focus ring draws inside the box, over the stroke.
export const FIELD = "w-full rounded-[10px] border border-input-stroke bg-card px-4 text-[14px] leading-[21px] text-text placeholder:text-placeholder";

export function Label({ htmlFor, children }: { htmlFor: string; children: ReactNode }) {
  return (
    <label htmlFor={htmlFor} className="block text-[13px] leading-[18px] text-secondary">
      {children}
    </label>
  );
}

// 36 x 20 (WF3, WF4): on is a light track with a dark knob at the right; off is a gray track and knob.
export function Switch({ label, checked, onChange, disabled = false }: { label: string; checked: boolean; onChange: (checked: boolean) => void; disabled?: boolean }) {
  return (
    <BaseSwitch.Root
      aria-label={label}
      checked={checked}
      onCheckedChange={(next) => onChange(next)}
      disabled={disabled}
      className="relative inline-flex h-5 w-9 shrink-0 cursor-default items-center rounded-full bg-selected p-0.5 transition-colors duration-150 data-[checked]:bg-user-bubble data-[disabled]:opacity-50"
    >
      <BaseSwitch.Thumb className="size-4 rounded-full bg-secondary transition-transform duration-150 ease-[cubic-bezier(0.23,1,0.32,1)] data-[checked]:translate-x-4 data-[checked]:bg-canvas" />
    </BaseSwitch.Root>
  );
}

// A segmented control (Autonomy, Stop after, Appearance, the model picker's provider): radio buttons on a track.
export function Segmented<T extends string>({ label, value, options, onChange }: { label: string; value: T; options: readonly { value: T; label: string }[]; onChange: (value: T) => void }) {
  return (
    <RadioGroup aria-label={label} value={value} onValueChange={(next) => onChange(next as T)} className="inline-flex h-8 shrink-0 items-center gap-0.5 rounded-full bg-popover p-1">
      {options.map((option) => (
        <Radio.Root
          key={option.value}
          value={option.value}
          className="flex h-6 cursor-default items-center whitespace-nowrap rounded-full px-3 text-[13px] text-secondary hover:text-body data-[checked]:bg-selected data-[checked]:font-semibold data-[checked]:text-text"
        >
          {option.label}
        </Radio.Root>
      ))}
    </RadioGroup>
  );
}

// The 120 x 32 number stepper (Chain limit).
export function Stepper({ label, value, min, max, onChange }: { label: string; value: number; min: number; max: number; onChange: (value: number) => void }) {
  const button = "flex size-6 items-center justify-center rounded-full text-icon hover:bg-pill hover:text-text disabled:opacity-40";
  return (
    <div role="group" aria-label={label} className="flex h-8 w-[120px] shrink-0 items-center justify-between rounded-full bg-popover px-1">
      <button type="button" aria-label="Decrease" disabled={value <= min} onClick={() => onChange(value - 1)} className={button}>
        <Minus className="size-4" aria-hidden />
      </button>
      <span aria-live="polite" className="text-[14px] font-semibold text-text">
        {value}
      </span>
      <button type="button" aria-label="Increase" disabled={value >= max} onClick={() => onChange(value + 1)} className={button}>
        <Plus className="size-4" aria-hidden />
      </button>
    </div>
  );
}
```

Create `packages/ui/components/dialog.tsx`:

```tsx
"use client";

import type { ReactNode, RefObject } from "react";
import { X } from "lucide-react";
import { Dialog as BaseDialog } from "@base-ui/react/dialog";
import { Pill } from "./pill";

type DialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  width?: number;
  actions?: ReactNode; // beside the title, before the close button
  closable?: boolean;
  initialFocus?: RefObject<HTMLElement | null>;
  children: ReactNode;
};

// Dialogs (WF7, WF11): the card fill with a divider stroke, radius 16, padding 20, over a 60% black scrim.
export function Dialog({ open, onOpenChange, title, width = 320, actions, closable = false, initialFocus, children }: DialogProps) {
  return (
    <BaseDialog.Root open={open} onOpenChange={(next) => onOpenChange(next)}>
      <BaseDialog.Portal>
        <BaseDialog.Backdrop className="fixed inset-0 z-40 bg-black/60 transition-opacity duration-200 data-[ending-style]:opacity-0 data-[starting-style]:opacity-0" />
        <BaseDialog.Popup
          initialFocus={initialFocus}
          style={{ width }}
          className="fixed left-1/2 top-1/2 z-50 flex max-h-[calc(100vh-48px)] max-w-[calc(100vw-48px)] -translate-x-1/2 -translate-y-1/2 flex-col overflow-y-auto rounded-[16px] border border-divider bg-card p-5 transition-[opacity,scale] duration-200 ease-[cubic-bezier(0.23,1,0.32,1)] data-[ending-style]:scale-95 data-[ending-style]:opacity-0 data-[starting-style]:scale-95 data-[starting-style]:opacity-0"
        >
          <div className="flex items-start gap-3">
            <BaseDialog.Title className="flex-1 text-[17px] font-semibold leading-[22px] text-title">{title}</BaseDialog.Title>
            {actions}
            {closable && (
              <BaseDialog.Close aria-label="Close" className="-mr-1 -mt-0.5 flex size-7 items-center justify-center rounded-[8px] text-icon hover:bg-pill hover:text-text">
                <X className="size-4" aria-hidden />
              </BaseDialog.Close>
            )}
          </div>
          {children}
        </BaseDialog.Popup>
      </BaseDialog.Portal>
    </BaseDialog.Root>
  );
}

// A yes-or-no question (the Full auto warning, Delete bot), with the buttons right-aligned on one line.
export function ConfirmDialog({ open, onOpenChange, title, body, confirm, danger = false, onConfirm }: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  body: string;
  confirm: ReactNode;
  danger?: boolean;
  onConfirm: () => void;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange} title={title}>
      <p className="mt-2 text-[13px] leading-[18px] text-body">{body}</p>
      <div className="mt-5 flex justify-end gap-2">
        <Pill onClick={() => onOpenChange(false)}>Cancel</Pill>
        <Pill primary={!danger} className={danger ? "text-danger" : undefined} onClick={onConfirm}>
          {confirm}
        </Pill>
      </div>
    </Dialog>
  );
}
```

Create `packages/ui/components/menu.tsx`:

```ts
// Menus (the chat's More menu, the Agents card menu): the popover style from WF11, 36 px rows with radius 8.
export const MENU_POPUP = "flex min-w-[220px] flex-col gap-1 rounded-[16px] border border-divider bg-popover p-1.5";
export const MENU_ITEM = "flex h-9 cursor-default items-center gap-2.5 rounded-[8px] px-3 text-[13px] text-text outline-none data-[highlighted]:bg-highlight [&_svg]:size-4 [&_svg]:text-icon";
```

In `packages/ui/components/chat/chat-header.tsx`:
1. Delete the local `const ITEM = ...` line.
2. Add `import { MENU_ITEM, MENU_POPUP } from "../menu";`.
3. In `MoreMenu`, use `className={MENU_POPUP}` on `Menu.Popup` and `className={MENU_ITEM}` on each `Menu.Item`.
4. Change `function HeaderState` to `export function HeaderState`.

- [ ] **Step 6: Run the tests and checks**

Run: `npx vitest run packages/ui`
Expected: PASS, including the 2 new format tests; plan 1D's format tests still pass.

Run: `npm run typecheck`, `npm run lint` and `npm run build`
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add packages/ui
git commit -m "Add the shared controls, dialogs and formatting the new screens use

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: First-run setup and Settings

**Files:**
- Create: `packages/ui/lib/setup.ts`, `packages/ui/lib/setup.test.ts`, `packages/ui/components/setup/provider-cards.tsx`, `packages/ui/app/settings/page.tsx`
- Replace: `packages/ui/app/page.tsx`
- Modify: `packages/desktop/src/desktop.real.test.ts` (one line)

**Interfaces:**
- Consumes: Task 3's `request`, `Segmented` and `Stepper`; plan 1D's `useApp`, `applyTheme` and `themeChoice`.
- Produces:
  - `cardView(setup, { signingIn, preparing }): CardView`, `planLabel(plan)` and `PROVIDER_NAME` in `lib/setup.ts`.
  - `ProviderCards()`: the Claude Code and Codex cards (WF9). Settings reuses them.
  - `/` shows first-run setup (WF9) while there are no bots, and otherwise opens the first bot's chat, as before.
  - `/settings/` (General: Appearance, then the provider cards) and `/settings/?section=bots` (the chain limit): WF10.

The provider cards (WF9, plus the sandbox state built from the same card style):

| State | What the card says | Actions |
|---|---|---|
| Checking | "Checking" | none |
| Not installed | Claude: "Install Claude Code from claude.com/claude-code, then check again." Codex: "Install Codex with `npm install -g @openai/codex`, then check again." | Check again |
| A failed check | The daemon's reason | Check again |
| Signed out | Claude: "Opens Claude's sign-in in a new window." Codex: "Opens OpenAI's sign-in page in your browser." | Sign in to Claude / Sign in to Codex |
| Signing in | "Finish signing in in the window that opened" (Claude) or "in your browser" (Codex), then "Tjommie notices as soon as you're done." | Open the sign-in window/page again; Cancel |
| Signed in | "Signed in as <account>", then the plan | Use a different account |
| Signed in, Codex's sandbox not set up | Adds "Codex's sandbox isn't set up yet. Codex bots at Own folder need it, and Windows asks for admin permission once." | Set up Codex's sandbox |
| Setting up the sandbox | "Setting up Codex's sandbox. Answer the Windows prompt." | none |
| The sandbox couldn't be checked | "Tjommie couldn't check Codex's sandbox, so Codex bots at Own folder can't run yet." | Check again |

- [ ] **Step 1: Write the failing tests**

Create `packages/ui/lib/setup.test.ts`:

```ts
import type { ProviderSetup } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { cardView, planLabel } from "./setup";

type Claude = Extract<ProviderSetup, { provider: "claude" }>;
type Codex = Extract<ProviderSetup, { provider: "codex" }>;
const claude = (extra: Partial<Claude> = {}): Claude => ({ provider: "claude", installed: true, version: "2.1.280", signedIn: false, account: null, plan: null, error: null, ...extra });
const codex = (extra: Partial<Codex> = {}): Codex => ({ provider: "codex", installed: true, version: "0.157.1", signedIn: false, account: null, plan: null, error: null, sandbox: "notConfigured", ...extra });
const quiet = { signingIn: false, preparing: false };

describe("cardView", () => {
  it("shows checking, not installed, a failed check, signed out and signing in", () => {
    expect(cardView(undefined, quiet)).toEqual({ kind: "checking" });
    expect(cardView(claude({ installed: false, version: null, error: "Claude Code is not installed" }), quiet)).toEqual({ kind: "not_installed" });
    expect(cardView(claude({ error: "Claude sign-in check failed" }), quiet)).toEqual({ kind: "problem", reason: "Claude sign-in check failed" });
    expect(cardView(claude(), quiet)).toEqual({ kind: "signed_out" });
    expect(cardView(claude(), { signingIn: true, preparing: false })).toEqual({ kind: "signing_in" });
    expect(cardView(claude({ signedIn: true }), { signingIn: true, preparing: false })).toEqual({ kind: "signing_in" });
  });

  it("shows the account, and whether Codex's sandbox still needs setting up", () => {
    expect(cardView(claude({ signedIn: true, account: "tyrel.jobs@gmail.com", plan: "max" }), quiet)).toEqual({ kind: "signed_in", account: "tyrel.jobs@gmail.com", plan: "max", sandbox: null });
    expect(cardView(codex({ signedIn: true }), quiet)).toMatchObject({ kind: "signed_in", sandbox: "needed" });
    expect(cardView(codex({ signedIn: true, sandbox: "updateRequired" }), quiet)).toMatchObject({ sandbox: "needed" });
    expect(cardView(codex({ signedIn: true }), { signingIn: false, preparing: true })).toMatchObject({ sandbox: "preparing" });
    expect(cardView(codex({ signedIn: true, sandbox: "unavailable" }), quiet)).toMatchObject({ sandbox: "unavailable" });
    expect(cardView(codex({ signedIn: true, sandbox: "ready" }), quiet)).toMatchObject({ sandbox: "ready" });
    expect(planLabel("max")).toBe("Max plan");
    expect(planLabel(null)).toBeNull();
  });
});
```

- [ ] **Step 2: Run them to see them fail**

Run: `npx vitest run packages/ui/lib/setup.test.ts`
Expected: FAIL. `./setup` can't be found.

- [ ] **Step 3: Write the card logic**

Create `packages/ui/lib/setup.ts`:

```ts
import type { Provider, ProviderSetup } from "@tjommie/protocol";

export const PROVIDER_NAME: Record<Provider, string> = { claude: "Claude Code", codex: "Codex" };

export type SandboxView = "ready" | "needed" | "preparing" | "unavailable";
export type CardView =
  | { kind: "checking" }
  | { kind: "not_installed" }
  | { kind: "signing_in" }
  | { kind: "signed_in"; account: string | null; plan: string | null; sandbox: SandboxView | null }
  | { kind: "problem"; reason: string }
  | { kind: "signed_out" };

// What a provider's setup card shows (WF9). `signingIn` and `preparing` belong to this window: the person clicked
// Sign in or Set up, and the daemon hasn't reported the outcome yet.
export function cardView(setup: ProviderSetup | undefined, local: { signingIn: boolean; preparing: boolean }): CardView {
  if (!setup) return { kind: "checking" };
  if (!setup.installed) return { kind: "not_installed" };
  if (local.signingIn) return { kind: "signing_in" };
  if (setup.signedIn) {
    let sandbox: SandboxView | null = null;
    if (setup.provider === "codex") {
      if (setup.sandbox === "ready") sandbox = "ready";
      else if (local.preparing) sandbox = "preparing";
      else sandbox = setup.sandbox === "unavailable" ? "unavailable" : "needed";
    }
    return { kind: "signed_in", account: setup.account, plan: setup.plan, sandbox };
  }
  if (setup.error) return { kind: "problem", reason: setup.error };
  return { kind: "signed_out" };
}

// "Max plan", from the CLI's plan name.
export const planLabel = (plan: string | null): string | null => (plan ? `${plan.slice(0, 1).toUpperCase()}${plan.slice(1)} plan` : null);
```

- [ ] **Step 4: Write the provider cards**

Create `packages/ui/components/setup/provider-cards.tsx`:

```tsx
"use client";

import { useState, type ReactNode } from "react";
import { CircleCheck, Loader, Sparkles, Terminal } from "lucide-react";
import type { Provider, ProviderSetup } from "@tjommie/protocol";
import { request } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { cardView, planLabel, PROVIDER_NAME, type CardView } from "@/lib/setup";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";
import { Pill } from "../pill";

const PROVIDERS: Provider[] = ["claude", "codex"];

type Handlers = { onSignIn: () => void; onCancel: () => void; onPrepare: () => void; onCheck: () => void };

// Claude Code and Codex side by side (WF9 first run, and Settings: General). Each card says what's missing and
// offers the one thing that fixes it.
export function ProviderCards() {
  const setup = useApp((state) => state.setup);
  const [signing, setSigning] = useState<{ provider: Provider; since: ProviderSetup | undefined } | null>(null);
  const [preparing, setPreparing] = useState(false);
  const [checking, setChecking] = useState(false);
  const [problem, setProblem] = useState<{ provider: Provider; text: string } | null>(null);
  const setupFor = (provider: Provider): ProviderSetup | undefined => setup?.find((entry) => entry.provider === provider);

  async function run(provider: Provider, action: () => Promise<unknown>): Promise<boolean> {
    setProblem(null);
    try {
      await action();
      return true;
    } catch (error) {
      setProblem({ provider, text: errorText(error) });
      return false;
    }
  }

  async function signIn(provider: Provider): Promise<void> {
    const since = setupFor(provider);
    if (await run(provider, () => request("setup.signIn", { provider }))) setSigning({ provider, since });
  }

  async function prepare(): Promise<void> {
    setPreparing(true);
    await run("codex", () => request("setup.prepareCodexSandbox", {}));
    setPreparing(false);
  }

  // setup.status runs both CLIs' checks, so only this button asks for it.
  async function checkAgain(provider: Provider): Promise<void> {
    setChecking(true);
    await run(provider, () => request("setup.status", {}));
    setChecking(false);
  }

  return (
    <div className="grid grid-cols-[repeat(auto-fit,minmax(300px,1fr))] gap-5">
      {PROVIDERS.map((provider) => {
        const entry = setupFor(provider);
        // Signing in lasts until the daemon reports this provider again: it sends setup.changed when sign-in ends.
        const signingIn = signing?.provider === provider && entry === signing.since;
        const view = cardView(entry, { signingIn, preparing: provider === "codex" && preparing });
        const { body, actions } = content(view, provider === "claude", checking, {
          onSignIn: () => void signIn(provider),
          onCancel: () => setSigning(null),
          onPrepare: () => void prepare(),
          onCheck: () => void checkAgain(provider),
        });
        const Icon = provider === "claude" ? Sparkles : Terminal;
        return (
          <section key={provider} aria-label={PROVIDER_NAME[provider]} className="flex min-h-[200px] flex-col rounded-[14px] border border-card-stroke bg-card p-5">
            <div className="flex items-center gap-3">
              <span className="flex size-9 shrink-0 items-center justify-center rounded-[10px] bg-pill text-icon">
                <Icon className="size-[18px]" aria-hidden />
              </span>
              <div>
                <h2 className="text-[15px] font-semibold leading-[20px] text-title">{PROVIDER_NAME[provider]}</h2>
                <p className="text-[12px] leading-[18px] text-secondary">{entry?.version ? `Version ${entry.version}` : view.kind === "not_installed" ? "Not installed" : "\u00a0"}</p>
              </div>
            </div>
            <div className="mt-5 flex-1 text-[14px] leading-[21px] text-text">{body}</div>
            {actions && <div className="mt-4 flex flex-wrap items-center gap-3">{actions}</div>}
            {problem?.provider === provider && (
              <p role="alert" className="mt-2 text-[12px] leading-[18px] text-danger">
                {problem.text}
              </p>
            )}
          </section>
        );
      })}
    </div>
  );
}

function Line({ icon, className, children }: { icon: ReactNode; className?: string; children: ReactNode }) {
  return (
    <p className={cn("flex items-center gap-2", className)}>
      {icon}
      <span>{children}</span>
    </p>
  );
}

const spinner = <Loader className="size-4 shrink-0 animate-spin text-icon" aria-hidden />;
const helper = "text-[13px] leading-[18px] text-secondary";

function content(view: CardView, claude: boolean, busy: boolean, on: Handlers): { body: ReactNode; actions: ReactNode } {
  const checkAgain = (
    <Pill disabled={busy} onClick={on.onCheck}>
      Check again
    </Pill>
  );
  switch (view.kind) {
    case "checking":
      return { body: <Line icon={spinner}>Checking</Line>, actions: null };
    case "not_installed":
      return {
        body: claude ? (
          <p className={helper}>
            Install Claude Code from{" "}
            <a href="https://claude.com/claude-code" target="_blank" rel="noreferrer" className="text-accent hover:underline">
              claude.com/claude-code
            </a>
            , then check again.
          </p>
        ) : (
          <p className={helper}>
            Install Codex with <code className="rounded-[6px] bg-code px-1 py-0.5 text-[12px]">npm install -g @openai/codex</code>, then check again.
          </p>
        ),
        actions: checkAgain,
      };
    case "problem":
      return { body: <p className={helper}>{view.reason}</p>, actions: checkAgain };
    case "signed_out":
      return {
        body: <p className={helper}>{claude ? "Opens Claude's sign-in in a new window." : "Opens OpenAI's sign-in page in your browser."}</p>,
        actions: (
          <Pill primary onClick={on.onSignIn}>
            {claude ? "Sign in to Claude" : "Sign in to Codex"}
          </Pill>
        ),
      };
    case "signing_in":
      return {
        body: (
          <>
            <Line icon={spinner}>{claude ? "Finish signing in in the window that opened" : "Finish signing in in your browser"}</Line>
            <p className="ml-6 text-[12px] leading-[18px] text-secondary">Tjommie notices as soon as you&apos;re done.</p>
          </>
        ),
        actions: (
          <>
            <Pill onClick={on.onSignIn}>{claude ? "Open the sign-in window again" : "Open the sign-in page again"}</Pill>
            <button type="button" onClick={on.onCancel} className="text-[14px] text-secondary hover:text-text">
              Cancel
            </button>
          </>
        ),
      };
    case "signed_in": {
      const plan = planLabel(view.plan);
      return {
        body: (
          <>
            <Line icon={<CircleCheck className="size-4 shrink-0 text-working" aria-hidden />}>{view.account ? `Signed in as ${view.account}` : "Signed in"}</Line>
            {plan && <p className="ml-6 text-[12px] leading-[18px] text-secondary">{plan}</p>}
            {view.sandbox === "needed" && (
              <p className={cn(helper, "mt-3")}>Codex&apos;s sandbox isn&apos;t set up yet. Codex bots at Own folder need it, and Windows asks for admin permission once.</p>
            )}
            {view.sandbox === "preparing" && (
              <Line icon={spinner} className="mt-3">
                Setting up Codex&apos;s sandbox. Answer the Windows prompt.
              </Line>
            )}
            {view.sandbox === "unavailable" && <p className={cn(helper, "mt-3")}>Tjommie couldn&apos;t check Codex&apos;s sandbox, so Codex bots at Own folder can&apos;t run yet.</p>}
          </>
        ),
        actions: (
          <>
            {view.sandbox === "needed" && (
              <Pill primary onClick={on.onPrepare}>
                Set up Codex&apos;s sandbox
              </Pill>
            )}
            {view.sandbox === "unavailable" && checkAgain}
            <button type="button" onClick={on.onSignIn} className="text-[14px] text-accent hover:underline">
              Use a different account
            </button>
          </>
        ),
      };
    }
  }
}
```

- [ ] **Step 5: Write first-run setup and Settings**

Replace `packages/ui/app/page.tsx` with:

```tsx
"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { useEffect } from "react";
import { ProviderCards } from "@/components/setup/provider-cards";
import { useApp } from "@/lib/store";

// Opens the first bot's chat. With no bots yet, it's first-run setup (WF9).
export default function Home() {
  const router = useRouter();
  const first = useApp((state) => state.bots[0]?.id);
  useEffect(() => {
    if (first) router.replace(`/chat/?bot=${encodeURIComponent(first)}`);
  }, [first, router]);
  if (first) return null;
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="drag h-11 shrink-0" />
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-[808px] px-6 pb-12 pt-10">
          <h1 className="text-[20px] font-semibold leading-[26px] text-title">Set up Tjommie</h1>
          <p className="mt-2 text-[14px] leading-[21px] text-secondary">
            Tjommie runs your bots with Claude Code and Codex on this PC. Sign in to each one once. Tjommie keeps its own sign-in, separate from how you use these tools yourself, and never sees your password.
          </p>
          <div className="mt-8">
            <ProviderCards />
          </div>
          <Link href="/bots/new/" className="press mt-10 inline-flex h-10 items-center rounded-full bg-user-bubble px-5 text-[14px] font-semibold text-user-text">
            Create your first bot
          </Link>
          <p className="mt-3 text-[12px] leading-[18px] text-secondary">You can sign in to the other one later in Settings.</p>
        </div>
      </div>
    </div>
  );
}
```

Create `packages/ui/app/settings/page.tsx`:

```tsx
"use client";

import Link from "next/link";
import { Suspense, useState, type ReactNode } from "react";
import { useSearchParams } from "next/navigation";
import { Bot, SlidersHorizontal, type LucideIcon } from "lucide-react";
import { Segmented, Stepper } from "@/components/controls";
import { ProviderCards } from "@/components/setup/provider-cards";
import { request } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { useApp } from "@/lib/store";
import { applyTheme, themeChoice, type ThemeChoice } from "@/lib/theme";
import { cn } from "@/lib/utils";

const THEMES = [
  { value: "dark", label: "Dark" },
  { value: "light", label: "Light" },
  { value: "system", label: "System" },
] as const satisfies readonly { value: ThemeChoice; label: string }[];

// Settings (WF10): General and Bots.
function SettingsScreen() {
  const section = useSearchParams().get("section") === "bots" ? "bots" : "general";
  return (
    <div className="flex min-h-0 flex-1">
      <nav aria-label="Settings" className="flex w-[220px] shrink-0 flex-col border-r border-card-stroke bg-drawer px-3">
        <div className="drag h-11 shrink-0" />
        <h1 className="mb-3 mt-1 px-3 text-[15px] font-semibold leading-[20px] text-title">Settings</h1>
        <div className="flex flex-col gap-1">
          <NavLink href="/settings/" icon={SlidersHorizontal} selected={section === "general"}>
            General
          </NavLink>
          <NavLink href="/settings/?section=bots" icon={Bot} selected={section === "bots"}>
            Bots
          </NavLink>
        </div>
      </nav>
      <div className="flex min-w-0 flex-1 flex-col">
        <div className="drag h-11 shrink-0" />
        <div className="min-h-0 flex-1 overflow-y-auto px-6 pb-10 pt-1">{section === "general" ? <General /> : <BotsSection />}</div>
      </div>
    </div>
  );
}

function NavLink({ href, icon: Icon, selected, children }: { href: string; icon: LucideIcon; selected: boolean; children: ReactNode }) {
  return (
    <Link
      href={href}
      aria-current={selected ? "page" : undefined}
      className={cn("flex h-9 items-center gap-2.5 rounded-[8px] px-3 text-[14px]", selected ? "bg-nav-selected font-medium text-text" : "text-body hover:bg-pill")}
    >
      <Icon className={cn("size-4", selected ? "text-text" : "text-secondary")} aria-hidden />
      {children}
    </Link>
  );
}

function General() {
  // This page renders only in the window, never in the static build, so localStorage is there.
  const [choice, setChoice] = useState<ThemeChoice>(() => themeChoice());
  return (
    <>
      <h2 className="text-[20px] font-semibold leading-[26px] text-title">General</h2>
      <section className="mt-6 flex max-w-[700px] items-center justify-between gap-4 rounded-[12px] bg-card p-4">
        <span className="text-[14px] text-text">Appearance</span>
        <Segmented
          label="Appearance"
          value={choice}
          options={THEMES}
          onChange={(next) => {
            applyTheme(next);
            setChoice(next);
          }}
        />
      </section>
      <h3 className="mt-8 text-[13px] font-semibold leading-[18px] text-label">Claude Code and Codex</h3>
      <div className="mt-3 max-w-[760px]">
        <ProviderCards />
      </div>
    </>
  );
}

function BotsSection() {
  const settings = useApp((state) => state.settings);
  const [problem, setProblem] = useState<string | null>(null);
  async function setChainLimit(chainLimit: number): Promise<void> {
    try {
      await request("settings.set", { chainLimit });
      setProblem(null);
    } catch (error) {
      setProblem(errorText(error));
    }
  }
  return (
    <>
      <h2 className="text-[20px] font-semibold leading-[26px] text-title">Bots</h2>
      <h3 className="mt-8 text-[13px] font-semibold leading-[18px] text-label">Bots asking each other</h3>
      <section className="mt-3 flex max-w-[700px] items-start justify-between gap-6 rounded-[12px] bg-card p-4">
        <div>
          <p className="text-[14px] text-text">Requests between bots per chain</p>
          <p className="mt-1 text-[12px] leading-[18px] text-secondary">
            A chain starts with each message you send and each routine run. When a chain reaches this many bot-to-bot requests, the next request stops and the bot asks you instead.
          </p>
          {problem && (
            <p role="alert" className="mt-1 text-[12px] leading-[18px] text-danger">
              {problem}
            </p>
          )}
        </div>
        {settings && <Stepper label="Requests between bots per chain" value={settings.chainLimit} min={1} max={20} onChange={(next) => void setChainLimit(next)} />}
      </section>
    </>
  );
}

export default function SettingsPage() {
  return (
    <Suspense>
      <SettingsScreen />
    </Suspense>
  );
}
```

In `packages/desktop/src/desktop.real.test.ts`, the home page no longer says "No bots yet.". Replace the `beforeAll` line `await page.getByText("No bots yet.").waitFor({ timeout: 120_000 });` with:

```ts
  await page.getByRole("heading", { name: "Set up Tjommie" }).waitFor({ timeout: 120_000 });
```

- [ ] **Step 6: Run the tests and checks**

Run: `npx vitest run packages/ui`
Expected: PASS, including the 2 setup tests.

Run: `npm run typecheck`, `npm run lint` and `npm run build`
Expected: all pass. `packages/ui/out/settings/index.html` exists.

- [ ] **Step 7: Check it by hand**

Use development mode (plan 1D, Task 4 Step 4) with an empty data root and the signed-in accounts. Set these in all three PowerShell windows before starting:

```powershell
$env:TJOMMIE_HOME = "$env:TEMP\tjommie-first-run"
$env:TJOMMIE_ACCOUNTS_DIR = "$env:LOCALAPPDATA\Tjommie\accounts"
```

Then check against `docs/wireframes/wf9-first-run-dark.png` and the two `wf10-settings-*` exports:
1. The window opens on "Set up Tjommie":
   - Both cards say "Checking", then "Signed in as ..." with the plan.
   - The rail shows only the dimmed Team room, the divider and the bottom cluster.
2. If Codex's sandbox isn't set up on this PC, the Codex card offers "Set up Codex's sandbox". Don't click it here: it changes Windows.
3. Settings:
   - General shows Appearance. Light recolors the app and the caption buttons, and Dark brings it back.
   - General also lists the same two cards.
   - Bots shows the chain limit at 5. Increase makes it 6, and a reload (Ctrl+R) keeps 6.

When done, stop the three windows and delete `$env:TEMP\tjommie-first-run`.

- [ ] **Step 8: Commit**

```bash
git add packages/ui packages/desktop/src/desktop.real.test.ts
git commit -m "Add first-run setup and Settings

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: The bot form's logic and the Suggestions

**Files:**
- Create: `packages/ui/lib/bot-form.ts`, `packages/ui/lib/bot-form.test.ts`, `packages/ui/lib/suggestions.ts`, `packages/ui/lib/suggestions.test.ts`

**Interfaces:**
- Consumes: Task 3's `daysLabel` and `timesLabel`; `BotInput` and `RoutineInput` from `@tjommie/protocol`.
- Produces, in `lib/bot-form.ts`:
  - Types:
    - `BotForm = { name, color, shape, instructions, provider, model, autonomy, projectFolder, browser, canMessage }`;
    - `RoutineDraft = { name, prompt, days, times, timeLimitMinutes, enabled }`.
  - Form state:
    - `NEW_BOT` (a blue circle, Claude Sonnet, Own folder, browser off);
    - `formFromBot(bot)`;
    - `createInput(form)` (the name trimmed);
    - `patchFor(bot, form)`: only the changed fields, with provider and model together.
  - Checks and summaries:
    - `nameProblem(name, bots, selfId)`;
    - `routinesSummary(routines)` ("Weekdays 8:48 AM, 12:47 PM, 4:45 PM");
    - `canMessageSummary(ids, bots)` ("2 bots");
    - `AUTONOMY_LEVELS`, and `autonomyHelp(autonomy, provider)` (the helper line under Autonomy).
- Produces, in `lib/suggestions.ts`: `SUGGESTIONS`, three `Suggestion` entries of `{ title, description, form, routines }` (WF3: Night Shift, Inbox Triage, Job hunter).

The Autonomy helper line depends on the provider, because "Own folder" protects Codex bots differently (spec §5.4):

| Level | Helper |
|---|---|
| Always ask | "Always ask: asks before every change it makes." |
| Own folder, Claude | "Own folder: works freely inside its own folder and asks before anything else. Claude bots also ask before each shell command." (WF3) |
| Own folder, Codex | "Own folder: works freely inside its own folder and asks before anything else. It can still read the rest of your user profile, and Codex bots can read each other's folders." |
| Full auto | "Full auto: never asks. It can run any command and change any file your Windows account can." |

- [ ] **Step 1: Write the failing tests**

Create `packages/ui/lib/bot-form.test.ts`:

```ts
import type { Bot } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { autonomyHelp, canMessageSummary, createInput, formFromBot, nameProblem, NEW_BOT, patchFor, routinesSummary } from "./bot-form";

const bot: Bot = {
  id: "b1",
  name: "Pnet apply",
  color: "orange",
  shape: "cloud",
  instructions: "Apply to Full Stack roles.",
  provider: "claude",
  model: "sonnet",
  autonomy: "folder",
  homeFolder: "C:\\bots\\b1",
  projectFolder: null,
  browser: true,
  paused: false,
  canMessage: ["b2", "b3"],
  state: "idle",
  setupReason: null,
  limitedUntil: null,
  createdAt: "2026-09-21T08:00:00.000Z",
};

describe("the bot form", () => {
  it("sends only what the person changed", () => {
    const form = formFromBot(bot);
    expect(patchFor(bot, form)).toEqual({});
    expect(patchFor(bot, { ...form, name: " Job scout ", canMessage: ["b3", "b2"] })).toEqual({ name: "Job scout" });
    expect(patchFor(bot, { ...form, model: "haiku" })).toEqual({ provider: "claude", model: "haiku" });
    expect(patchFor(bot, { ...form, provider: "codex", model: "gpt-6-astra", canMessage: ["b2"] })).toEqual({ provider: "codex", model: "gpt-6-astra", canMessage: ["b2"] });
    expect(patchFor(bot, { ...form, projectFolder: "C:\\work", browser: false })).toEqual({ projectFolder: "C:\\work", browser: false });
  });

  it("checks the name and trims it for the daemon", () => {
    const bots = [bot, { ...bot, id: "b2", name: "CV Writer" }];
    expect(nameProblem("  ", bots, null)).toBe("Give the bot a name.");
    expect(nameProblem("x".repeat(41), bots, null)).toBe("Keep the name to 40 characters or fewer.");
    expect(nameProblem("cv writer", bots, null)).toBe("There's already a bot called cv writer.");
    expect(nameProblem("Pnet apply", bots, "b1")).toBeNull();
    expect(createInput({ ...NEW_BOT, name: " Night Shift " }).name).toBe("Night Shift");
  });

  it("sums up routines and Can message for their rows", () => {
    const weekdays = [1, 2, 3, 4, 5];
    expect(routinesSummary([])).toBe("None yet");
    expect(routinesSummary([{ days: weekdays, times: ["16:45"], enabled: true }, { days: weekdays, times: ["08:48", "12:47"], enabled: true }])).toBe("Weekdays 8:48 AM, 12:47 PM, 4:45 PM");
    expect(routinesSummary([{ days: weekdays, times: ["08:48"], enabled: true }, { days: [0], times: ["09:00"], enabled: true }])).toBe("2 routines");
    expect(routinesSummary([{ days: weekdays, times: ["08:48"], enabled: false }])).toBe("1 paused");
    expect(canMessageSummary(["b2", "gone"], [bot, { ...bot, id: "b2" }])).toBe("1 bot");
    expect(canMessageSummary([], [bot])).toBe("Nobody yet");
  });

  it("says what Own folder protects for each CLI", () => {
    expect(autonomyHelp("folder", "claude")).toContain("Claude bots also ask before each shell command.");
    expect(autonomyHelp("folder", "codex")).toContain("It can still read the rest of your user profile");
    expect(autonomyHelp("full", "codex")).toMatch(/^Full auto: never asks\./);
  });
});
```

Create `packages/ui/lib/suggestions.test.ts`:

```ts
import { BotInput, RoutineInput } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { NEW_BOT } from "./bot-form";
import { SUGGESTIONS } from "./suggestions";

describe("Suggestions", () => {
  it("every suggestion is a valid bot with valid routines", () => {
    expect(SUGGESTIONS.map((suggestion) => suggestion.title)).toEqual(["Night Shift", "Inbox Triage", "Job hunter"]);
    for (const suggestion of SUGGESTIONS) {
      expect(suggestion.form.name).toBe(suggestion.title);
      expect(() => BotInput.parse({ ...NEW_BOT, ...suggestion.form })).not.toThrow();
      expect(suggestion.routines.length).toBeGreaterThan(0);
      for (const routine of suggestion.routines) expect(() => RoutineInput.parse({ botId: "b1", ...routine })).not.toThrow();
    }
  });
});
```

- [ ] **Step 2: Run them to see them fail**

Run: `npx vitest run packages/ui/lib/bot-form.test.ts packages/ui/lib/suggestions.test.ts`
Expected: FAIL. `./bot-form` and `./suggestions` can't be found.

- [ ] **Step 3: Write the form logic**

Create `packages/ui/lib/bot-form.ts`:

```ts
import type { Autonomy, Bot, BotColor, BotPatch, BotShape, ParamsIn, Provider } from "@tjommie/protocol";
import { daysLabel, timesLabel } from "./format";

export type BotForm = {
  name: string;
  color: BotColor;
  shape: BotShape;
  instructions: string;
  provider: Provider;
  model: string;
  autonomy: Autonomy;
  projectFolder: string | null;
  browser: boolean;
  canMessage: string[];
};

// A routine on a form: one the New bot screen creates with the bot, or the one the routine editor holds.
export type RoutineDraft = { name: string; prompt: string; days: number[]; times: string[]; timeLimitMinutes: number | null; enabled: boolean };

export const NEW_BOT: BotForm = {
  name: "",
  color: "blue",
  shape: "circle",
  instructions: "",
  provider: "claude",
  model: "sonnet",
  autonomy: "folder",
  projectFolder: null,
  browser: false,
  canMessage: [],
};

export const formFromBot = (bot: Bot): BotForm => ({
  name: bot.name,
  color: bot.color,
  shape: bot.shape,
  instructions: bot.instructions,
  provider: bot.provider,
  model: bot.model,
  autonomy: bot.autonomy,
  projectFolder: bot.projectFolder,
  browser: bot.browser,
  canMessage: bot.canMessage,
});

// Why the name can't be saved yet, or null. The daemon checks again (name_taken).
export function nameProblem(name: string, bots: Pick<Bot, "id" | "name">[], selfId: string | null): string | null {
  const trimmed = name.trim();
  if (!trimmed) return "Give the bot a name.";
  if (trimmed.length > 40) return "Keep the name to 40 characters or fewer.";
  if (bots.some((bot) => bot.id !== selfId && bot.name.toLowerCase() === trimmed.toLowerCase())) return `There's already a bot called ${trimmed}.`;
  return null;
}

export const createInput = (form: BotForm): ParamsIn<"bots.create"> => ({ ...form, name: form.name.trim() });

const sameSet = (a: string[], b: string[]): boolean => a.length === b.length && a.every((id) => b.includes(id));

// Only what the person changed, so a save never puts back a value that changed elsewhere since the form opened.
// Provider and model travel together: the daemon needs a model whenever the provider changes.
export function patchFor(bot: Bot, form: BotForm): BotPatch {
  const patch: BotPatch = {};
  const name = form.name.trim();
  if (name !== bot.name) patch.name = name;
  if (form.color !== bot.color) patch.color = form.color;
  if (form.shape !== bot.shape) patch.shape = form.shape;
  if (form.instructions !== bot.instructions) patch.instructions = form.instructions;
  if (form.provider !== bot.provider || form.model !== bot.model) {
    patch.provider = form.provider;
    patch.model = form.model;
  }
  if (form.autonomy !== bot.autonomy) patch.autonomy = form.autonomy;
  if (form.projectFolder !== bot.projectFolder) patch.projectFolder = form.projectFolder;
  if (form.browser !== bot.browser) patch.browser = form.browser;
  if (!sameSet(form.canMessage, bot.canMessage)) patch.canMessage = form.canMessage;
  return patch;
}

// The Routines row's value: "None yet", "Weekdays 8:48 AM, 12:47 PM", "2 routines" or "1 paused".
export function routinesSummary(routines: Pick<RoutineDraft, "days" | "times" | "enabled">[]): string {
  if (routines.length === 0) return "None yet";
  const on = routines.filter((routine) => routine.enabled);
  if (on.length === 0) return `${routines.length} paused`;
  const days = daysLabel(on[0].days);
  if (on.every((routine) => daysLabel(routine.days) === days)) return `${days} ${timesLabel(on.flatMap((routine) => routine.times))}`;
  return `${on.length} routines`;
}

// The Can message row's value. Deleted bots don't count.
export function canMessageSummary(ids: string[], bots: Pick<Bot, "id">[]): string {
  const count = ids.filter((id) => bots.some((bot) => bot.id === id)).length;
  return count === 0 ? "Nobody yet" : `${count} ${count === 1 ? "bot" : "bots"}`;
}

export const AUTONOMY_LEVELS = [
  { value: "ask", label: "Always ask" },
  { value: "folder", label: "Own folder" },
  { value: "full", label: "Full auto" },
] as const satisfies readonly { value: Autonomy; label: string }[];

// The line under Autonomy. What "Own folder" doesn't protect is said plainly (spec 5.4).
export function autonomyHelp(autonomy: Autonomy, provider: Provider): string {
  if (autonomy === "ask") return "Always ask: asks before every change it makes.";
  if (autonomy === "full") return "Full auto: never asks. It can run any command and change any file your Windows account can.";
  return provider === "claude"
    ? "Own folder: works freely inside its own folder and asks before anything else. Claude bots also ask before each shell command."
    : "Own folder: works freely inside its own folder and asks before anything else. It can still read the rest of your user profile, and Codex bots can read each other's folders.";
}
```

- [ ] **Step 4: Write the Suggestions**

Create `packages/ui/lib/suggestions.ts`:

```ts
import type { BotForm, RoutineDraft } from "./bot-form";

export type Suggestion = {
  title: string;
  description: string;
  form: Pick<BotForm, "name" | "color" | "shape" | "instructions" | "browser">;
  routines: RoutineDraft[];
};

const WEEKDAYS = [1, 2, 3, 4, 5];

// The New bot screen's templates (spec section 7, WF3). Picking one fills in the form; nothing is created until
// Create bot, which then creates the routines too.
export const SUGGESTIONS: Suggestion[] = [
  {
    title: "Night Shift",
    description: "Works overnight and preps your morning digest",
    form: {
      name: "Night Shift",
      color: "amber",
      shape: "hexagon",
      browser: false,
      instructions:
        "Each weekday morning, write a short digest in digest.md in your home folder: what's new in your inbox folder, what you finished yesterday, and anything I asked you to keep an eye on. Keep it to one screen, and tell me when it's ready.",
    },
    routines: [
      {
        name: "Morning digest",
        prompt: "Write today's morning digest in digest.md, then tell me it's ready in one line.",
        days: WEEKDAYS,
        times: ["06:30"],
        timeLimitMinutes: 60,
        enabled: true,
      },
    ],
  },
  {
    title: "Inbox Triage",
    description: "Sorts your email and drafts replies in your voice",
    form: {
      name: "Inbox Triage",
      color: "pink",
      shape: "cloud",
      browser: true,
      instructions:
        "Use your browser to check my email; I sign in to it there once. Sort new mail into what needs me, what can wait and what's noise. Draft replies in my voice, but never send anything without asking me first.",
    },
    routines: [
      {
        name: "Inbox sweep",
        prompt: "Check my inbox for new mail, sort it, and draft replies for anything that needs one. Don't send anything.",
        days: WEEKDAYS,
        times: ["08:00", "12:00", "16:00"],
        timeLimitMinutes: 60,
        enabled: true,
      },
    ],
  },
  {
    title: "Job hunter",
    description: "Applies to roles that match your CV every weekday",
    form: {
      name: "Job hunter",
      color: "orange",
      shape: "cloud",
      browser: true,
      instructions:
        "Find jobs that match my CV (I'll put it in your inbox folder) on the job sites I sign in to in your browser. Apply only to roles that fit, skip senior roles unless I say otherwise, and keep a list of what you applied to in applications.md.",
    },
    routines: [
      {
        name: "Job search",
        prompt: "Search for new roles that match my CV and apply to the ones that fit. Add each application to applications.md.",
        days: WEEKDAYS,
        times: ["09:00"],
        timeLimitMinutes: 180,
        enabled: true,
      },
    ],
  },
];
```

- [ ] **Step 5: Run the tests and checks**

Run: `npx vitest run packages/ui`
Expected: PASS, including the 4 bot form tests and the Suggestions test.

Run: `npm run typecheck` and `npm run lint`
Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add packages/ui/lib
git commit -m "Add the bot form's logic and the three Suggestions

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: The routine editor, and routines in the bot panel

**Files:**
- Create: `packages/ui/lib/routine-form.ts`, `packages/ui/lib/routine-form.test.ts`, `packages/ui/lib/routines.ts`, `packages/ui/components/routines/routine-editor.tsx`, `packages/ui/components/routines/routine-rows.tsx`, `packages/ui/components/panel-bar.tsx`
- Replace: `packages/ui/components/chat/bot-panel.tsx`

**Interfaces:**
- Consumes: Tasks 3 and 5; plan 1D's `BotPanel`, `recentActivity` and `IconButton`.
- Produces:
  - **`lib/routine-form.ts`** (pure):
    - `EMPTY_ROUTINE`, `draftFrom(routine)`, `WEEK` (Monday first), `toggleDay(days, day)` and `addTime(times, time)`;
    - `routineProblem(draft)`, `stopAfterOptions(limit)`, `localZone()` and `zoneLine(timeZone)`;
    - `runLine(run, now?)`, which returns `{ icon, when, text, length }`.
  - **`lib/routines.ts`:** `saveRoutine(botId, routine | null, draft)` creates or updates; `removeRoutine(routine)`.
  - **`RoutineEditor({ title, initial, timeZone, runsFor?, onClose, onSave, onDelete? })`:** the WF7 dialog. It closes itself once `onSave` or `onDelete` succeeds.
  - **`RoutineRows({ routines, onOpen })`:** the routine list (a clock and the schedule, or a pause icon and "Paused").
  - **`PanelBar({ settingsHref, settingsLabel, hideLabel, onHide })`:** the right panel's top bar.
  - **The bot panel:** its Routines header gains Add, and its rows open the editor.

- [ ] **Step 1: Write the failing tests**

Create `packages/ui/lib/routine-form.test.ts`:

```ts
import type { RoutineRun } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { addTime, EMPTY_ROUTINE, routineProblem, runLine, stopAfterOptions, toggleDay, zoneLine } from "./routine-form";

const now = new Date(2026, 8, 27, 14, 0); // Sunday
const at = (day: number, hour: number, minute: number) => new Date(2026, 8, day, hour, minute).toISOString();
const run = (extra: Partial<RoutineRun>): RoutineRun => ({
  id: "run1",
  botId: "b1",
  routineId: "r1",
  dueAt: at(27, 8, 48),
  startedAt: null,
  endedAt: null,
  status: "done",
  note: null,
  ...extra,
});

describe("the routine editor", () => {
  it("edits days and times", () => {
    expect(toggleDay([1, 3], 2)).toEqual([1, 2, 3]);
    expect(toggleDay([1, 2, 3], 2)).toEqual([1, 3]);
    expect(addTime(["12:47"], "08:48")).toEqual(["08:48", "12:47"]);
    expect(addTime(["08:48"], "08:48")).toEqual(["08:48"]);
    expect(addTime(["08:48"], "24:00")).toEqual(["08:48"]);
  });

  it("says what's missing before saving, and offers a proposal's own time limit", () => {
    expect(routineProblem(EMPTY_ROUTINE)).toBe("Give the routine a name.");
    expect(routineProblem({ ...EMPTY_ROUTINE, name: "Check" })).toBe("Say what the bot should do.");
    expect(routineProblem({ ...EMPTY_ROUTINE, name: "Check", prompt: "Check", days: [] })).toBe("Pick at least one day.");
    expect(routineProblem({ ...EMPTY_ROUTINE, name: "Check", prompt: "Check", times: [] })).toBe("Add at least one time.");
    expect(routineProblem({ ...EMPTY_ROUTINE, name: "Check", prompt: "Check" })).toBeNull();
    expect(stopAfterOptions(180).map((option) => option.label)).toEqual(["No limit", "1 hour", "2 hours", "3 hours"]);
    expect(stopAfterOptions(90).at(-1)).toEqual({ value: "90", label: "1h 30m" });
    expect(zoneLine("Africa/Johannesburg")).toBe("Times are in South Africa Standard Time (Africa/Johannesburg).");
  });

  it("builds the Recent runs rows", () => {
    expect(runLine(run({ startedAt: at(27, 8, 48), endedAt: at(27, 11, 39) }), now)).toEqual({ icon: "done", when: "Today 8:48 AM", text: "Done", length: "2h 51m" });
    expect(runLine(run({ status: "stopped", note: "Stopped at the time limit.", startedAt: at(27, 8, 48), endedAt: at(27, 11, 48) }), now)).toEqual({
      icon: "stopped",
      when: "Today 8:48 AM",
      text: "Stopped at the time limit",
      length: "3h",
    });
    expect(runLine(run({ status: "failed", note: "Claude stopped unexpectedly" }), now)).toMatchObject({ icon: "failed", text: "Failed: Claude stopped unexpectedly", length: null });
    expect(runLine(run({ status: "missed", dueAt: at(25, 8, 48) }), now)).toEqual({ icon: "missed", when: "Friday 8:48 AM", text: "Missed: the PC was off", length: null });
    expect(runLine(run({ status: "skipped", note: "Pnet apply was paused." }), now)).toMatchObject({ icon: "skipped", text: "Skipped: Pnet apply was paused" });
  });
});
```

- [ ] **Step 2: Run them to see them fail**

Run: `npx vitest run packages/ui/lib/routine-form.test.ts`
Expected: FAIL. `./routine-form` can't be found.

- [ ] **Step 3: Write the routine logic**

Create `packages/ui/lib/routine-form.ts`:

```ts
import type { Routine, RoutineRun } from "@tjommie/protocol";
import type { RoutineDraft } from "./bot-form";
import { clock, dayName, duration } from "./format";

export const EMPTY_ROUTINE: RoutineDraft = { name: "", prompt: "", days: [1, 2, 3, 4, 5], times: ["09:00"], timeLimitMinutes: null, enabled: true };

export const draftFrom = (routine: Routine): RoutineDraft => ({
  name: routine.name,
  prompt: routine.prompt,
  days: routine.days,
  times: routine.times,
  timeLimitMinutes: routine.timeLimitMinutes,
  enabled: routine.enabled,
});

// The week as the editor shows it, Monday first. Days are 0 (Sunday) to 6.
export const WEEK = [
  { day: 1, letter: "M", name: "Monday" },
  { day: 2, letter: "T", name: "Tuesday" },
  { day: 3, letter: "W", name: "Wednesday" },
  { day: 4, letter: "T", name: "Thursday" },
  { day: 5, letter: "F", name: "Friday" },
  { day: 6, letter: "S", name: "Saturday" },
  { day: 0, letter: "S", name: "Sunday" },
] as const;

export const toggleDay = (days: number[], day: number): number[] => (days.includes(day) ? days.filter((each) => each !== day) : [...days, day].sort((a, b) => a - b));

// Adds an "HH:MM" time once, in order. Anything else leaves the times as they were.
export const addTime = (times: string[], time: string): string[] => (/^([01]\d|2[0-3]):[0-5]\d$/.test(time) && !times.includes(time) ? [...times, time].sort() : times);

// Why the routine can't be saved yet, or null. The daemon checks again.
export function routineProblem(draft: RoutineDraft): string | null {
  if (!draft.name.trim()) return "Give the routine a name.";
  if (draft.name.trim().length > 60) return "Keep the name to 60 characters or fewer.";
  if (!draft.prompt.trim()) return "Say what the bot should do.";
  if (draft.days.length === 0) return "Pick at least one day.";
  if (draft.times.length === 0) return "Add at least one time.";
  return null;
}

// Stop after (WF7). A limit set another way, like a bot's proposal, gets an option of its own.
export function stopAfterOptions(limit: number | null): { value: string; label: string }[] {
  const options = [
    { value: "none", label: "No limit" },
    { value: "60", label: "1 hour" },
    { value: "120", label: "2 hours" },
    { value: "180", label: "3 hours" },
  ];
  if (limit !== null && !options.some((option) => option.value === String(limit))) options.push({ value: String(limit), label: duration(limit * 60_000) });
  return options;
}

// The time zone new routines get: the daemon runs on this PC, so it's this PC's zone too.
export const localZone = (): string => Intl.DateTimeFormat().resolvedOptions().timeZone;

// "Times are in South Africa Standard Time (Africa/Johannesburg)."
export function zoneLine(timeZone: string): string {
  const name = new Intl.DateTimeFormat("en-US", { timeZone, timeZoneName: "long" }).formatToParts(new Date()).find((part) => part.type === "timeZoneName")?.value;
  return name && name !== timeZone ? `Times are in ${name} (${timeZone}).` : `Times are in ${timeZone}.`;
}

export type RunLine = { icon: "done" | "stopped" | "failed" | "missed" | "skipped" | "running"; when: string; text: string; length: string | null };

// One row of Recent runs (WF7): when it was due, what happened, and how long it ran.
export function runLine(run: RoutineRun, now: Date = new Date()): RunLine {
  const when = `${dayName(run.dueAt, now)} ${clock(run.dueAt)}`;
  const length = run.startedAt && run.endedAt ? duration(Date.parse(run.endedAt) - Date.parse(run.startedAt)) : null;
  const note = run.note ? run.note.replace(/\.$/, "") : null;
  switch (run.status) {
    case "done":
      return { icon: "done", when, text: "Done", length };
    case "stopped":
      return { icon: "stopped", when, text: note ?? "Stopped", length };
    case "failed":
      return { icon: "failed", when, text: note ? `Failed: ${note}` : "Failed", length };
    case "missed":
      return { icon: "missed", when, text: "Missed: the PC was off", length: null };
    case "skipped":
      return { icon: "skipped", when, text: note ? `Skipped: ${note}` : "Skipped", length: null };
    case "queued":
      return { icon: "running", when, text: "Waiting to start", length: null };
    case "running":
      return { icon: "running", when, text: "Running", length: null };
  }
}
```

Create `packages/ui/lib/routines.ts`:

```ts
import type { Routine } from "@tjommie/protocol";
import type { RoutineDraft } from "./bot-form";
import { request } from "./connection";

const cleaned = (draft: RoutineDraft): RoutineDraft => ({ ...draft, name: draft.name.trim(), prompt: draft.prompt.trim() });

// Creates a routine for the bot, or saves the changes to one it already has. New routines get this PC's zone.
export async function saveRoutine(botId: string, routine: Routine | null, draft: RoutineDraft): Promise<void> {
  if (routine) await request("routines.update", { id: routine.id, patch: cleaned(draft) });
  else await request("routines.create", { botId, ...cleaned(draft) });
}

export async function removeRoutine(routine: Routine): Promise<void> {
  await request("routines.delete", { id: routine.id });
}
```

- [ ] **Step 4: Write the editor, the rows and the panel bar**

Create `packages/ui/components/routines/routine-editor.tsx`:

```tsx
"use client";

import { useEffect, useRef, useState } from "react";
import { Check, CircleAlert, CircleDashed, Globe, Loader, Pause, Plus, TimerOff, Trash2, X, type LucideIcon } from "lucide-react";
import type { RoutineRun } from "@tjommie/protocol";
import type { RoutineDraft } from "@/lib/bot-form";
import { request } from "@/lib/connection";
import { timesLabel } from "@/lib/format";
import { errorText } from "@/lib/labels";
import { addTime, routineProblem, runLine, stopAfterOptions, toggleDay, WEEK, zoneLine, type RunLine } from "@/lib/routine-form";
import { cn } from "@/lib/utils";
import { FIELD, Label, Segmented, Switch } from "../controls";
import { Dialog } from "../dialog";
import { Pill } from "../pill";

const RUN_ICONS: Record<RunLine["icon"], LucideIcon> = { done: Check, stopped: TimerOff, failed: CircleAlert, missed: CircleDashed, skipped: Pause, running: Loader };

type Props = {
  title: string;
  initial: RoutineDraft;
  timeZone: string;
  runsFor?: string; // a saved routine's id: shows its Recent runs
  onClose: () => void;
  onSave: (draft: RoutineDraft) => Promise<void>;
  onDelete?: () => Promise<void>;
};

// The routine editor (WF7): a 560 px dialog. It stays open until Save or Delete works, and says why when they don't.
export function RoutineEditor({ title, initial, timeZone, runsFor, onClose, onSave, onDelete }: Props) {
  const [draft, setDraft] = useState(initial);
  const [adding, setAdding] = useState(false);
  const [runs, setRuns] = useState<RoutineRun[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const nameField = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!runsFor) return;
    request("routines.runs", { routineId: runsFor }).then(setRuns, () => setRuns([]));
  }, [runsFor]);

  async function act(action: () => Promise<void>): Promise<void> {
    setBusy(true);
    setProblem(null);
    try {
      await action();
      onClose();
    } catch (error) {
      setProblem(errorText(error));
      setBusy(false);
    }
  }

  function save(): void {
    const missing = routineProblem(draft);
    if (missing) setProblem(missing);
    else void act(() => onSave(draft));
  }

  const limit = draft.timeLimitMinutes === null ? "none" : String(draft.timeLimitMinutes);
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
      title={title}
      width={560}
      closable
      initialFocus={nameField}
      actions={
        <span className="flex items-center gap-2 text-[13px] text-secondary">
          On
          <Switch label="Routine on" checked={draft.enabled} onChange={(enabled) => setDraft({ ...draft, enabled })} />
        </span>
      }
    >
      <div className="mt-5 flex flex-col gap-2">
        <Label htmlFor="routine-name">Name</Label>
        <input id="routine-name" ref={nameField} value={draft.name} maxLength={60} onChange={(event) => setDraft({ ...draft, name: event.target.value })} className={cn(FIELD, "h-11")} />
      </div>
      <div className="mt-4 flex flex-col gap-2">
        <Label htmlFor="routine-prompt">What to do</Label>
        <textarea
          id="routine-prompt"
          value={draft.prompt}
          maxLength={10_000}
          onChange={(event) => setDraft({ ...draft, prompt: event.target.value })}
          className={cn(FIELD, "max-h-[200px] min-h-[96px] resize-none py-3 [field-sizing:content]")}
        />
      </div>
      <p className="mt-4 text-[13px] leading-[18px] text-secondary">Days</p>
      <div role="group" aria-label="Days" className="mt-2 flex gap-2">
        {WEEK.map(({ day, letter, name }) => {
          const on = draft.days.includes(day);
          return (
            <button
              key={day}
              type="button"
              aria-label={name}
              aria-pressed={on}
              onClick={() => setDraft({ ...draft, days: toggleDay(draft.days, day) })}
              className={cn("flex size-8 items-center justify-center rounded-full text-[13px]", on ? "bg-user-bubble font-semibold text-user-text" : "bg-pill text-secondary")}
            >
              {letter}
            </button>
          );
        })}
      </div>
      <p className="mt-4 text-[13px] leading-[18px] text-secondary">Times</p>
      <div className="mt-2 flex flex-wrap items-center gap-2">
        {draft.times.map((time) => (
          <span key={time} className="inline-flex h-8 items-center gap-1.5 rounded-full bg-pill pl-3 pr-1.5 text-[13px] text-text">
            {timesLabel([time])}
            <button
              type="button"
              aria-label={`Remove ${timesLabel([time])}`}
              onClick={() => setDraft({ ...draft, times: draft.times.filter((each) => each !== time) })}
              className="flex size-5 items-center justify-center rounded-full text-icon hover:text-text"
            >
              <X className="size-3" aria-hidden />
            </button>
          </span>
        ))}
        {adding ? (
          <input
            type="time"
            autoFocus
            aria-label="New time"
            onChange={(event) => {
              if (!event.target.value) return;
              setDraft({ ...draft, times: addTime(draft.times, event.target.value) });
              setAdding(false);
            }}
            onBlur={() => setAdding(false)}
            className="h-8 rounded-full border border-divider bg-transparent px-3 text-[13px] text-text"
          />
        ) : (
          <button type="button" onClick={() => setAdding(true)} className="inline-flex h-8 items-center gap-1.5 rounded-full border border-divider px-3 text-[13px] text-text hover:bg-pill">
            <Plus className="size-3.5" aria-hidden />
            Add time
          </button>
        )}
      </div>
      <p className="mt-3 flex items-center gap-1.5 text-[12px] leading-[18px] text-secondary">
        <Globe className="size-3.5 shrink-0" aria-hidden />
        {zoneLine(timeZone)}
      </p>
      <p className="mt-4 text-[13px] leading-[18px] text-secondary">Stop after</p>
      <div className="mt-2">
        <Segmented
          label="Stop after"
          value={limit}
          options={stopAfterOptions(draft.timeLimitMinutes)}
          onChange={(value) => setDraft({ ...draft, timeLimitMinutes: value === "none" ? null : Number(value) })}
        />
      </div>
      {runsFor && (
        <div className="mt-5 border-t border-divider pt-4">
          <div className="flex items-baseline justify-between">
            <h3 className="text-[13px] font-semibold leading-[18px] text-label">Recent runs</h3>
            <span className="text-[12px] leading-[18px] text-caption">Last 20 runs are kept</span>
          </div>
          {runs && runs.length === 0 && <p className="mt-2 text-[13px] leading-[18px] text-secondary">No runs yet.</p>}
          {runs && runs.length > 0 && (
            <ul className="mt-2 flex flex-col">
              {runs.map((run) => {
                const line = runLine(run);
                const Icon = RUN_ICONS[line.icon];
                return (
                  <li key={run.id} className="flex h-[26px] items-center gap-3 text-[13px]">
                    <Icon className={cn("size-3.5 shrink-0", line.icon === "done" ? "text-working" : "text-icon", line.icon === "running" && "animate-spin")} aria-hidden />
                    <span className="w-[150px] shrink-0 text-text">{line.when}</span>
                    <span className="min-w-0 flex-1 truncate text-body">{line.text}</span>
                    {line.length && <span className="shrink-0 text-[12px] text-caption">{line.length}</span>}
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      )}
      <div className="mt-5 flex items-center gap-3 border-t border-divider pt-4">
        {onDelete && (
          <button type="button" disabled={busy} onClick={() => void act(onDelete)} className="flex shrink-0 items-center gap-1.5 text-[14px] text-danger disabled:opacity-50">
            <Trash2 className="size-4" aria-hidden />
            Delete routine
          </button>
        )}
        {problem && (
          <p role="alert" className="min-w-0 flex-1 text-right text-[12px] leading-[18px] text-danger">
            {problem}
          </p>
        )}
        <span className="ml-auto flex shrink-0 gap-2">
          <Pill onClick={onClose}>Cancel</Pill>
          <Pill primary disabled={busy} onClick={save}>
            Save
          </Pill>
        </span>
      </div>
    </Dialog>
  );
}
```

Create `packages/ui/components/routines/routine-rows.tsx`:

```tsx
import { Clock, Pause } from "lucide-react";
import { routineTitle, schedule } from "@/lib/format";

export type RoutineRow = { key: string; name: string; timeLimitMinutes: number | null; days: number[]; times: string[]; enabled: boolean };

// A bot's routines (the WF1 panel, and the Routines row on the bot forms). Each row opens the routine editor.
export function RoutineRows({ routines, onOpen }: { routines: RoutineRow[]; onOpen: (key: string) => void }) {
  if (routines.length === 0) return <p className="text-[13px] leading-[18px] text-secondary">No routines yet.</p>;
  return (
    <ul className="-mx-2 flex flex-col gap-1">
      {routines.map((routine) => (
        <li key={routine.key}>
          <button type="button" onClick={() => onOpen(routine.key)} className="flex w-full items-start gap-2.5 rounded-[10px] px-2 py-1.5 text-left hover:bg-highlight">
            {routine.enabled ? <Clock className="mt-0.5 size-4 shrink-0 text-working" aria-hidden /> : <Pause className="mt-0.5 size-4 shrink-0 text-secondary" aria-hidden />}
            <span className="min-w-0">
              <span className="block truncate text-[13px] leading-[18px] text-title">{routineTitle(routine.name, routine.timeLimitMinutes)}</span>
              <span className="block text-[12px] leading-[18px] text-secondary">{routine.enabled ? schedule(routine.days, routine.times) : "Paused"}</span>
            </span>
          </button>
        </li>
      ))}
    </ul>
  );
}
```

Create `packages/ui/components/panel-bar.tsx`:

```tsx
import Link from "next/link";
import { ChevronsRight, Settings } from "lucide-react";
import { IconButton } from "./icon-button";

// The right panel's top bar: settings and hide at the left. Windows draws its caption buttons over the right end.
export function PanelBar({ settingsHref, settingsLabel, hideLabel, onHide }: { settingsHref: string; settingsLabel: string; hideLabel: string; onHide: () => void }) {
  return (
    <div className="drag flex h-11 shrink-0 items-center gap-1 px-3">
      <Link href={settingsHref} aria-label={settingsLabel} title={settingsLabel} className="flex size-8 items-center justify-center rounded-[10px] text-icon hover:bg-pill hover:text-text">
        <Settings className="size-4" aria-hidden />
      </Link>
      <IconButton label={hideLabel} onClick={onHide}>
        <ChevronsRight aria-hidden />
      </IconButton>
    </div>
  );
}
```

- [ ] **Step 5: Give the bot panel its routines**

Replace `packages/ui/components/chat/bot-panel.tsx` with the version below. Summary, Browser and Recent activity are plan 1D's, unchanged. The top bar is now `PanelBar`, and the routines come from `PanelRoutines`.

```tsx
"use client";

import { useMemo, useState, type ReactNode } from "react";
import { Check, Folder, Loader, Shield, Sparkles, X, type LucideIcon } from "lucide-react";
import type { Bot, Message, Routine } from "@tjommie/protocol";
import { recentActivity } from "@/lib/activity";
import { request } from "@/lib/connection";
import { desktop } from "@/lib/desktop";
import { clock } from "@/lib/format";
import { AUTONOMY_LINE, errorText, modelLabel, stateLabel } from "@/lib/labels";
import { draftFrom, EMPTY_ROUTINE, localZone } from "@/lib/routine-form";
import { removeRoutine, saveRoutine } from "@/lib/routines";
import { useApp } from "@/lib/store";
import { BotAvatar } from "../bot-avatar";
import { PanelBar } from "../panel-bar";
import { RoutineEditor } from "../routines/routine-editor";
import { RoutineRows } from "../routines/routine-rows";

// The right panel beside a bot's chat (WF1 Phase 1). 16 px between cards, 24 px above a section label.
export function BotPanel({ bot, messages, onHide }: { bot: Bot; messages: Message[]; onHide: () => void }) {
  const allRoutines = useApp((state) => state.routines);
  const routines = useMemo(() => allRoutines.filter((routine) => routine.botId === bot.id), [allRoutines, bot.id]);
  const steps = useMemo(() => recentActivity(messages), [messages]);
  return (
    <aside aria-label={`About ${bot.name}`} className="flex w-[460px] shrink-0 flex-col border-l border-divider bg-canvas">
      <PanelBar settingsHref={`/bots/settings/?bot=${encodeURIComponent(bot.id)}`} settingsLabel="Bot settings" hideLabel="Hide the bot panel" onHide={onHide} />
      <div className="flex min-h-0 flex-1 flex-col overflow-y-auto px-4 pb-4">
        <Summary bot={bot} />
        {bot.browser && <BrowserCard bot={bot} />}
        <PanelRoutines bot={bot} routines={routines} />
        <SectionLabel>Recent activity</SectionLabel>
        {steps.length === 0 ? (
          <p className="text-[13px] leading-[18px] text-secondary">No recent activity.</p>
        ) : (
          <ul className="flex flex-col gap-2">
            {steps.map((step) => (
              <li key={step.key} className="flex items-center gap-2.5 text-[13px] leading-[18px]">
                {step.status === "running" ? (
                  <Loader className="size-3.5 shrink-0 animate-spin text-icon" aria-label="Running" />
                ) : step.status === "done" ? (
                  <Check className="size-3.5 shrink-0 text-icon" aria-label="Done" />
                ) : (
                  <X className="size-3.5 shrink-0 text-secondary" aria-label="Failed" />
                )}
                <span className="min-w-0 flex-1 truncate text-body">{step.summary}</span>
                <span className="shrink-0 text-[12px] text-caption">{step.status === "running" ? "now" : clock(step.at)}</span>
              </li>
            ))}
          </ul>
        )}
      </div>
    </aside>
  );
}

// Routines with Add (WF1 Phase 1). Rows and Add open the routine editor; saves go straight to the daemon.
function PanelRoutines({ bot, routines }: { bot: Bot; routines: Routine[] }) {
  const [editing, setEditing] = useState<Routine | "new" | null>(null);
  const routine = editing === "new" ? null : editing;
  return (
    <>
      <div className="mb-2 mt-6 flex items-center justify-between">
        <h3 className="text-[13px] font-semibold leading-[18px] text-label">Routines</h3>
        <button type="button" onClick={() => setEditing("new")} className="text-[13px] leading-[18px] text-accent hover:underline">
          Add
        </button>
      </div>
      <RoutineRows routines={routines.map((each) => ({ ...each, key: each.id }))} onOpen={(key) => setEditing(routines.find((each) => each.id === key) ?? null)} />
      {editing && (
        <RoutineEditor
          key={routine?.id ?? "new"}
          title={routine ? "Edit routine" : "New routine"}
          initial={routine ? draftFrom(routine) : EMPTY_ROUTINE}
          timeZone={routine?.timeZone ?? localZone()}
          runsFor={routine?.id}
          onClose={() => setEditing(null)}
          onSave={(draft) => saveRoutine(bot.id, routine, draft)}
          onDelete={routine ? () => removeRoutine(routine) : undefined}
        />
      )}
    </>
  );
}

function SectionLabel({ children }: { children: ReactNode }) {
  return <h3 className="mb-2 mt-6 text-[13px] font-semibold leading-[18px] text-label">{children}</h3>;
}

function Row({ icon: Icon, children, action }: { icon: LucideIcon; children: ReactNode; action?: ReactNode }) {
  return (
    <div className="flex items-start gap-2">
      <Icon className="mt-0.5 size-3.5 shrink-0 text-icon" aria-hidden />
      <span className="min-w-0 flex-1">{children}</span>
      {action}
    </div>
  );
}

function OpenFolder({ path }: { path: string }) {
  return (
    <button type="button" onClick={() => void desktop()?.openFolder(path)} className="shrink-0 text-accent hover:underline">
      Open
    </button>
  );
}

function Summary({ bot }: { bot: Bot }) {
  return (
    <section className="mt-3 rounded-[12px] bg-card p-4">
      <div className="flex items-center gap-3">
        <BotAvatar color={bot.color} shape={bot.shape} size={40} className="shrink-0" />
        <h2 className="min-w-0 flex-1 truncate text-[17px] font-semibold text-title">{bot.name}</h2>
        {bot.state !== "idle" && (
          <span className="flex shrink-0 items-center gap-1.5 rounded-full bg-pill px-2.5 py-1 text-[12px] font-medium text-text">
            {bot.state === "working" && <Loader className="size-3 animate-spin" aria-hidden />}
            {stateLabel(bot)}
          </span>
        )}
      </div>
      <div className="mt-3 flex flex-col gap-2 pl-[52px] text-[13px] leading-[18px] text-body">
        <Row icon={Sparkles}>{modelLabel(bot)}</Row>
        <Row icon={Shield}>{AUTONOMY_LINE[bot.autonomy]}</Row>
        <Row icon={Folder} action={<OpenFolder path={bot.homeFolder} />}>
          Home folder
        </Row>
        {bot.projectFolder && (
          <Row icon={Folder} action={<OpenFolder path={bot.projectFolder} />}>
            <span className="block truncate">Project folder: {bot.projectFolder}</span>
          </Row>
        )}
      </div>
    </section>
  );
}

function BrowserCard({ bot }: { bot: Bot }) {
  const [problem, setProblem] = useState<string | null>(null);
  async function show(): Promise<void> {
    setProblem(null);
    try {
      await request("bots.showBrowser", { id: bot.id });
    } catch (error) {
      setProblem(errorText(error));
    }
  }
  return (
    <section className="mt-4 flex gap-3 rounded-[12px] bg-card p-3">
      <ChromeLogo />
      <div className="min-w-0 flex-1">
        <div className="flex items-center justify-between gap-3">
          <h3 className="text-[13px] font-semibold leading-[18px] text-title">Browser</h3>
          <button type="button" onClick={() => void show()} className="shrink-0 text-[13px] leading-[18px] text-accent hover:underline">
            Show window
          </button>
        </div>
        <p className="mt-0.5 text-[12px] leading-[18px] text-secondary">Its own Chrome window, separate from yours. Sites you sign in to there stay signed in.</p>
        {problem && (
          <p role="alert" className="text-[12px] leading-[18px] text-danger">
            {problem}
          </p>
        )}
      </div>
    </section>
  );
}

// The Chrome logo in its brand colors: an approved exception to the token palette.
function ChromeLogo() {
  return (
    <svg viewBox="0 0 48 48" width={20} height={20} aria-hidden="true" className="mt-0.5 shrink-0">
      <path d="M24 24 6.68 14A20 20 0 0 1 41.32 14Z" fill="#E33B2E" />
      <path d="M24 24 41.32 14A20 20 0 0 1 24 44Z" fill="#FBC116" />
      <path d="M24 24 24 44A20 20 0 0 1 6.68 14Z" fill="#229342" />
      <circle cx="24" cy="24" r="9.5" fill="#FFFFFF" />
      <circle cx="24" cy="24" r="7.5" fill="#1A73E8" />
    </svg>
  );
}
```

- [ ] **Step 6: Run the tests and checks**

Run: `npx vitest run packages/ui`
Expected: PASS, including the 3 routine editor tests.

Run: `npm run typecheck`, `npm run lint` and `npm run build`
Expected: all pass.

- [ ] **Step 7: Check it by hand**

In development mode with a Claude bot (plan 1D, Task 12 Step 5), check against `docs/wireframes/wf7-routine-editor-dark.png`:
1. In the bot panel, Add opens "New routine" over the dimmed window, with the name field focused.
2. Name "Morning check", What to do "Check the news", days Monday to Friday. Add time 08:48 with the time picker, then Save. The panel lists "Morning check" with "Weekdays at 8:48 AM, 9:00 AM".
3. The row opens "Edit routine". Remove the 9:00 AM chip, set Stop after to 1 hour, then Save. The row reads "Morning check (1h)".
4. Clear the name and press Save: "Give the routine a name." shows and the dialog stays open. Cancel closes it without saving.
5. On switched off makes the row show a pause icon and "Paused". Delete routine removes it.

- [ ] **Step 8: Commit**

```bash
git add packages/ui
git commit -m "Add the routine editor and let the bot panel add and edit routines

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: The bot form

**Files:**
- Create: `packages/ui/components/bot-form/identity.tsx`, `packages/ui/components/bot-form/rows.tsx`, `packages/ui/components/bot-form/model-row.tsx`, `packages/ui/components/bot-form/bot-form.tsx`

**Interfaces:**
- Consumes:
  - Task 2's `pickFolder` bridge call;
  - Task 3's controls, dialogs and `request`;
  - Task 5's `BotForm`, `AUTONOMY_LEVELS`, `autonomyHelp` and `canMessageSummary`;
  - plan 1D's `BotAvatar`, `BOT_COLORS`, `modelLabel` and `fileName`.
- Produces:
  - `BotFormFields({ form, onChange, bots, selfId, nameError, routinesRow })`: the form's left column, shared by New bot and Bot settings.
  - From `rows.tsx`:
    - `Row({ icon, title, value?, open, onToggle, children })`, a row that opens in place;
    - `RowHeader`, `ROW_BUTTON` and `RoutinesRow({ summary, children })`.

The rows (WF3 Phase 1):

| Row | Value at the right | Opens to |
|---|---|---|
| Model | "Claude · Sonnet (latest)" | The model picker popover (WF11): Claude or Codex, then that provider's models, with a check on the current one and a caption |
| Folder | "Its own folder" or the project folder's name | Its own folder or A project folder; the project path with Change |
| Autonomy | the level | Always ask, Own folder, Full auto, and the helper line (open to start with). Full auto asks first (the WF11 warning) |
| Browser | "Its own Chrome profile" and a switch | (nothing) |
| Can message | avatars and "2 bots" | Every other bot, with a switch each |
| Routines | the Task 5 summary | Whatever the page passes: drafts on New bot, saved routines on Bot settings |

- [ ] **Step 1: Write the name, avatar and pickers**

Create `packages/ui/components/bot-form/identity.tsx`:

```tsx
"use client";

import { BotColor, BotShape } from "@tjommie/protocol";
import type { BotForm } from "@/lib/bot-form";
import { cn } from "@/lib/utils";
import { BOT_COLORS, BotAvatar } from "../bot-avatar";
import { FIELD, Label } from "../controls";

// The avatar preview, the name, and the color and shape pickers (WF3, WF8). The shapes preview in the bot's color.
export function Identity({ form, onChange, nameError }: { form: BotForm; onChange: (form: BotForm) => void; nameError: string | null }) {
  return (
    <div className="flex gap-6">
      <div className="flex size-[156px] shrink-0 items-center justify-center rounded-[14px] bg-card">
        <BotAvatar color={form.color} shape={form.shape} size={96} />
      </div>
      <div className="min-w-0 flex-1">
        <Label htmlFor="bot-name">Name</Label>
        <input
          id="bot-name"
          value={form.name}
          maxLength={40}
          aria-invalid={nameError ? true : undefined}
          onChange={(event) => onChange({ ...form, name: event.target.value })}
          className={cn(FIELD, "mt-2 h-11")}
        />
        {nameError && (
          <p role="alert" className="mt-1 text-[12px] leading-[18px] text-danger">
            {nameError}
          </p>
        )}
        <div role="radiogroup" aria-label="Color" className="mt-4 flex justify-between gap-2">
          {BotColor.options.map((color) => (
            <button
              key={color}
              type="button"
              role="radio"
              aria-checked={form.color === color}
              aria-label={color}
              onClick={() => onChange({ ...form, color })}
              style={{ backgroundColor: BOT_COLORS[color] }}
              className={cn("size-7 shrink-0 rounded-full", form.color === color && "ring-2 ring-text ring-offset-[3px] ring-offset-canvas")}
            />
          ))}
        </div>
        <div role="radiogroup" aria-label="Shape" className="mt-4 flex justify-between gap-2">
          {BotShape.options.map((shape) => (
            <button
              key={shape}
              type="button"
              role="radio"
              aria-checked={form.shape === shape}
              aria-label={shape.replace("-", " ")}
              onClick={() => onChange({ ...form, shape })}
              className={cn("flex size-8 shrink-0 items-center justify-center rounded-[10px] bg-popover", form.shape === shape && "bg-selected ring-1 ring-inset ring-accent")}
            >
              <BotAvatar color={form.color} shape={shape} size={20} />
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
```

- [ ] **Step 2: Write the rows**

Create `packages/ui/components/bot-form/rows.tsx`:

```tsx
"use client";

import { useState, type ReactNode } from "react";
import { ChevronDown, ChevronRight, Clock, Folder, Globe, MessageCircle, Shield, type LucideIcon } from "lucide-react";
import type { Autonomy, Bot, Provider } from "@tjommie/protocol";
import { fileName } from "@/lib/attachments";
import { AUTONOMY_LEVELS, autonomyHelp, canMessageSummary } from "@/lib/bot-form";
import { desktop } from "@/lib/desktop";
import { BotAvatar } from "../bot-avatar";
import { Segmented, Switch } from "../controls";
import { ConfirmDialog } from "../dialog";

// A settings row (WF3): 56 px on the card fill, radius 12, an icon, the title, and the value at the right.
export const ROW_BUTTON = "flex h-14 w-full items-center gap-3 rounded-[12px] px-4 text-left";

export function RowHeader({ icon: Icon, title, value, chevron }: { icon: LucideIcon; title: string; value?: ReactNode; chevron?: "right" | "down" }) {
  return (
    <>
      <Icon className="size-[18px] shrink-0 text-icon" aria-hidden />
      <span className="flex-1 whitespace-nowrap text-[14px] text-text">{title}</span>
      {value !== undefined && <span className="flex min-w-0 max-w-[60%] items-center gap-1.5 overflow-hidden whitespace-nowrap text-[13px] text-secondary">{value}</span>}
      {chevron === "right" && <ChevronRight className="size-4 shrink-0 text-icon" aria-hidden />}
      {chevron === "down" && <ChevronDown className="size-4 shrink-0 text-icon" aria-hidden />}
    </>
  );
}

// A row that opens in place to show its controls.
export function Row({ icon, title, value, open, onToggle, children }: { icon: LucideIcon; title: string; value?: ReactNode; open: boolean; onToggle: () => void; children: ReactNode }) {
  return (
    <section className="rounded-[12px] bg-card">
      <button type="button" aria-expanded={open} onClick={onToggle} className={ROW_BUTTON}>
        <RowHeader icon={icon} title={title} value={value} chevron={open ? "down" : "right"} />
      </button>
      {open && <div className="px-4 pb-4 pl-[46px]">{children}</div>}
    </section>
  );
}

export function FolderRow({ projectFolder, onChange }: { projectFolder: string | null; onChange: (folder: string | null) => void }) {
  const [open, setOpen] = useState(false);
  async function choose(): Promise<void> {
    const folder = await desktop()?.pickFolder();
    if (folder) onChange(folder);
  }
  return (
    <Row icon={Folder} title="Folder" value={<span className="truncate">{projectFolder ? fileName(projectFolder) : "Its own folder"}</span>} open={open} onToggle={() => setOpen(!open)}>
      <Segmented
        label="Folder"
        value={projectFolder ? "project" : "own"}
        options={[
          { value: "own", label: "Its own folder" },
          { value: "project", label: "A project folder" },
        ]}
        onChange={(value) => (value === "own" ? onChange(null) : void choose())}
      />
      {projectFolder && (
        <p className="mt-3 flex items-center gap-2 text-[12px] leading-[18px] text-secondary">
          <span className="min-w-0 truncate">{projectFolder}</span>
          <button type="button" onClick={() => void choose()} className="shrink-0 text-accent hover:underline">
            Change
          </button>
        </p>
      )}
      <p className="mt-2 text-[12px] leading-[18px] text-secondary">
        {projectFolder ? "It works in the project folder, and keeps its notes and inbox in its own folder." : "It works in its own folder, which Tjommie makes for it."}
      </p>
    </Row>
  );
}

// Full auto asks first (WF11), because it lets the bot do anything the person's Windows account can.
export function AutonomyRow({ autonomy, provider, botName, onChange }: { autonomy: Autonomy; provider: Provider; botName: string; onChange: (autonomy: Autonomy) => void }) {
  const [open, setOpen] = useState(true);
  const [confirming, setConfirming] = useState(false);
  const name = botName.trim() || "this bot";
  return (
    <Row icon={Shield} title="Autonomy" value={open ? undefined : AUTONOMY_LEVELS.find((level) => level.value === autonomy)?.label} open={open} onToggle={() => setOpen(!open)}>
      <Segmented label="Autonomy" value={autonomy} options={AUTONOMY_LEVELS} onChange={(next) => (next === "full" ? setConfirming(true) : onChange(next))} />
      <p className="mt-3 text-[12px] leading-[18px] text-secondary">{autonomyHelp(autonomy, provider)}</p>
      <ConfirmDialog
        open={confirming}
        onOpenChange={setConfirming}
        title={`Let ${name} work without asking?`}
        body={`In Full auto, ${name} can run any command and change any file your Windows account can, without checking with you first. Only choose it for bots you trust with that.`}
        confirm="Use Full auto"
        onConfirm={() => {
          onChange("full");
          setConfirming(false);
        }}
      />
    </Row>
  );
}

export function BrowserRow({ browser, onChange }: { browser: boolean; onChange: (browser: boolean) => void }) {
  return (
    <section className="flex h-14 items-center gap-3 rounded-[12px] bg-card px-4">
      <RowHeader icon={Globe} title="Browser" value="Its own Chrome profile" />
      <Switch label="Browser" checked={browser} onChange={onChange} />
    </section>
  );
}

export function CanMessageRow({ selfId, canMessage, bots, onChange }: { selfId: string | null; canMessage: string[]; bots: Bot[]; onChange: (ids: string[]) => void }) {
  const [open, setOpen] = useState(false);
  const others = bots.filter((bot) => bot.id !== selfId);
  const chosen = others.filter((bot) => canMessage.includes(bot.id));
  return (
    <Row
      icon={MessageCircle}
      title="Can message"
      value={
        <>
          {chosen.slice(0, 3).map((bot) => (
            <BotAvatar key={bot.id} color={bot.color} shape={bot.shape} size={18} className="shrink-0" />
          ))}
          {canMessageSummary(canMessage, others)}
        </>
      }
      open={open}
      onToggle={() => setOpen(!open)}
    >
      {others.length === 0 ? (
        <p className="text-[13px] leading-[18px] text-secondary">No other bots yet.</p>
      ) : (
        <ul className="flex flex-col">
          {others.map((bot) => (
            <li key={bot.id} className="flex h-10 items-center gap-3">
              <BotAvatar color={bot.color} shape={bot.shape} size={24} className="shrink-0" />
              <span className="min-w-0 flex-1 truncate text-[14px] text-text">{bot.name}</span>
              <Switch
                label={`Can message ${bot.name}`}
                checked={canMessage.includes(bot.id)}
                onChange={(on) => onChange(on ? [...canMessage, bot.id] : canMessage.filter((id) => id !== bot.id))}
              />
            </li>
          ))}
        </ul>
      )}
      <p className="mt-2 text-[12px] leading-[18px] text-secondary">It can ask these bots for help. They can&apos;t ask it back unless you allow that in their own settings.</p>
    </Row>
  );
}

export function RoutinesRow({ summary, children }: { summary: string; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  return (
    <Row icon={Clock} title="Routines" value={<span className="truncate">{summary}</span>} open={open} onToggle={() => setOpen(!open)}>
      {children}
    </Row>
  );
}
```

- [ ] **Step 3: Write the model picker**

Create `packages/ui/components/bot-form/model-row.tsx`:

```tsx
"use client";

import { useEffect, useState } from "react";
import { Check, Sparkles } from "lucide-react";
import { Popover } from "@base-ui/react/popover";
import type { ModelInfo, Provider } from "@tjommie/protocol";
import { request } from "@/lib/connection";
import { errorText, modelLabel } from "@/lib/labels";
import { cn } from "@/lib/utils";
import { Segmented } from "../controls";
import { ROW_BUTTON, RowHeader } from "./rows";

const PROVIDERS = [
  { value: "claude", label: "Claude" },
  { value: "codex", label: "Codex" },
] as const satisfies readonly { value: Provider; label: string }[];

// The Model row and its picker (WF11): Claude or Codex, that provider's models, and a caption.
export function ModelRow({ provider, model, onChange }: { provider: Provider; model: string; onChange: (value: { provider: Provider; model: string }) => void }) {
  const [open, setOpen] = useState(false);
  const [tab, setTab] = useState<Provider>(provider);
  const [lists, setLists] = useState<Partial<Record<Provider, ModelInfo[]>>>({});
  const [problem, setProblem] = useState<string | null>(null);

  // Codex's list comes from its app-server, so each list is fetched once, when the picker first shows it.
  useEffect(() => {
    if (!open || lists[tab] || problem) return;
    request("models.list", { provider: tab }).then(
      (models) => setLists((current) => ({ ...current, [tab]: models })),
      (error: unknown) => setProblem(errorText(error)),
    );
  }, [open, tab, lists, problem]);

  const models = lists[tab];
  return (
    <Popover.Root
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        setProblem(null);
        if (next) setTab(provider);
      }}
    >
      <Popover.Trigger className={cn(ROW_BUTTON, "bg-card")}>
        <RowHeader icon={Sparkles} title="Model" value={<span className="truncate">{modelLabel({ provider, model })}</span>} chevron="right" />
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Positioner side="bottom" align="end" sideOffset={6}>
          <Popover.Popup className="w-[320px] rounded-[16px] border border-divider bg-popover p-2">
            <Segmented
              label="Provider"
              value={tab}
              options={PROVIDERS}
              onChange={(next) => {
                setTab(next);
                setProblem(null);
              }}
            />
            <div className="mt-2 flex flex-col gap-1">
              {problem ? (
                <p className="px-3 py-2 text-[13px] leading-[18px] text-danger">{problem}</p>
              ) : !models ? (
                <p className="px-3 py-2 text-[13px] leading-[18px] text-secondary">Loading the models</p>
              ) : (
                models.map((info) => {
                  const selected = provider === tab && model === info.id;
                  return (
                    <button
                      key={info.id}
                      type="button"
                      aria-pressed={selected}
                      onClick={() => {
                        onChange({ provider: tab, model: info.id });
                        setOpen(false);
                      }}
                      className={cn("flex h-9 items-center justify-between rounded-[8px] px-3 text-left text-[14px] text-text hover:bg-highlight", selected && "bg-highlight")}
                    >
                      {tab === "claude" ? info.label.replace(/^Claude /, "") : info.label}
                      {selected && <Check className="size-4" aria-hidden />}
                    </button>
                  );
                })
              )}
            </div>
            <p className="px-3 pb-1 pt-2 text-[12px] leading-[18px] text-caption">{tab === "claude" ? "Latest versions are picked automatically." : "Models from Codex's own list."}</p>
          </Popover.Popup>
        </Popover.Positioner>
      </Popover.Portal>
    </Popover.Root>
  );
}
```

- [ ] **Step 4: Put the form together**

Create `packages/ui/components/bot-form/bot-form.tsx`:

```tsx
"use client";

import type { ReactNode } from "react";
import type { Bot } from "@tjommie/protocol";
import type { BotForm } from "@/lib/bot-form";
import { cn } from "@/lib/utils";
import { FIELD, Label } from "../controls";
import { Identity } from "./identity";
import { ModelRow } from "./model-row";
import { AutonomyRow, BrowserRow, CanMessageRow, FolderRow } from "./rows";

// New bot and Bot settings share this column (WF3, WF8): the avatar and name, what the bot does, and its settings rows.
// The two screens handle routines differently, so each passes its own Routines row.
export function BotFormFields({ form, onChange, bots, selfId, nameError, routinesRow }: {
  form: BotForm;
  onChange: (form: BotForm) => void;
  bots: Bot[];
  selfId: string | null;
  nameError: string | null;
  routinesRow: ReactNode;
}) {
  return (
    <div className="flex flex-col">
      <Identity form={form} onChange={onChange} nameError={nameError} />
      <div className="mt-6 flex flex-col gap-2">
        <Label htmlFor="bot-instructions">What should this bot do?</Label>
        <textarea
          id="bot-instructions"
          value={form.instructions}
          maxLength={20_000}
          onChange={(event) => onChange({ ...form, instructions: event.target.value })}
          className={cn(FIELD, "max-h-[240px] min-h-[72px] resize-none py-3 [field-sizing:content]")}
        />
      </div>
      <div className="mt-6 flex flex-col gap-2">
        <ModelRow provider={form.provider} model={form.model} onChange={(value) => onChange({ ...form, ...value })} />
        <FolderRow projectFolder={form.projectFolder} onChange={(projectFolder) => onChange({ ...form, projectFolder })} />
        <AutonomyRow autonomy={form.autonomy} provider={form.provider} botName={form.name} onChange={(autonomy) => onChange({ ...form, autonomy })} />
        <BrowserRow browser={form.browser} onChange={(browser) => onChange({ ...form, browser })} />
        <CanMessageRow selfId={selfId} canMessage={form.canMessage} bots={bots} onChange={(canMessage) => onChange({ ...form, canMessage })} />
        {routinesRow}
      </div>
    </div>
  );
}
```

- [ ] **Step 5: Run the checks**

Run: `npm run typecheck`, `npm run lint` and `npm run build`
Expected: all pass. The form appears on screen in Tasks 8 and 9, where it's checked by hand.

- [ ] **Step 6: Commit**

```bash
git add packages/ui/components/bot-form
git commit -m "Add the bot form shared by New bot and Bot settings

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: New bot

**Files:**
- Create: `packages/ui/app/bots/new/page.tsx`

**Interfaces:**
- Consumes:
  - Task 5's `NEW_BOT`, `createInput`, `nameProblem`, `routinesSummary` and `SUGGESTIONS`;
  - Task 6's `RoutineEditor`, `RoutineRows`, `EMPTY_ROUTINE` and `localZone`;
  - Task 7's `BotFormFields` and `RoutinesRow`.
- Produces: `/bots/new/` (WF3 Phase 1):
  - the form, Suggestions at the right, and an action bar with Cancel and Create bot;
  - routines added here are drafts, created right after the bot;
  - success opens the new bot's chat.

- [ ] **Step 1: Write the page**

Create `packages/ui/app/bots/new/page.tsx`:

```tsx
"use client";

import { useState } from "react";
import { useRouter } from "next/navigation";
import type { Bot } from "@tjommie/protocol";
import { BotAvatar } from "@/components/bot-avatar";
import { BotFormFields } from "@/components/bot-form/bot-form";
import { RoutinesRow } from "@/components/bot-form/rows";
import { Pill } from "@/components/pill";
import { RoutineEditor } from "@/components/routines/routine-editor";
import { RoutineRows } from "@/components/routines/routine-rows";
import { createInput, nameProblem, NEW_BOT, routinesSummary, type BotForm, type RoutineDraft } from "@/lib/bot-form";
import { request } from "@/lib/connection";
import { errorText } from "@/lib/labels";
import { EMPTY_ROUTINE, localZone } from "@/lib/routine-form";
import { useApp } from "@/lib/store";
import { SUGGESTIONS, type Suggestion } from "@/lib/suggestions";

const chatOf = (bot: Bot): string => `/chat/?bot=${encodeURIComponent(bot.id)}`;

// New bot (WF3): the form, Suggestions at the right, and Create bot. Routines added here are created with the bot.
export default function NewBotPage() {
  const router = useRouter();
  const bots = useApp((state) => state.bots);
  const [form, setForm] = useState<BotForm>(NEW_BOT);
  const [routines, setRoutines] = useState<RoutineDraft[]>([]);
  const [editing, setEditing] = useState<number | "new" | null>(null);
  const [touched, setTouched] = useState(false);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [created, setCreated] = useState<Bot | null>(null);
  const nameError = touched ? nameProblem(form.name, bots, null) : null;

  function applySuggestion(suggestion: Suggestion): void {
    setForm({ ...NEW_BOT, ...suggestion.form });
    setRoutines(suggestion.routines);
    setTouched(false);
    setProblem(null);
  }

  async function create(): Promise<void> {
    setTouched(true);
    if (busy || nameProblem(form.name, bots, null)) return;
    setBusy(true);
    setProblem(null);
    let bot: Bot;
    try {
      bot = await request("bots.create", createInput(form));
    } catch (error) {
      setProblem(errorText(error));
      setBusy(false);
      return;
    }
    const failed: string[] = [];
    for (const routine of routines) {
      try {
        await request("routines.create", { botId: bot.id, ...routine, name: routine.name.trim(), prompt: routine.prompt.trim() });
      } catch (error) {
        failed.push(`${routine.name} (${errorText(error)})`);
      }
    }
    if (failed.length === 0) {
      router.push(chatOf(bot));
      return;
    }
    // The bot exists now, so this screen mustn't offer to create it again.
    setCreated(bot);
    setProblem(`${bot.name} was created, but not these routines: ${failed.join(", ")}.`);
    setBusy(false);
  }

  const draft = editing === "new" ? EMPTY_ROUTINE : editing === null ? null : routines[editing];
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <header className="drag flex h-14 shrink-0 items-center justify-center">
        <div className="flex h-10 items-center gap-2 rounded-full bg-chip px-3.5">
          <BotAvatar color={form.color} shape={form.shape} size={24} />
          <h1 className="text-[15px] font-semibold text-title">New bot</h1>
        </div>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="flex gap-10 pb-10 pl-[120px] pr-6 pt-2 max-xl:pl-10">
          <div className="w-[640px] min-w-0 shrink">
            <BotFormFields
              form={form}
              onChange={setForm}
              bots={bots}
              selfId={null}
              nameError={nameError}
              routinesRow={
                <RoutinesRow summary={routinesSummary(routines)}>
                  <RoutineRows routines={routines.map((routine, index) => ({ ...routine, key: String(index) }))} onOpen={(key) => setEditing(Number(key))} />
                  <button type="button" onClick={() => setEditing("new")} className="mt-2 text-[13px] text-accent hover:underline">
                    Add routine
                  </button>
                </RoutinesRow>
              }
            />
          </div>
          <aside className="ml-auto w-[340px] shrink-0">
            <h2 className="text-[13px] font-semibold leading-[18px] text-label">Suggestions</h2>
            <div className="mt-3 flex flex-col gap-3">
              {SUGGESTIONS.map((suggestion) => (
                <button
                  key={suggestion.title}
                  type="button"
                  onClick={() => applySuggestion(suggestion)}
                  className="press flex items-center gap-4 rounded-[14px] border border-card-stroke bg-card p-4 text-left hover:border-input-stroke"
                >
                  <BotAvatar color={suggestion.form.color} shape={suggestion.form.shape} size={40} className="shrink-0" />
                  <span>
                    <span className="block text-[15px] font-semibold leading-[20px] text-title">{suggestion.title}</span>
                    <span className="mt-0.5 block text-[13px] leading-[18px] text-secondary">{suggestion.description}</span>
                  </span>
                </button>
              ))}
            </div>
          </aside>
        </div>
      </div>
      <footer className="flex h-16 shrink-0 items-center justify-end gap-2 border-t border-card-stroke px-6">
        {problem && (
          <p role="alert" className="mr-auto min-w-0 text-[13px] leading-[18px] text-danger">
            {problem}
          </p>
        )}
        {created ? (
          <Pill primary onClick={() => router.push(chatOf(created))}>
            Open {created.name}
          </Pill>
        ) : (
          <>
            <Pill onClick={() => router.back()}>Cancel</Pill>
            <Pill primary disabled={busy} onClick={() => void create()}>
              Create bot
            </Pill>
          </>
        )}
      </footer>
      {draft && (
        <RoutineEditor
          key={String(editing)}
          title={editing === "new" ? "New routine" : "Edit routine"}
          initial={draft}
          timeZone={localZone()}
          onClose={() => setEditing(null)}
          onSave={async (next) => {
            setRoutines(editing === "new" ? [...routines, next] : routines.map((routine, index) => (index === editing ? next : routine)));
          }}
          onDelete={
            editing === "new"
              ? undefined
              : async () => {
                  setRoutines(routines.filter((_, index) => index !== editing));
                }
          }
        />
      )}
    </div>
  );
}
```

- [ ] **Step 2: Run the checks**

Run: `npm run typecheck`, `npm run lint` and `npm run build`
Expected: all pass. `packages/ui/out/bots/new/index.html` exists.

- [ ] **Step 3: Check it by hand**

In development mode (plan 1D, Task 4 Step 4), check against `docs/wireframes/wf3-phase1-new-bot-dark.png`:
1. The rail's + opens New bot. The chip in the header shows a blue circle, and the rows read Claude · Sonnet (latest), Its own folder, Own folder (open, with the Claude helper line), the browser off, Nobody yet and None yet.
2. Colors and shapes change the 96 px preview and the header chip; the shape tiles take the chosen color.
3. Model: the picker lists Sonnet, Opus and Haiku with a check on Sonnet. The Codex tab shows "Loading the models", then Codex's own list.
4. Folder: A project folder opens Windows' folder picker. The chosen folder's name shows in the row, and "Its own folder" puts it back.
5. Autonomy: Full auto shows "Let this bot work without asking?". Cancel keeps Own folder. With Codex picked, Own folder's helper line mentions reading the user profile.
6. Suggestions:
   - Job hunter fills the name, the orange cloud, the instructions and the browser.
   - Its Routines row reads "Weekdays 9:00 AM" and opens to "Job search (3h)".
7. With the name cleared, Create bot shows "Give the bot a name.". With it filled, Create bot opens the new bot's chat, and the panel lists Job search.

- [ ] **Step 4: Commit**

```bash
git add packages/ui/app/bots/new
git commit -m "Add the New bot screen with Suggestions

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Bot settings

**Files:**
- Create: `packages/ui/app/bots/settings/page.tsx`
- Modify: `packages/ui/components/rail.tsx` (one line)

**Interfaces:**
- Consumes:
  - Task 2's `bots.notes` and `openNotes`;
  - Task 3's `HeaderState`, `ConfirmDialog` and `longDate`;
  - Task 5's `formFromBot`, `patchFor` and `nameProblem`;
  - Tasks 6 and 7.
- Produces: `/bots/settings/?bot=<id>` (WF8):
  - **Left:** the form, filled from the bot.
  - **Right:** About this bot: Created, Home folder with Open folder, and a notes.md preview with Open notes.md.
  - **Action bar:**
    - Delete bot (asks first, WF11) and Pause or Resume bot;
    - Cancel, and Save changes, which sends only what changed and is disabled while nothing has.
  - Routines here save straight to the daemon, like the bot panel's.
  - The rail keeps the bot selected on this page.

- [ ] **Step 1: Write the page**

Create `packages/ui/app/bots/settings/page.tsx`:

```tsx
"use client";

import { Suspense, useEffect, useMemo, useState } from "react";
import { useRouter, useSearchParams } from "next/navigation";
import { Folder, Pause, Play, Trash2 } from "lucide-react";
import type { Bot, Routine } from "@tjommie/protocol";
import { BotAvatar } from "@/components/bot-avatar";
import { BotFormFields } from "@/components/bot-form/bot-form";
import { RoutinesRow } from "@/components/bot-form/rows";
import { HeaderState } from "@/components/chat/chat-header";
import { ConfirmDialog } from "@/components/dialog";
import { Pill } from "@/components/pill";
import { RoutineEditor } from "@/components/routines/routine-editor";
import { RoutineRows } from "@/components/routines/routine-rows";
import { formFromBot, nameProblem, patchFor, routinesSummary, type BotForm } from "@/lib/bot-form";
import { request } from "@/lib/connection";
import { desktop } from "@/lib/desktop";
import { longDate } from "@/lib/format";
import { errorText } from "@/lib/labels";
import { draftFrom, EMPTY_ROUTINE, localZone } from "@/lib/routine-form";
import { removeRoutine, saveRoutine } from "@/lib/routines";
import { useApp } from "@/lib/store";

function Settings() {
  const botId = useSearchParams().get("bot") ?? "";
  const bots = useApp((state) => state.bots);
  const bot = useMemo(() => bots.find((candidate) => candidate.id === botId), [bots, botId]);
  // key: each bot's settings start from that bot.
  return bot ? <BotSettings key={bot.id} bot={bot} bots={bots} /> : <GoHome />;
}

// An unknown or deleted bot has no settings.
function GoHome() {
  const router = useRouter();
  useEffect(() => {
    router.replace("/");
  }, [router]);
  return null;
}

// Bot settings (WF8): the New bot form filled from the bot, About this bot, and the action bar.
function BotSettings({ bot, bots }: { bot: Bot; bots: Bot[] }) {
  const router = useRouter();
  const allRoutines = useApp((state) => state.routines);
  const routines = useMemo(() => allRoutines.filter((routine) => routine.botId === bot.id), [allRoutines, bot.id]);
  const [form, setForm] = useState<BotForm>(() => formFromBot(bot));
  const [editing, setEditing] = useState<Routine | "new" | null>(null);
  const [deleting, setDeleting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const chat = `/chat/?bot=${encodeURIComponent(bot.id)}`;
  const nameError = nameProblem(form.name, bots, bot.id);
  const patch = patchFor(bot, form);
  const dirty = Object.keys(patch).length > 0;

  async function run(action: () => Promise<unknown>, then?: () => void): Promise<void> {
    setBusy(true);
    setProblem(null);
    try {
      await action();
      then?.();
    } catch (error) {
      setProblem(errorText(error));
    } finally {
      setBusy(false);
    }
  }

  const routine = editing === "new" ? null : editing;
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <header className="drag flex h-14 shrink-0 items-center justify-center">
        <div className="flex h-10 min-w-0 max-w-[calc(100%-352px)] items-center gap-2 rounded-full bg-chip px-3.5">
          <BotAvatar color={bot.color} shape={bot.shape} size={24} className="shrink-0" />
          <h1 className="min-w-0 truncate text-[15px] font-semibold text-title">{bot.name}</h1>
          <HeaderState bot={bot} />
        </div>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto">
        <div className="flex gap-10 pb-10 pl-[120px] pr-6 pt-2 max-xl:pl-10">
          <div className="w-[640px] min-w-0 shrink">
            <BotFormFields
              form={form}
              onChange={setForm}
              bots={bots}
              selfId={bot.id}
              nameError={nameError}
              routinesRow={
                <RoutinesRow summary={routinesSummary(routines)}>
                  <RoutineRows routines={routines.map((each) => ({ ...each, key: each.id }))} onOpen={(key) => setEditing(routines.find((each) => each.id === key) ?? null)} />
                  <button type="button" onClick={() => setEditing("new")} className="mt-2 text-[13px] text-accent hover:underline">
                    Add routine
                  </button>
                </RoutinesRow>
              }
            />
          </div>
          <aside className="ml-auto w-[340px] shrink-0">
            <h2 className="text-[13px] font-semibold leading-[18px] text-label">About this bot</h2>
            <About bot={bot} />
          </aside>
        </div>
      </div>
      <footer className="flex h-16 shrink-0 items-center gap-2 border-t border-card-stroke px-6">
        <button type="button" onClick={() => setDeleting(true)} className="mr-2 flex shrink-0 items-center gap-1.5 text-[14px] text-danger">
          <Trash2 className="size-4" aria-hidden />
          Delete bot
        </button>
        <Pill disabled={busy} onClick={() => void run(() => request("bots.pause", { id: bot.id, paused: !bot.paused }))}>
          {bot.paused ? <Play className="size-3.5" aria-hidden /> : <Pause className="size-3.5" aria-hidden />}
          {bot.paused ? "Resume bot" : "Pause bot"}
        </Pill>
        {problem && (
          <p role="alert" className="min-w-0 truncate text-[13px] leading-[18px] text-danger">
            {problem}
          </p>
        )}
        <span className="ml-auto flex shrink-0 gap-2">
          <Pill onClick={() => router.push(chat)}>Cancel</Pill>
          <Pill primary disabled={busy || !dirty || nameError !== null} onClick={() => void run(() => request("bots.update", { id: bot.id, patch }), () => router.push(chat))}>
            Save changes
          </Pill>
        </span>
      </footer>
      <ConfirmDialog
        open={deleting}
        onOpenChange={setDeleting}
        title={`Delete ${bot.name}?`}
        body="Its chat, routines and home folder (notes, inbox and Chrome profile) are deleted. Attached project folders aren't touched."
        confirm={
          <>
            <Trash2 className="size-4" aria-hidden />
            Delete bot
          </>
        }
        danger
        onConfirm={() => {
          setDeleting(false);
          void run(() => request("bots.delete", { id: bot.id }), () => router.replace("/"));
        }}
      />
      {editing && (
        <RoutineEditor
          key={routine?.id ?? "new"}
          title={routine ? "Edit routine" : "New routine"}
          initial={routine ? draftFrom(routine) : EMPTY_ROUTINE}
          timeZone={routine?.timeZone ?? localZone()}
          runsFor={routine?.id}
          onClose={() => setEditing(null)}
          onSave={(draft) => saveRoutine(bot.id, routine, draft)}
          onDelete={routine ? () => removeRoutine(routine) : undefined}
        />
      )}
    </div>
  );
}

// About this bot (WF8): when it was made, its home folder, and the start of its notes.md.
function About({ bot }: { bot: Bot }) {
  const [notes, setNotes] = useState<string | null>(null);
  useEffect(() => {
    request("bots.notes", { id: bot.id }).then(
      ({ text }) => setNotes(text),
      () => setNotes(""),
    );
  }, [bot.id]);
  return (
    <section className="mt-3 rounded-[14px] border border-card-stroke bg-card p-4">
      <p className="text-[12px] leading-[18px] text-secondary">Created</p>
      <p className="mt-0.5 text-[14px] text-text">{longDate(bot.createdAt)}</p>
      <p className="mt-3 text-[12px] leading-[18px] text-secondary">Home folder</p>
      <button type="button" onClick={() => void desktop()?.openFolder(bot.homeFolder)} className="mt-0.5 flex items-center gap-1.5 text-[14px] text-accent hover:underline">
        <Folder className="size-4" aria-hidden />
        Open folder
      </button>
      <p className="mt-3 text-[12px] leading-[18px] text-secondary">Notes</p>
      <pre className="mt-1.5 max-h-[160px] overflow-hidden whitespace-pre-wrap rounded-[10px] bg-canvas p-3 font-sans text-[12px] leading-[18px] text-body [overflow-wrap:anywhere]">
        {notes === null ? "Loading" : notes.trim() || "No notes yet."}
      </pre>
      <button type="button" onClick={() => void desktop()?.openNotes(bot.homeFolder)} className="mt-3 text-[14px] text-accent hover:underline">
        Open notes.md
      </button>
    </section>
  );
}

export default function BotSettingsPage() {
  return (
    <Suspense>
      <Settings />
    </Suspense>
  );
}
```

In `packages/ui/components/rail.tsx`, keep the bot selected on its settings page. Replace the line that sets `openBot` with:

```tsx
  const openBot = pathname.startsWith("/chat") || pathname.startsWith("/bots/settings") ? params.get("bot") : null;
```

- [ ] **Step 2: Run the checks**

Run: `npm run typecheck`, `npm run lint` and `npm run build`
Expected: all pass. `packages/ui/out/bots/settings/index.html` exists.

- [ ] **Step 3: Check it by hand**

In development mode with a bot, check against `docs/wireframes/wf8-bot-settings-dark.png`:
1. Opening Bot settings (the panel's gear, or More, then Bot settings) shows:
   - the bot's chip with its state, the form filled in, and the bot still selected in the rail;
   - About this bot, with the created date and "# <name>'s notes".
2. Save changes is disabled. After a rename it's enabled; Save changes returns to the chat with the new name in the rail and the header.
3. Open folder opens the bot's folder in Explorer, and Open notes.md opens `notes.md` in the default editor.
4. Pause bot turns into Resume bot, and the rail dims the avatar. Resume bot brings it back.
5. Delete bot asks "Delete <name>?". Cancel keeps the bot. Deleting a throwaway bot goes back home, and the rail drops it.

- [ ] **Step 4: Commit**

```bash
git add packages/ui/app/bots/settings packages/ui/components/rail.tsx
git commit -m "Add Bot settings with the notes preview, Pause and Delete

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: The exchange view

**Files:**
- Create: `packages/ui/lib/exchange.ts`, `packages/ui/lib/exchange.test.ts`, `packages/ui/components/exchange/exchange-view.tsx`
- Replace: `packages/ui/components/chat/handoff-row.tsx`, `packages/ui/components/chat/message-list.tsx`, `packages/ui/components/chat/chat-screen.tsx`
- Modify: `packages/ui/components/chat/message-view.tsx`

**Interfaces:**
- Consumes:
  - Task 3's `request` and `exchangePage`;
  - Task 6's `PanelBar`;
  - plan 1D's `loadConversation`, `MessageView`, `Composer`, `BotPanel` and `ChatHeader`.
- Produces:
  - **`lib/exchange.ts`:**
    - `exchangeMeta(exchange, now?)`: "Today 9:31 AM · Replied after 2 minutes", "... · Waiting for a reply" or "... · No reply: <reason>";
    - `exchangeCaption(exchange, messages, bots)`: "Sent back to <bot>." and where any files went.
  - **`ExchangeView({ exchangeId, onClose, bar })`** (WF6):
    - the header with both avatars and the meta line;
    - the exchange's messages under author rows, bubbles up to 400 px;
    - the caption;
    - a composer that posts to the Team room with both bots mentioned as chips.
  - **`HandoffRow`** takes `onOpen` and `selected`. An exchange's row is a button that opens the exchange, and it's selected while open. The Team room answer row links to the room. Both end with a chevron.
  - **`MessageView`** takes `onOpenExchange` and `openExchangeId`, and passes them to handoff rows.
  - **`MessageList({ messages, hasMore, onOlder, render })`:** the caller now draws each message. The Team room reuses the list. It also stays at the bottom when its height changes.
  - **`ChatScreen`:** a handoff row opens the exchange in the right panel, in place of the bot panel.

- [ ] **Step 1: Write the failing tests**

Create `packages/ui/lib/exchange.test.ts`:

```ts
import type { Exchange, Message } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { exchangeCaption, exchangeMeta } from "./exchange";

const now = new Date(2026, 8, 27, 14, 0);
const bots = [
  { id: "b1", name: "Pnet apply" },
  { id: "b2", name: "CV Writer" },
];
const exchange = (extra: Partial<Exchange>): Exchange => ({
  id: "x1",
  fromBotId: "b1",
  toBotId: "b2",
  request: "Tailor the CV for listing PN-48213",
  reply: null,
  status: "waiting",
  createdAt: new Date(2026, 8, 27, 9, 31).toISOString(),
  repliedAt: null,
  ...extra,
});
const file: Message = {
  id: "f1",
  conversationId: "x1",
  author: "b2",
  kind: "file",
  body: "CV-Sabenza-Angular.pdf",
  data: { path: "C:\\bots\\b1\\inbox\\CV-Sabenza-Angular.pdf", name: "CV-Sabenza-Angular.pdf", size: 188_416 },
  status: "done",
  createdAt: now.toISOString(),
};

describe("the exchange view", () => {
  it("says when the exchange was asked and how it ended", () => {
    expect(exchangeMeta(exchange({}), now)).toBe("Today 9:31 AM · Waiting for a reply");
    expect(exchangeMeta(exchange({ status: "replied", reply: "Done.", repliedAt: new Date(2026, 8, 27, 9, 33).toISOString() }), now)).toBe("Today 9:31 AM · Replied after 2 minutes");
    expect(exchangeMeta(exchange({ status: "failed", reply: "it was stopped", repliedAt: new Date(2026, 8, 27, 9, 32).toISOString() }), now)).toBe("Today 9:31 AM · No reply: it was stopped");
  });

  it("says where the answer and its files went", () => {
    const replied = exchange({ status: "replied", reply: "Done." });
    expect(exchangeCaption(replied, [], bots)).toBe("Sent back to Pnet apply.");
    expect(exchangeCaption(replied, [file], bots)).toBe("Sent back to Pnet apply. The file is in Pnet apply's inbox.");
    expect(exchangeCaption(exchange({}), [file], bots)).toBeNull();
  });
});
```

- [ ] **Step 2: Run them to see them fail**

Run: `npx vitest run packages/ui/lib/exchange.test.ts`
Expected: FAIL. `./exchange` can't be found.

- [ ] **Step 3: Write the exchange lines**

Create `packages/ui/lib/exchange.ts`:

```ts
import type { Bot, Exchange, Message } from "@tjommie/protocol";
import { stamp } from "./format";
import { botName } from "./labels";

// How long the answer took: "less than a minute", "2 minutes", "1 hour".
function waited(ms: number): string {
  const minutes = Math.round(ms / 60_000);
  if (minutes < 1) return "less than a minute";
  if (minutes < 60) return `${minutes} ${minutes === 1 ? "minute" : "minutes"}`;
  const hours = Math.round(minutes / 60);
  return `${hours} ${hours === 1 ? "hour" : "hours"}`;
}

// The exchange view's meta line (WF6): "Today 9:31 AM · Replied after 2 minutes".
export function exchangeMeta(exchange: Exchange, now?: Date): string {
  const asked = stamp(exchange.createdAt, now);
  if (exchange.status === "waiting") return `${asked} · Waiting for a reply`;
  if (exchange.status === "failed") return `${asked} · No reply: ${exchange.reply ?? "something went wrong"}`;
  const took = exchange.repliedAt ? Date.parse(exchange.repliedAt) - Date.parse(exchange.createdAt) : 0;
  return `${asked} · Replied after ${waited(took)}`;
}

// The line under a replied exchange: where the answer, and any files, went.
export function exchangeCaption(exchange: Exchange, messages: Message[], bots: Pick<Bot, "id" | "name">[]): string | null {
  if (exchange.status !== "replied") return null;
  const from = botName(bots, exchange.fromBotId);
  const files = messages.filter((message) => message.kind === "file").length;
  if (files === 0) return `Sent back to ${from}.`;
  return `Sent back to ${from}. ${files === 1 ? "The file is" : "The files are"} in ${from}'s inbox.`;
}
```

- [ ] **Step 4: Make handoff rows open exchanges**

Replace `packages/ui/components/chat/handoff-row.tsx` with:

```tsx
import Link from "next/link";
import { ArrowRight, ChevronRight, Loader, Users } from "lucide-react";
import type { Bot, HandoffData } from "@tjommie/protocol";
import { DELETED_BOT } from "@/lib/labels";
import { cn } from "@/lib/utils";
import { BotAvatar, MissingAvatar } from "../bot-avatar";

const ROW = "flex max-w-full items-center gap-2 rounded-[10px] border border-divider px-3 py-2 text-left text-[13px] hover:bg-card";

function Avatar({ bot }: { bot: Bot | undefined }) {
  return bot ? <BotAvatar color={bot.color} shape={bot.shape} size={18} className="shrink-0" /> : <MissingAvatar size={18} />;
}

// A one-line row for work with another bot (WF1, WF11). An exchange's row opens the exchange in the right panel,
// and it's selected while that exchange is open (WF6). A Team room answer's row opens the room.
export function HandoffRow({ body, data, bots, conversationId, onOpen, selected = false }: {
  body: string;
  data: HandoffData;
  bots: Bot[];
  conversationId: string;
  onOpen?: (exchangeId: string) => void;
  selected?: boolean;
}) {
  if ("roomMessageId" in data) {
    return (
      <Link href="/room/" className={ROW}>
        <Users className="size-3.5 shrink-0 text-secondary" aria-hidden />
        <span className="min-w-0 truncate text-body">Answered in the Team room</span>
        <ChevronRight className="size-3.5 shrink-0 text-caption" aria-hidden />
      </Link>
    );
  }
  const from = bots.find((bot) => bot.id === data.fromBotId);
  const to = bots.find((bot) => bot.id === data.toBotId);
  const incoming = conversationId === data.toBotId;
  const fromName = from?.name ?? DELETED_BOT;
  const text = incoming ? `${fromName} asked: ${body}` : `${fromName} asked ${to?.name ?? DELETED_BOT}: ${body}`;
  return (
    <button type="button" aria-pressed={selected} onClick={() => onOpen?.(data.exchangeId)} className={cn(ROW, selected && "border-selected bg-card")}>
      <Avatar bot={from} />
      {!incoming && (
        <>
          <ArrowRight className="size-3 shrink-0 text-secondary" aria-hidden />
          <Avatar bot={to} />
        </>
      )}
      <span className="min-w-0 truncate text-body">{text}</span>
      <span className="flex shrink-0 items-center gap-1 text-caption">
        {data.status === "waiting" ? (
          <>
            <Loader className="size-3 animate-spin text-icon" aria-hidden />
            waiting for a reply
          </>
        ) : data.status === "failed" ? (
          "no reply"
        ) : (
          `${data.messages} ${data.messages === 1 ? "message" : "messages"}`
        )}
      </span>
      <ChevronRight className="size-3.5 shrink-0 text-caption" aria-hidden />
    </button>
  );
}
```

In `packages/ui/components/chat/message-view.tsx`:
1. Replace the function's first line with:

```tsx
export function MessageView({ message, bot, bots, conversationId, onOpenExchange, openExchangeId = null }: {
  message: Message;
  bot: Bot | undefined;
  bots: Bot[];
  conversationId: string;
  onOpenExchange?: (exchangeId: string) => void;
  openExchangeId?: string | null;
}) {
```

2. Replace the `case "handoff":` return with:

```tsx
      return (
        <HandoffRow
          body={message.body}
          data={payload.data}
          bots={bots}
          conversationId={conversationId}
          onOpen={onOpenExchange}
          selected={"exchangeId" in payload.data && payload.data.exchangeId === openExchangeId}
        />
      );
```

- [ ] **Step 5: Let the list draw any conversation**

Replace `packages/ui/components/chat/message-list.tsx` with:

```tsx
"use client";

import { useEffect, useLayoutEffect, useMemo, useRef, type ReactNode } from "react";
import type { Message } from "@tjommie/protocol";
import { timeline } from "@/lib/timeline";
import { Timestamp } from "./notices";

type Props = { messages: Message[]; hasMore: boolean; onOlder: () => Promise<void>; render: (message: Message) => ReactNode };

// A conversation's messages with their time labels: a bot's chat or the Team room. The caller draws each message.
// It follows the newest message while the person is at the bottom, even when the list's own height changes, and
// jumps there when they send one. Near the top it loads the older page and keeps the messages in view where they
// were: the browser's own scroll anchoring is off, and the list moves by what was added.
export function MessageList({ messages, hasMore, onOlder, render }: Props) {
  const scroller = useRef<HTMLDivElement>(null);
  const sentinel = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);
  const firstShown = useRef<{ id: string; top: number } | null>(null);
  const lastId = useRef<string | undefined>(undefined);
  const items = useMemo(() => timeline(messages), [messages]);

  useLayoutEffect(() => {
    const element = scroller.current;
    if (!element) return;
    const last = messages.at(-1);
    const justSent = last !== undefined && last.id !== lastId.current && last.author === "you";
    lastId.current = last?.id;
    const before = firstShown.current;
    const kept = before && before.id !== messages[0]?.id ? element.querySelector<HTMLElement>(`[data-message="${before.id}"]`) : null;
    if (before && kept) {
      element.scrollTop += kept.offsetTop - before.top; // an older page was added above
    } else if (pinned.current || justSent) {
      element.scrollTop = element.scrollHeight; // your own message always comes into view
      pinned.current = true;
    }
    const first = element.querySelector<HTMLElement>("[data-message]");
    firstShown.current = first?.dataset.message ? { id: first.dataset.message, top: first.offsetTop } : null;
  }, [messages]);

  // A popover or attachments under the list, or a resized window, change its height: stay at the bottom.
  useEffect(() => {
    const element = scroller.current;
    if (!element) return;
    const observer = new ResizeObserver(() => {
      if (pinned.current) element.scrollTop = element.scrollHeight;
    });
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const root = scroller.current;
    const target = sentinel.current;
    if (!root || !target || !hasMore) return;
    let loading = false;
    const observer = new IntersectionObserver(
      (entries) => {
        if (loading || !entries.some((entry) => entry.isIntersecting)) return;
        loading = true;
        void onOlder()
          .catch(() => {})
          .finally(() => {
            loading = false;
          });
      },
      { root, rootMargin: "300px 0px 0px 0px" },
    );
    observer.observe(target);
    return () => observer.disconnect();
  }, [hasMore, onOlder]);

  return (
    <div
      ref={scroller}
      onScroll={(event) => {
        const element = event.currentTarget;
        pinned.current = element.scrollHeight - element.scrollTop - element.clientHeight < 48;
      }}
      className="relative min-h-0 flex-1 overflow-y-auto [overflow-anchor:none]"
    >
      <div ref={sentinel} aria-hidden className="h-px" />
      <ol className="flex flex-col gap-3 px-6 pb-4 pt-2">
        {items.map((item) =>
          item.type === "stamp" ? (
            <li key={item.key}>
              <Timestamp label={item.label} />
            </li>
          ) : (
            <li key={item.key} data-message={item.message.id} className="flex flex-col items-start">
              {render(item.message)}
            </li>
          ),
        )}
      </ol>
    </div>
  );
}
```

- [ ] **Step 6: Write the exchange view**

Create `packages/ui/components/exchange/exchange-view.tsx`:

```tsx
"use client";

import Link from "next/link";
import { useEffect, useState, type ReactNode } from "react";
import { ArrowRight, ArrowUp, X } from "lucide-react";
import type { Bot, Exchange, Message } from "@tjommie/protocol";
import { exchangePage, loadConversation, request } from "@/lib/connection";
import { exchangeCaption, exchangeMeta } from "@/lib/exchange";
import { DELETED_BOT, errorText } from "@/lib/labels";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";
import { BotAvatar, MissingAvatar } from "../bot-avatar";
import { MessageView } from "../chat/message-view";
import { IconButton } from "../icon-button";

const NO_MESSAGES: Message[] = [];

function Avatar({ bot, size }: { bot: Bot | undefined; size: number }) {
  return bot ? <BotAvatar color={bot.color} shape={bot.shape} size={size} className="shrink-0" /> : <MissingAvatar size={size} />;
}

// The exchange view (WF6): who asked whom, the whole exchange, and a box that replies in the Team room with both
// bots mentioned. It opens in the right panel, in place of the bot panel or the Team room's members.
export function ExchangeView({ exchangeId, onClose, bar }: { exchangeId: string; onClose: () => void; bar: ReactNode }) {
  const bots = useApp((state) => state.bots);
  const conversation = useApp((state) => state.conversations[exchangeId]);
  const ready = useApp((state) => state.connection.status === "ready");
  const [exchange, setExchange] = useState<Exchange | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const messages = conversation?.status === "ready" ? conversation.messages : NO_MESSAGES;
  const progress = `${messages.length}:${messages.at(-1)?.status ?? ""}`;

  useEffect(() => {
    if (ready && !conversation) void loadConversation(exchangeId, exchangePage(exchangeId));
  }, [ready, conversation, exchangeId]);

  // The header's status lives on the exchange, which has no event of its own, so it's fetched again whenever the
  // exchange's messages move on.
  useEffect(() => {
    let live = true;
    request("exchanges.get", { id: exchangeId }).then(
      (page) => {
        if (live) setExchange(page.exchange);
      },
      (error: unknown) => {
        if (live) setProblem(errorText(error));
      },
    );
    return () => {
      live = false;
    };
  }, [exchangeId, progress]);

  const from = exchange ? bots.find((bot) => bot.id === exchange.fromBotId) : undefined;
  const to = exchange ? bots.find((bot) => bot.id === exchange.toBotId) : undefined;
  const caption = exchange ? exchangeCaption(exchange, messages, bots) : null;
  return (
    <aside aria-label="Exchange" className="flex w-[460px] shrink-0 flex-col border-l border-divider bg-canvas">
      {bar}
      <header className="mx-4 flex shrink-0 items-start gap-3 border-b border-divider pb-3 pt-1">
        <span className="mt-0.5 flex shrink-0 items-center gap-1.5">
          <Avatar bot={from} size={24} />
          <ArrowRight className="size-3.5 text-caption" aria-hidden />
          <Avatar bot={to} size={24} />
        </span>
        <div className="min-w-0 flex-1">
          <h2 className="truncate text-[15px] font-semibold leading-[20px] text-title">
            {exchange ? `${from?.name ?? DELETED_BOT} asked ${to?.name ?? DELETED_BOT}` : "Exchange"}
          </h2>
          <p className="text-[12px] leading-[18px] text-caption">{exchange ? exchangeMeta(exchange) : (problem ?? "Loading")}</p>
        </div>
        <IconButton label="Close the exchange" onClick={onClose}>
          <X aria-hidden />
        </IconButton>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
        <ol className="flex flex-col gap-2">
          {messages.map((message, index) => (
            <li key={message.id} className="flex max-w-[400px] flex-col items-start gap-2">
              {message.author !== "system" && message.author !== messages[index - 1]?.author && <AuthorRow id={message.author} bots={bots} />}
              <MessageView message={message} bot={undefined} bots={bots} conversationId={exchangeId} />
            </li>
          ))}
        </ol>
        {caption && <p className="mt-3 text-[12px] leading-[18px] text-secondary">{caption}</p>}
      </div>
      <ExchangeComposer bots={[from, to].filter((bot): bot is Bot => bot !== undefined)} />
    </aside>
  );
}

function AuthorRow({ id, bots }: { id: string; bots: Bot[] }) {
  const bot = bots.find((candidate) => candidate.id === id);
  return (
    <p className="flex items-center gap-2 text-[12px] font-semibold leading-[18px] text-secondary">
      <Avatar bot={bot} size={18} />
      {bot?.name ?? DELETED_BOT}
    </p>
  );
}

// Replies in the Team room with both bots already @mentioned (spec 6.2). There are no attachments here.
function ExchangeComposer({ bots }: { bots: Bot[] }) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [posted, setPosted] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  async function send(): Promise<void> {
    if (busy || !text.trim() || bots.length === 0) return;
    setBusy(true);
    setProblem(null);
    try {
      await request("room.post", { text: `${bots.map((bot) => `@${bot.name}`).join(" ")} ${text.trim()}` });
      setText("");
      setPosted(true);
    } catch (error) {
      setProblem(errorText(error));
    } finally {
      setBusy(false);
    }
  }
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        void send();
      }}
      className="shrink-0 px-4 pb-4"
    >
      {posted && (
        <p className="mb-2 text-[12px] leading-[18px] text-secondary">
          Posted in the Team room.{" "}
          <Link href="/room/" className="text-accent hover:underline">
            Open room
          </Link>
        </p>
      )}
      <div className="flex items-center gap-2 rounded-[22px] border border-composer-stroke bg-composer py-[5px] pl-2 pr-[5px]">
        {bots.map((bot) => (
          <span key={bot.id} className="inline-flex h-7 shrink-0 items-center gap-1.5 rounded-full bg-pill px-2.5 text-[12px] text-text">
            <BotAvatar color={bot.color} shape={bot.shape} size={18} />@{bot.name}
          </span>
        ))}
        {/* The caret shows focus here, as in the chat's composer. */}
        <input
          value={text}
          onChange={(event) => {
            setText(event.target.value);
            setPosted(false);
          }}
          aria-label="Reply in Team room"
          placeholder="Reply in Team room"
          className="min-w-0 flex-1 truncate bg-transparent text-[14px] text-text outline-none placeholder:text-placeholder"
        />
        <button
          type="submit"
          aria-label="Send"
          disabled={busy || !text.trim()}
          className={cn("flex size-8 shrink-0 items-center justify-center rounded-full", text.trim() ? "bg-user-bubble text-user-text" : "bg-disabled text-placeholder")}
        >
          <ArrowUp className="size-4" aria-hidden />
        </button>
      </div>
      {problem && (
        <p role="alert" className="mt-1.5 text-[12px] leading-[18px] text-danger">
          {problem}
        </p>
      )}
    </form>
  );
}
```

- [ ] **Step 7: Open exchanges from the chat**

Replace `packages/ui/components/chat/chat-screen.tsx` with:

```tsx
"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { useRouter } from "next/navigation";
import type { Bot, Message } from "@tjommie/protocol";
import { botHistory, loadConversation, loadOlder } from "@/lib/connection";
import { AUTONOMY_HINT } from "@/lib/labels";
import { useApp } from "@/lib/store";
import { BotAvatar } from "../bot-avatar";
import { ExchangeView } from "../exchange/exchange-view";
import { PanelBar } from "../panel-bar";
import { BotPanel } from "./bot-panel";
import { ChatHeader } from "./chat-header";
import { Composer } from "./composer";
import { MessageList } from "./message-list";
import { MessageView } from "./message-view";
import { SetupBanner } from "./setup-banner";

const PANEL_KEY = "tjommie.panel";
const NO_MESSAGES: Message[] = [];

// A bot's chat (WF1 Phase 1): the header, the setup banner, the messages, the composer, and the right panel, which
// shows an open exchange (WF6) or else the bot panel.
export function ChatScreen({ botId }: { botId: string }) {
  const router = useRouter();
  const bots = useApp((state) => state.bots);
  const bot = useMemo(() => bots.find((candidate) => candidate.id === botId), [bots, botId]);
  const conversation = useApp((state) => state.conversations[botId]);
  const ready = useApp((state) => state.connection.status === "ready");
  // This screen renders only in the window, never in the static build, so localStorage is there.
  const [panel, setPanel] = useState(() => localStorage.getItem(PANEL_KEY) !== "closed");
  const [exchangeId, setExchangeId] = useState<string | null>(null);

  // Loads the newest page when the chat opens, and again when a new connection clears it.
  useEffect(() => {
    if (ready && bot && !conversation) void loadConversation(botId, botHistory(botId));
  }, [ready, bot, conversation, botId]);
  // An unknown or deleted bot has no chat.
  useEffect(() => {
    if (!bot) router.replace("/");
  }, [bot, router]);
  const older = useCallback(() => loadOlder(botId, botHistory(botId)), [botId]);
  const render = useCallback(
    (message: Message) => <MessageView message={message} bot={bot} bots={bots} conversationId={botId} onOpenExchange={setExchangeId} openExchangeId={exchangeId} />,
    [bot, bots, botId, exchangeId],
  );

  if (!bot) return null;
  const loaded = conversation?.status === "ready" ? conversation : null;
  const messages = loaded?.messages ?? NO_MESSAGES;
  const showPanel = (open: boolean): void => {
    localStorage.setItem(PANEL_KEY, open ? "open" : "closed");
    setPanel(open);
  };
  const togglePanel = (): void => {
    if (exchangeId) setExchangeId(null);
    else showPanel(!panel);
  };
  const settings = `/bots/settings/?bot=${encodeURIComponent(bot.id)}`;
  return (
    <div className="flex min-h-0 flex-1">
      <section aria-label={`Chat with ${bot.name}`} className="flex min-w-0 flex-1 flex-col">
        <ChatHeader bot={bot} panelOpen={panel || exchangeId !== null} onTogglePanel={togglePanel} />
        {bot.state === "needs_setup" && <SetupBanner bot={bot} />}
        {!loaded ? (
          <div className="flex-1" />
        ) : messages.length === 0 ? (
          <EmptyChat bot={bot} />
        ) : (
          <MessageList messages={messages} hasMore={loaded.hasMore} onOlder={older} render={render} />
        )}
        <Composer bot={bot} />
      </section>
      {exchangeId ? (
        <ExchangeView
          key={exchangeId}
          exchangeId={exchangeId}
          onClose={() => setExchangeId(null)}
          bar={<PanelBar settingsHref={settings} settingsLabel="Bot settings" hideLabel="Hide the panel" onHide={() => setExchangeId(null)} />}
        />
      ) : (
        panel && <BotPanel bot={bot} messages={messages} onHide={() => showPanel(false)} />
      )}
    </div>
  );
}

function EmptyChat({ bot }: { bot: Bot }) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-4 px-8 text-center">
      <BotAvatar color={bot.color} shape={bot.shape} size={40} className="opacity-35" />
      <p className="max-w-[360px] text-[14px] leading-[21px] text-body">
        Say hi to {bot.name}. {AUTONOMY_HINT[bot.autonomy]}
      </p>
    </div>
  );
}
```

- [ ] **Step 8: Run the tests and checks**

Run: `npx vitest run packages/ui`
Expected: PASS, including the 2 exchange tests.

Run: `npm run typecheck`, `npm run lint` and `npm run build`
Expected: all pass.

- [ ] **Step 9: Check it by hand**

In development mode, make an exchange between two Claude bots:
- Create Researcher and Writer with `npm run chat -w @tjommie/daemon -- Researcher claude` and `... -- Writer claude`.
- In the window's Bot settings for Researcher, turn on Can message Writer, then Save changes.
- In Researcher's chat, send: "Use the message_bot tool to ask Writer to reply with exactly the word MANGO. When Writer's answer arrives, tell me what Writer said."

Then check against `docs/wireframes/wf6-exchange-view-dark.png`:
1. The handoff row ("Researcher asked Writer: ...") shows "waiting for a reply", then "2 messages", and ends with a chevron.
2. Clicking the row opens the exchange in the right panel:
   - the header "Researcher asked Writer" with "Today <time> · Replied after ...";
   - author rows over the request and the reply, and "Sent back to Researcher.";
   - the row itself is selected.
3. Writer's chat shows the incoming row "Researcher asked: ...". It opens the same exchange.
4. In the exchange view, type "Thanks, both of you." and send. "Posted in the Team room." appears. Both bots answer in the Team room, which Task 11 shows.
5. The x, the panel's >> and the header's panel button all close the exchange.

- [ ] **Step 10: Commit**

```bash
git add packages/ui
git commit -m "Open exchanges from handoff rows in the right panel

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: The Team room

**Files:**
- Create: `packages/ui/lib/mentions.ts`, `packages/ui/lib/mentions.test.ts`, `packages/ui/components/room/room-post.tsx`, `packages/ui/components/room/room-composer.tsx`, `packages/ui/components/room/members-panel.tsx`, `packages/ui/components/room/room-screen.tsx`, `packages/ui/app/room/page.tsx`
- Modify: `packages/ui/components/chat/file-card.tsx`

**Interfaces:**
- Consumes:
  - Task 1's `findMentions` and `mentionRanges`;
  - Task 3's `roomHistory`, `SHORT_STATE` and `STATE_TEXT`;
  - Task 10's `MessageList` and `ExchangeView`;
  - plan 1D's `BotBubble`, `FileCard` and `MessageView`.
- Produces:
  - **`lib/mentions.ts`:** `typedMention(text, caret)`, `mentionOptions(query, bots)` and `insertMention(text, start, caret, name)`.
  - **`/room/`** (WF5):
    - the chip "Team room · N bots";
    - posts with their headers and "Open exchange";
    - your posts, with the mentions in blue;
    - the composer and its mention popover;
    - the Members panel with the chain limit line;
    - the exchange view in place of Members.
  - **`FileCard`:** a size is now optional, because room posts carry file names without sizes.

How the composer's mention popover works:
- It opens while the text just before the caret is `@` and the start of a bot name (or "everyone").
- The `@` must start the text or follow a space; "me@..." is an email.
- Names can have spaces, so the query runs to the caret, and the popover closes when nothing matches.
- Arrow keys move the highlight, Enter or Tab inserts `@Name `, and Escape closes the popover.

- [ ] **Step 1: Write the failing tests**

Create `packages/ui/lib/mentions.test.ts`:

```ts
import type { Bot } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { insertMention, mentionOptions, typedMention } from "./mentions";

const bots = [{ name: "Careers24 scout" }, { name: "Chief of Staff" }, { name: "CV Writer" }] as Bot[];

describe("typing a mention", () => {
  it("finds the @ being typed, and ignores email addresses", () => {
    expect(typedMention("Hi @Ca", 6)).toEqual({ start: 3, query: "Ca" });
    expect(typedMention("@Chief of", 9)).toEqual({ start: 0, query: "Chief of" });
    expect(typedMention("mail me@wr", 10)).toBeNull();
    expect(typedMention("no mention", 10)).toBeNull();
    expect(typedMention("@a\nb", 4)).toBeNull();
  });

  it("offers the bots whose names start with it, then everyone", () => {
    expect(mentionOptions("ca", bots).map((option) => option.name)).toEqual(["Careers24 scout"]);
    expect(mentionOptions("c", bots).map((option) => option.name)).toEqual(["Careers24 scout", "Chief of Staff", "CV Writer"]);
    expect(mentionOptions("", bots).map((option) => option.name)).toEqual(["Careers24 scout", "Chief of Staff", "CV Writer", "everyone"]);
    expect(mentionOptions("ev", bots)).toEqual([{ name: "everyone", bot: null }]);
    expect(mentionOptions("zz", bots)).toEqual([]);
  });

  it("puts the whole mention in, with a space after it", () => {
    expect(insertMention("Hi @Ca please", 3, 6, "Careers24 scout")).toEqual({ text: "Hi @Careers24 scout  please", caret: 20 });
  });
});
```

- [ ] **Step 2: Run them to see them fail**

Run: `npx vitest run packages/ui/lib/mentions.test.ts`
Expected: FAIL. `./mentions` can't be found.

- [ ] **Step 3: Write the typing helpers**

Create `packages/ui/lib/mentions.ts`:

```ts
import type { Bot } from "@tjommie/protocol";

// The "@..." being typed right before the caret: where it starts, and what follows the "@". Names can have spaces,
// so the query runs to the caret. An "@" inside a word ("me@...") is an email address, not a mention.
export function typedMention(text: string, caret: number): { start: number; query: string } | null {
  const before = text.slice(0, caret);
  const at = before.lastIndexOf("@");
  if (at === -1) return null;
  const query = before.slice(at + 1);
  if (query.includes("\n") || query.length > 40) return null;
  if (at > 0 && !/[\s(]/.test(before[at - 1])) return null;
  return { start: at, query };
}

export type MentionOption = { name: string; bot: Bot | null }; // bot null: @everyone

// The bots whose names start with the query (any case), then @everyone. Empty when nothing matches.
export function mentionOptions(query: string, bots: Bot[]): MentionOption[] {
  const typed = query.toLowerCase();
  const options: MentionOption[] = bots.filter((bot) => bot.name.toLowerCase().startsWith(typed)).map((bot) => ({ name: bot.name, bot }));
  if ("everyone".startsWith(typed)) options.push({ name: "everyone", bot: null });
  return options;
}

// Swaps the typed "@..." for the whole mention and a space, and says where the caret goes.
export function insertMention(text: string, start: number, caret: number, name: string): { text: string; caret: number } {
  const mention = `@${name} `;
  return { text: text.slice(0, start) + mention + text.slice(caret), caret: start + mention.length };
}
```

- [ ] **Step 4: Write the posts, the composer and the members**

In `packages/ui/components/chat/file-card.tsx`, room posts carry files without a size:
1. Replace the function's first line with:

```tsx
export function FileCard({ file }: { file: Pick<FileData, "path" | "name"> & { size?: number } }) {
```

2. Replace the size line, `<span className="block text-[12px] leading-[18px] text-caption">{fileSize(file.size)}</span>`, with:

```tsx
        {file.size !== undefined && <span className="block text-[12px] leading-[18px] text-caption">{fileSize(file.size)}</span>}
```

Create `packages/ui/components/room/room-post.tsx`:

```tsx
import type { ReactNode } from "react";
import { ChevronRight } from "lucide-react";
import { ExchangePostData, mentionRanges, type Bot, type Message } from "@tjommie/protocol";
import { clock } from "@/lib/format";
import { DELETED_BOT } from "@/lib/labels";
import { BotAvatar, MissingAvatar } from "../bot-avatar";
import { BotBubble } from "../chat/bubbles";
import { FileCard } from "../chat/file-card";

// A text post in the Team room (WF5). Bots' posts have the avatar, a header line (name, "to <bot>" for bot-to-bot
// posts, time), the bubble, their files and "Open exchange". Your posts are right-aligned, with mentions in blue.
export function RoomPost({ message, bots, onOpenExchange }: { message: Message; bots: Bot[]; onOpenExchange: (exchangeId: string) => void }) {
  if (message.author === "you") return <YourPost text={message.body} bots={bots} />;
  const parsed = ExchangePostData.safeParse(message.data);
  const post = parsed.success ? parsed.data : null;
  const author = bots.find((bot) => bot.id === message.author);
  const to = post ? bots.find((bot) => bot.id === post.toBotId) : undefined;
  return (
    <article className="flex w-full gap-3">
      {author ? <BotAvatar color={author.color} shape={author.shape} size={28} className="mt-0.5 shrink-0" /> : <MissingAvatar size={28} />}
      <div className="flex min-w-0 flex-col items-start gap-1.5">
        <p className="flex items-baseline gap-1.5 text-[13px] leading-[18px]">
          <span className="font-semibold text-title">{author?.name ?? DELETED_BOT}</span>
          {post && <span className="text-secondary">to {to?.name ?? DELETED_BOT}</span>}
          <span className="text-[12px] text-caption">{clock(message.createdAt)}</span>
        </p>
        <div className="max-w-[560px]">
          <BotBubble message={message} />
        </div>
        {post?.files?.map((file) => <FileCard key={file.path} file={file} />)}
        {post && (
          <button type="button" onClick={() => onOpenExchange(post.exchangeId)} className="flex items-center gap-1 text-[12px] leading-[18px] text-accent hover:underline">
            Open exchange
            <ChevronRight className="size-3.5" aria-hidden />
          </button>
        )}
      </div>
    </article>
  );
}

function YourPost({ text, bots }: { text: string; bots: Bot[] }) {
  const parts: ReactNode[] = [];
  let at = 0;
  for (const range of mentionRanges(text, bots.map((bot) => bot.name))) {
    if (range.start > at) parts.push(text.slice(at, range.start));
    parts.push(
      <span key={range.start} className="font-semibold text-mention">
        {text.slice(range.start, range.end)}
      </span>,
    );
    at = range.end;
  }
  parts.push(text.slice(at));
  return (
    <div data-author="you" className="max-w-[560px] self-end whitespace-pre-wrap rounded-[18px] bg-user-bubble px-4 py-3 text-[14px] leading-[21px] text-user-text [overflow-wrap:anywhere]">
      {parts}
    </div>
  );
}
```

Create `packages/ui/components/room/room-composer.tsx`:

```tsx
"use client";

import { useRef, useState, type KeyboardEvent } from "react";
import { ArrowUp, Users } from "lucide-react";
import { findMentions } from "@tjommie/protocol";
import { request } from "@/lib/connection";
import { errorText, SHORT_STATE, STATE_TEXT } from "@/lib/labels";
import { insertMention, mentionOptions, typedMention, type MentionOption } from "@/lib/mentions";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";
import { BotAvatar } from "../bot-avatar";

// The Team room's composer (WF5). Typing "@" opens the mention popover above it, filtered by what follows. A post
// that mentions nobody gets no answer, and the composer says so (WF11). Room posts carry no files.
export function RoomComposer() {
  const bots = useApp((state) => state.bots);
  const field = useRef<HTMLTextAreaElement>(null);
  const [text, setText] = useState("");
  const [caret, setCaret] = useState(0);
  const [highlight, setHighlight] = useState(0);
  const [dismissed, setDismissed] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [hint, setHint] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const typed = typedMention(text, caret);
  const options = typed && typed.start !== dismissed ? mentionOptions(typed.query, bots) : [];
  const open = options.length > 0;
  const active = Math.min(highlight, options.length - 1);

  function edit(next: string, at: number): void {
    setText(next);
    setCaret(at);
    setHighlight(0);
  }

  function pick(option: MentionOption): void {
    if (!typed) return;
    const next = insertMention(text, typed.start, caret, option.name);
    edit(next.text, next.caret);
    requestAnimationFrame(() => {
      field.current?.focus();
      field.current?.setSelectionRange(next.caret, next.caret);
    });
  }

  async function send(): Promise<void> {
    if (busy || !text.trim()) return;
    setBusy(true);
    setProblem(null);
    try {
      await request("room.post", { text });
      setHint(findMentions(text, bots).length === 0);
      edit("", 0);
    } catch (error) {
      setProblem(errorText(error));
    } finally {
      setBusy(false);
    }
  }

  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>): void {
    if (open) {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        setHighlight((active + (event.key === "ArrowDown" ? 1 : options.length - 1)) % options.length);
        return;
      }
      if (event.key === "Enter" || event.key === "Tab") {
        event.preventDefault();
        pick(options[active]);
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        setDismissed(typed?.start ?? null);
        return;
      }
    }
    if (event.key === "Enter" && !event.shiftKey && !event.nativeEvent.isComposing) {
      event.preventDefault();
      void send();
    }
  }

  return (
    <div className="shrink-0 px-4 pb-4">
      {open && (
        <ul id="mention-list" role="listbox" aria-label="Mention a bot" className="mb-2 flex w-[280px] flex-col gap-1 rounded-[16px] border border-divider bg-popover p-1.5">
          {options.map((option, index) => (
            <li
              key={option.name}
              id={`mention-${index}`}
              role="option"
              aria-selected={index === active}
              onMouseDown={(event) => {
                event.preventDefault();
                pick(option);
              }}
              className={cn("flex h-9 cursor-default items-center gap-2.5 rounded-[8px] px-2.5 text-[14px] text-text", index === active && "bg-highlight")}
            >
              {option.bot ? <BotAvatar color={option.bot.color} shape={option.bot.shape} size={18} className="shrink-0" /> : <Users className="size-[18px] shrink-0 text-icon" aria-hidden />}
              <span className="min-w-0 flex-1 truncate">{option.bot ? option.name : "@everyone"}</span>
              <span className={cn("shrink-0 text-[12px]", option.bot ? STATE_TEXT[option.bot.state] : "text-secondary")}>
                {option.bot ? SHORT_STATE[option.bot.state] : `All ${bots.length} bots`}
              </span>
            </li>
          ))}
        </ul>
      )}
      {hint && !text && <p className="mb-2 text-center text-[12px] leading-[18px] text-caption">No bot was mentioned, so nobody will reply. Type @ to ask a bot.</p>}
      <form
        onSubmit={(event) => {
          event.preventDefault();
          void send();
        }}
        className="flex items-end gap-2 rounded-[22px] border border-composer-stroke bg-composer py-[5px] pl-4 pr-[5px]"
      >
        {/* The caret shows focus here, as in the chat's composer. */}
        <textarea
          ref={field}
          value={text}
          rows={1}
          role="combobox"
          aria-label="Message the Team room"
          aria-expanded={open}
          aria-controls="mention-list"
          aria-activedescendant={open ? `mention-${active}` : undefined}
          aria-autocomplete="list"
          placeholder="Message the Team room. Type @ to mention a bot."
          onChange={(event) => edit(event.target.value, event.target.selectionStart)}
          onSelect={(event) => setCaret(event.currentTarget.selectionStart)}
          onKeyDown={onKeyDown}
          className="max-h-[200px] min-h-8 flex-1 resize-none bg-transparent py-[5.5px] text-[14px] leading-[21px] text-text outline-none [field-sizing:content] placeholder:text-placeholder"
        />
        <button
          type="submit"
          aria-label="Send"
          disabled={busy || !text.trim()}
          className={cn("flex size-8 shrink-0 items-center justify-center rounded-full", text.trim() ? "bg-user-bubble text-user-text" : "bg-disabled text-placeholder")}
        >
          <ArrowUp className="size-4" aria-hidden />
        </button>
      </form>
      {problem && (
        <p role="alert" className="mt-1.5 px-3 text-[12px] leading-[18px] text-danger">
          {problem}
        </p>
      )}
    </div>
  );
}
```

Create `packages/ui/components/room/members-panel.tsx`:

```tsx
import Link from "next/link";
import { Info } from "lucide-react";
import { stateLabel, STATE_TEXT } from "@/lib/labels";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";
import { BotAvatar } from "../bot-avatar";
import { PanelBar } from "../panel-bar";

// The Team room's right panel (WF5): every bot with its state in words, and the chain limit with a way to change it.
export function MembersPanel({ onHide }: { onHide: () => void }) {
  const bots = useApp((state) => state.bots);
  const settings = useApp((state) => state.settings);
  return (
    <aside aria-label="Team room members" className="flex w-[460px] shrink-0 flex-col border-l border-divider bg-canvas">
      <PanelBar settingsHref="/settings/?section=bots" settingsLabel="Settings" hideLabel="Hide the members" onHide={onHide} />
      <div className="min-h-0 flex-1 overflow-y-auto px-4 pb-4">
        <h2 className="mb-2 mt-3 text-[13px] font-semibold leading-[18px] text-label">Members</h2>
        <ul className="-mx-2 flex flex-col">
          {bots.map((bot) => (
            <li key={bot.id}>
              <Link href={`/chat/?bot=${encodeURIComponent(bot.id)}`} className="flex h-11 items-center gap-3 rounded-[10px] px-2 hover:bg-card">
                <BotAvatar color={bot.color} shape={bot.shape} size={24} className="shrink-0" />
                <span className="min-w-0 flex-1 truncate text-[14px] text-text">{bot.name}</span>
                <span className={cn("shrink-0 text-[12px]", STATE_TEXT[bot.state])}>{stateLabel(bot)}</span>
              </Link>
            </li>
          ))}
        </ul>
        {settings && (
          <p className="mt-4 flex items-center gap-2 text-[12px] leading-[18px] text-secondary">
            <Info className="size-3.5 shrink-0" aria-hidden />
            Bots can ask each other up to {settings.chainLimit} times per chain.
            <Link href="/settings/?section=bots" className="text-accent hover:underline">
              Change
            </Link>
          </p>
        )}
      </div>
    </aside>
  );
}
```

- [ ] **Step 5: Write the room screen and its page**

Create `packages/ui/components/room/room-screen.tsx`:

```tsx
"use client";

import { useCallback, useEffect, useState } from "react";
import { PanelRight, Users } from "lucide-react";
import { TEAM_ROOM_ID, type Message } from "@tjommie/protocol";
import { loadConversation, loadOlder, roomHistory } from "@/lib/connection";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";
import { MessageList } from "../chat/message-list";
import { MessageView } from "../chat/message-view";
import { ExchangeView } from "../exchange/exchange-view";
import { IconButton } from "../icon-button";
import { PanelBar } from "../panel-bar";
import { MembersPanel } from "./members-panel";
import { RoomComposer } from "./room-composer";
import { RoomPost } from "./room-post";

const PANEL_KEY = "tjommie.panel";

// The Team room (WF5): every bot is a member. Text posts get the room's own layout; activity rows, notices and
// errors (with Retry) sit indented under the posts, drawn like in a chat.
export function RoomScreen() {
  const bots = useApp((state) => state.bots);
  const conversation = useApp((state) => state.conversations[TEAM_ROOM_ID]);
  const ready = useApp((state) => state.connection.status === "ready");
  // This screen renders only in the window, never in the static build, so localStorage is there.
  const [panel, setPanel] = useState(() => localStorage.getItem(PANEL_KEY) !== "closed");
  const [exchangeId, setExchangeId] = useState<string | null>(null);

  useEffect(() => {
    if (ready && !conversation) void loadConversation(TEAM_ROOM_ID, roomHistory);
  }, [ready, conversation]);
  const older = useCallback(() => loadOlder(TEAM_ROOM_ID, roomHistory), []);
  const render = useCallback(
    (message: Message) =>
      message.kind === "text" ? (
        <RoomPost message={message} bots={bots} onOpenExchange={setExchangeId} />
      ) : (
        <div className="w-full pl-10">
          <MessageView message={message} bot={undefined} bots={bots} conversationId={TEAM_ROOM_ID} />
        </div>
      ),
    [bots],
  );

  const loaded = conversation?.status === "ready" ? conversation : null;
  const panelOpen = panel || exchangeId !== null;
  const showPanel = (open: boolean): void => {
    localStorage.setItem(PANEL_KEY, open ? "open" : "closed");
    setPanel(open);
  };
  const togglePanel = (): void => {
    if (exchangeId) setExchangeId(null);
    else showPanel(!panel);
  };
  return (
    <div className="flex min-h-0 flex-1">
      <section aria-label="Team room" className="flex min-w-0 flex-1 flex-col">
        <header className="drag relative flex h-14 shrink-0 items-center justify-center px-4">
          <div className="flex h-10 items-center gap-2 rounded-full bg-chip px-3.5">
            <Users className="size-[18px] text-icon" aria-hidden />
            <h1 className="text-[15px] font-semibold text-title">Team room</h1>
            <span className="text-[13px] text-secondary">
              {bots.length} {bots.length === 1 ? "bot" : "bots"}
            </span>
          </div>
          <div className={cn("absolute top-3", panelOpen ? "right-4" : "right-[154px]")}>
            <IconButton label="Members panel" active={panelOpen} onClick={togglePanel}>
              <PanelRight aria-hidden />
            </IconButton>
          </div>
        </header>
        {!loaded ? (
          <div className="flex-1" />
        ) : loaded.messages.length === 0 ? (
          <p className="m-auto px-8 text-center text-[13px] leading-[18px] text-secondary">No posts yet. @mention a bot to ask it something.</p>
        ) : (
          <MessageList messages={loaded.messages} hasMore={loaded.hasMore} onOlder={older} render={render} />
        )}
        <RoomComposer />
      </section>
      {exchangeId ? (
        <ExchangeView
          key={exchangeId}
          exchangeId={exchangeId}
          onClose={() => setExchangeId(null)}
          bar={<PanelBar settingsHref="/settings/?section=bots" settingsLabel="Settings" hideLabel="Hide the panel" onHide={() => setExchangeId(null)} />}
        />
      ) : (
        panel && <MembersPanel onHide={() => showPanel(false)} />
      )}
    </div>
  );
}
```

Create `packages/ui/app/room/page.tsx`:

```tsx
"use client";

import { RoomScreen } from "@/components/room/room-screen";

export default function RoomPage() {
  return <RoomScreen />;
}
```

- [ ] **Step 6: Run the tests and checks**

Run: `npx vitest run packages/ui`
Expected: PASS, including the 3 mention tests.

Run: `npm run typecheck`, `npm run lint` and `npm run build`
Expected: all pass. `packages/ui/out/room/index.html` exists.

- [ ] **Step 7: Check it by hand**

In development mode with the Researcher and Writer bots from Task 10, check against `docs/wireframes/wf5-team-room-dark.png`:
1. The rail's Team room tile opens the room:
   - the chip reads "Team room · 2 bots";
   - the exchange from Task 10 shows as two posts ("Researcher to Writer", "Writer to Researcher"), each with Open exchange;
   - Members lists both bots with their states, and "Bots can ask each other up to 5 times per chain. Change" is under them.
2. Type "@W": the popover lists Writer and its state. Enter inserts "@Writer ". Finish with "reply with exactly: ROOM-OK" and send. Your bubble shows "@Writer" in blue, and Writer answers in the room.
3. Send "Hello team": "No bot was mentioned, so nobody will reply. Type @ to ask a bot." shows under the list.
4. Open exchange shows the exchange in place of Members; its x brings Members back.
5. Change opens Settings, Bots.

- [ ] **Step 8: Commit**

```bash
git add packages/ui
git commit -m "Add the Team room with mentions, members and exchanges

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 12: The Agents overview

**Files:**
- Create: `packages/ui/lib/overview.ts`, `packages/ui/lib/overview.test.ts`, `packages/ui/components/agents/cards.tsx`, `packages/ui/app/agents/page.tsx`

**Interfaces:**
- Consumes:
  - Task 3's `request`, `Segmented`, `FIELD`, `MENU_POPUP`, `MENU_ITEM`, `SHORT_STATE` and `timeAgo`;
  - plan 1D's `BotAvatar`, `TeamMark`, `stateLabel`, `botName` and `clock`.
- Produces:
  - **`lib/overview.ts`:**
    - `FILTERS` and `OverviewFilter`;
    - `overviewSummary(bots)` ("6 bots, 2 working, 1 waiting for you") and `matches(bot, filter, query)`;
    - `taskLine(bot, messages)`, `nextRoutineLine(bot, routines, now?)`, `talkingWith(botId, messages, now?)` and `roomLine(messages, bots, now?)`.
  - **`/agents/`** (WF2 Phase 1):
    - the header with the summary, the filter, search and New bot;
    - a card per bot, then the Team room card and the New bot card.
    - Each bot card opens its chat. Its menu (on hover) holds Open chat, Pause or Resume bot, and Bot settings.

Where each card's lines come from:

| Line | From |
|---|---|
| Placeholder | The 56 px avatar at 35% (Idle, Needs setup), 50% (Paused, Limited) or full, and the state. A waiting bot's placeholder gets a 2 px accent inner border, and its card the waiting stroke |
| Task line | Paused: "Paused by you". Limited: "Limited until 3:45 PM". Needs setup: the reason. Waiting: the newest open card's summary. Working: the running step, else "Working". Idle: the first line of its newest reply |
| Next routine | "Next: <name>, <time>" (with "Tomorrow" or a weekday when it isn't today), "No routine", or "Paused" |
| Footer | "Talking with" and the bots it had exchanges with today, else "No bot chats today"; a "Review" pill on a waiting bot |
| Team room card | The last two bot-to-bot posts, and "<bot> and <bot> talked 4 minutes ago" |

The page fetches each bot's newest 30 messages, and the room's newest 10, when it opens and whenever a bot changes.

- [ ] **Step 1: Write the failing tests**

Create `packages/ui/lib/overview.test.ts`:

```ts
import type { Bot, Message, Routine } from "@tjommie/protocol";
import { describe, expect, it } from "vitest";
import { matches, nextRoutineLine, overviewSummary, roomLine, talkingWith, taskLine } from "./overview";

const now = new Date(2026, 8, 27, 14, 0);
const bot = (state: Bot["state"], extra: Partial<Bot> = {}) => ({ id: "b1", name: "Pnet apply", state, limitedUntil: null, setupReason: null, paused: state === "paused", ...extra });
const message = (id: string, kind: Message["kind"], extra: Partial<Message> = {}): Message => ({
  id,
  conversationId: "b1",
  author: "b1",
  kind,
  body: "",
  data: null,
  status: "done",
  createdAt: new Date(2026, 8, 27, 9, 0).toISOString(),
  ...extra,
});

describe("the Agents overview", () => {
  it("sums up the bots and filters them", () => {
    expect(overviewSummary([bot("working"), bot("working"), bot("waiting"), bot("idle"), bot("idle"), bot("paused")])).toBe("6 bots, 2 working, 1 waiting for you");
    expect(overviewSummary([bot("idle")])).toBe("1 bot");
    expect(matches({ name: "CV Writer", state: "paused" }, "paused", "")).toBe(true);
    expect(matches({ name: "CV Writer", state: "paused" }, "working", "")).toBe(false);
    expect(matches({ name: "CV Writer", state: "idle" }, "all", "writ")).toBe(true);
    expect(matches({ name: "CV Writer", state: "idle" }, "all", "pnet")).toBe(false);
  });

  it("says what each bot is doing", () => {
    const messages = [
      message("m1", "text", { body: "Morning briefing sent at 7:00 AM.\nDetails below." }),
      message("m2", "approval", { body: "Pnet apply wants to save CV.pdf", data: { approvalId: "a1", tool: "Write", detail: "{}", status: "pending", rule: "Write" } }),
      message("m3", "activity", {
        body: "2 steps",
        data: {
          items: [
            { id: "s1", name: "Read", summary: "Read listing PN-48213", status: "done" },
            { id: "s2", name: "Fill", summary: "Filling the Apply form", status: "running" },
          ],
        },
      }),
    ];
    expect(taskLine(bot("idle"), messages)).toBe("Morning briefing sent at 7:00 AM.");
    expect(taskLine(bot("waiting"), messages)).toBe("Pnet apply wants to save CV.pdf");
    expect(taskLine(bot("working"), messages)).toBe("Filling the Apply form");
    expect(taskLine(bot("paused"), messages)).toBe("Paused by you");
    expect(taskLine(bot("needs_setup", { setupReason: "Sign in to Claude" }), messages)).toBe("Sign in to Claude");
    expect(taskLine(bot("idle"), [])).toBe("No messages yet");
  });

  it("shows the next routine run", () => {
    const routine = (name: string, nextRunAt: string | null, extra: Partial<Routine> = {}): Routine => ({
      id: name,
      botId: "b1",
      name,
      prompt: "Check",
      days: [1],
      times: ["08:48"],
      timeZone: "Africa/Johannesburg",
      timeLimitMinutes: null,
      enabled: true,
      nextRunAt,
      createdAt: now.toISOString(),
      ...extra,
    });
    const routines = [
      routine("Later", new Date(2026, 8, 28, 8, 48).toISOString()),
      routine("Soon", new Date(2026, 8, 27, 17, 0).toISOString()),
      routine("Off", new Date(2026, 8, 27, 15, 0).toISOString(), { enabled: false }),
    ];
    expect(nextRoutineLine({ id: "b1", paused: false }, routines, now)).toBe("Next: Soon, 5:00 PM");
    expect(nextRoutineLine({ id: "b1", paused: false }, [routines[0]], now)).toBe("Next: Later, Tomorrow 8:48 AM");
    expect(nextRoutineLine({ id: "b1", paused: true }, routines, now)).toBe("Paused");
    expect(nextRoutineLine({ id: "b2", paused: false }, routines, now)).toBe("No routine");
  });

  it("finds today's partners and the room's last exchange", () => {
    const handoff = (id: string, from: string, to: string, at: Date) =>
      message(id, "handoff", { createdAt: at.toISOString(), data: { exchangeId: `x-${id}`, fromBotId: from, toBotId: to, messages: 2, status: "replied" } });
    const today = new Date(2026, 8, 27, 9, 31);
    expect(talkingWith("b1", [handoff("h1", "b1", "b2", today), handoff("h2", "b3", "b1", today), handoff("h3", "b1", "b4", new Date(2026, 8, 26, 9, 0))], now)).toEqual(["b2", "b3"]);
    const bots = [
      { id: "b1", name: "Pnet apply" },
      { id: "b2", name: "CV Writer" },
    ];
    const post = message("p1", "text", { conversationId: "team", author: "b2", body: "Done.", createdAt: new Date(2026, 8, 27, 13, 56).toISOString(), data: { exchangeId: "x1", toBotId: "b1" } });
    expect(roomLine([post], bots, now)).toBe("CV Writer and Pnet apply talked 4 minutes ago");
    expect(roomLine([], bots, now)).toBe("No bot chats yet");
  });
});
```

- [ ] **Step 2: Run them to see them fail**

Run: `npx vitest run packages/ui/lib/overview.test.ts`
Expected: FAIL. `./overview` can't be found.

- [ ] **Step 3: Write the overview's lines**

Create `packages/ui/lib/overview.ts`:

```ts
import { ActivityData, ApprovalData, ExchangeHandoffData, ExchangePostData, type Bot, type BotState, type Message, type Routine } from "@tjommie/protocol";
import { clock, dayName, timeAgo } from "./format";
import { botName, stateLabel } from "./labels";

export type OverviewFilter = "all" | "working" | "waiting" | "paused";

export const FILTERS = [
  { value: "all", label: "All" },
  { value: "working", label: "Working" },
  { value: "waiting", label: "Waiting" },
  { value: "paused", label: "Paused" },
] as const satisfies readonly { value: OverviewFilter; label: string }[];

// "6 bots, 2 working, 1 waiting for you".
export function overviewSummary(bots: Pick<Bot, "state">[]): string {
  const count = (state: BotState): number => bots.filter((bot) => bot.state === state).length;
  const parts = [`${bots.length} ${bots.length === 1 ? "bot" : "bots"}`];
  if (count("working")) parts.push(`${count("working")} working`);
  if (count("waiting")) parts.push(`${count("waiting")} waiting for you`);
  return parts.join(", ");
}

export function matches(bot: Pick<Bot, "name" | "state">, filter: OverviewFilter, query: string): boolean {
  const typed = query.trim().toLowerCase();
  if (typed && !bot.name.toLowerCase().includes(typed)) return false;
  return filter === "all" || bot.state === filter;
}

const firstLine = (text: string): string =>
  text
    .split("\n")
    .map((line) => line.trim())
    .find(Boolean) ?? "";

// What the bot is doing, in one line, from its newest messages (WF2's task line).
export function taskLine(bot: Pick<Bot, "id" | "state" | "limitedUntil" | "setupReason">, messages: Message[]): string {
  if (bot.state === "paused") return "Paused by you";
  if (bot.state === "limited") return stateLabel(bot);
  if (bot.state === "needs_setup") return bot.setupReason ?? "Needs setup";
  const newest = [...messages].reverse();
  if (bot.state === "waiting") {
    const card = newest.find((message) => message.kind === "approval" && ApprovalData.safeParse(message.data).data?.status === "pending");
    return card?.body ?? "Waiting for you";
  }
  if (bot.state === "working") {
    const row = newest.find((message) => message.kind === "activity");
    const step = row ? ActivityData.safeParse(row.data).data?.items.at(-1) : undefined;
    return step?.status === "running" ? step.summary : "Working";
  }
  const reply = newest.find((message) => message.author === bot.id && message.kind === "text" && message.body.trim());
  return reply ? firstLine(reply.body) : "No messages yet";
}

// "Next: Morning check, 8:48 AM", "Next: Morning check, Tomorrow 8:48 AM", "No routine" or "Paused".
export function nextRoutineLine(bot: Pick<Bot, "id" | "paused">, routines: Routine[], now: Date = new Date()): string {
  if (bot.paused) return "Paused";
  const next = routines
    .filter((routine) => routine.botId === bot.id && routine.enabled && routine.nextRunAt)
    .sort((a, b) => (a.nextRunAt ?? "").localeCompare(b.nextRunAt ?? ""))[0];
  if (!next?.nextRunAt) return "No routine";
  const day = dayName(next.nextRunAt, now);
  return `Next: ${next.name}, ${day === "Today" ? "" : `${day} `}${clock(next.nextRunAt)}`;
}

// The bots this bot had exchanges with today, from the handoff rows among its newest messages.
export function talkingWith(botId: string, messages: Message[], now: Date = new Date()): string[] {
  const today = now.toDateString();
  const partners = new Set<string>();
  for (const message of messages) {
    if (message.kind !== "handoff" || new Date(message.createdAt).toDateString() !== today) continue;
    const row = ExchangeHandoffData.safeParse(message.data);
    if (row.success) partners.add(row.data.fromBotId === botId ? row.data.toBotId : row.data.fromBotId);
  }
  return [...partners];
}

// The Team room card's line: which bots talked last, and when.
export function roomLine(messages: Message[], bots: Pick<Bot, "id" | "name">[], now: Date = new Date()): string {
  const post = [...messages].reverse().find((message) => message.kind === "text" && ExchangePostData.safeParse(message.data).success);
  if (!post) return "No bot chats yet";
  const { toBotId } = ExchangePostData.parse(post.data);
  return `${botName(bots, post.author)} and ${botName(bots, toBotId)} talked ${timeAgo(post.createdAt, now)}`;
}
```

- [ ] **Step 4: Write the cards and the page**

Create `packages/ui/components/agents/cards.tsx`:

```tsx
"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { CircleAlert, Clock, Ellipsis, Hourglass, Loader, MessageCircle, Pause, Play, Plug, Plus, Settings } from "lucide-react";
import { Menu } from "@base-ui/react/menu";
import { ExchangePostData, type Bot, type Message, type Routine } from "@tjommie/protocol";
import { request } from "@/lib/connection";
import { botName, DELETED_BOT, SHORT_STATE, stateLabel } from "@/lib/labels";
import { nextRoutineLine, roomLine, talkingWith, taskLine } from "@/lib/overview";
import { cn } from "@/lib/utils";
import { BotAvatar, MissingAvatar, TeamMark } from "../bot-avatar";
import { MENU_ITEM, MENU_POPUP } from "../menu";

const CARD = "relative flex h-[368px] flex-col rounded-[14px] border bg-card p-3";
// The card's title is a link stretched over the whole card; the menu sits above it.
const STRETCHED = "after:absolute after:inset-0 after:rounded-[14px]";
const chatOf = (id: string): string => `/chat/?bot=${encodeURIComponent(id)}`;

// A bot's card (WF2 Phase 1).
export function BotCard({ bot, bots, messages, routines }: { bot: Bot; bots: Bot[]; messages: Message[]; routines: Routine[] }) {
  const waiting = bot.state === "waiting";
  const dim = bot.state === "idle" || bot.state === "needs_setup" ? "opacity-35" : bot.state === "paused" || bot.state === "limited" ? "opacity-50" : undefined;
  const partners = talkingWith(bot.id, messages)
    .map((id) => bots.find((each) => each.id === id))
    .filter((each): each is Bot => each !== undefined);
  return (
    <article className={cn(CARD, "group", waiting ? "border-waiting-stroke" : "border-card-stroke hover:border-input-stroke")}>
      <div className={cn("flex h-[181px] shrink-0 flex-col items-center justify-center gap-3 rounded-[10px] bg-popover", waiting && "ring-2 ring-inset ring-accent")}>
        <BotAvatar color={bot.color} shape={bot.shape} size={56} className={dim} />
        <PlaceholderState bot={bot} />
      </div>
      <div className="mt-4 flex items-center gap-2.5">
        <BotAvatar color={bot.color} shape={bot.shape} size={28} className="shrink-0" />
        <h2 className="min-w-0 flex-1 truncate text-[15px] font-semibold leading-[20px] text-title">
          <Link href={chatOf(bot.id)} className={STRETCHED}>
            {bot.name}
          </Link>
        </h2>
        <StateChip bot={bot} />
        <CardMenu bot={bot} />
      </div>
      <p className="mt-3 truncate text-[13px] leading-[18px] text-body">{taskLine(bot, messages)}</p>
      <p className="mt-1.5 flex items-center gap-1.5 text-[12px] leading-[18px] text-secondary">
        <Clock className="size-3.5 shrink-0" aria-hidden />
        <span className="truncate">{nextRoutineLine(bot, routines)}</span>
      </p>
      <div className="mt-auto flex h-10 items-center gap-2 border-t border-card-stroke pt-3">
        {partners.length > 0 ? (
          <>
            <span className="text-[12px] text-caption">Talking with</span>
            <span className="flex -space-x-1">
              {partners.map((partner) => (
                <BotAvatar key={partner.id} color={partner.color} shape={partner.shape} size={18} />
              ))}
            </span>
          </>
        ) : (
          <span className="text-[12px] text-caption">No bot chats today</span>
        )}
        {waiting && <span className="ml-auto rounded-full bg-user-bubble px-3 py-1 text-[13px] font-medium text-user-text">Review</span>}
      </div>
    </article>
  );
}

function PlaceholderState({ bot }: { bot: Bot }) {
  if (bot.state === "working")
    return (
      <span className="flex items-center gap-1.5 text-[13px] text-body">
        <Loader className="size-3.5" aria-hidden />
        Working
      </span>
    );
  if (bot.state === "waiting") return <span className="text-[13px] text-accent">Waiting for you</span>;
  if (bot.state === "paused")
    return (
      <span className="flex items-center gap-1.5 text-[13px] text-secondary">
        <Pause className="size-3.5" aria-hidden />
        Paused
      </span>
    );
  if (bot.state === "idle") return <span className="text-[12px] text-placeholder">Idle</span>;
  return <span className="text-[12px] text-secondary">{stateLabel(bot)}</span>;
}

// The chip at the right of the title row (h 22, WF2).
function StateChip({ bot }: { bot: Bot }) {
  const tone = bot.state === "working" ? "bg-pill text-text" : bot.state === "waiting" ? "bg-waiting-fill text-accent" : "bg-popover text-secondary";
  const Icon =
    bot.state === "working" ? Loader : bot.state === "waiting" ? CircleAlert : bot.state === "paused" ? Pause : bot.state === "limited" ? Hourglass : bot.state === "needs_setup" ? Plug : null;
  return (
    <span className={cn("flex h-[22px] shrink-0 items-center gap-1 rounded-full px-2 text-[12px]", tone)}>
      {Icon && <Icon className="size-3" aria-hidden />}
      {bot.state === "waiting" ? "Waiting for you" : SHORT_STATE[bot.state]}
    </span>
  );
}

function CardMenu({ bot }: { bot: Bot }) {
  const router = useRouter();
  return (
    <Menu.Root>
      <Menu.Trigger
        aria-label={`Menu for ${bot.name}`}
        className="relative z-10 flex size-7 shrink-0 items-center justify-center rounded-full bg-pill text-icon opacity-0 hover:text-text focus-visible:opacity-100 group-hover:opacity-100 data-[popup-open]:opacity-100"
      >
        <Ellipsis className="size-4" aria-hidden />
      </Menu.Trigger>
      <Menu.Portal>
        <Menu.Positioner side="bottom" align="end" sideOffset={6}>
          <Menu.Popup className={MENU_POPUP}>
            <Menu.Item className={MENU_ITEM} onClick={() => router.push(chatOf(bot.id))}>
              <MessageCircle aria-hidden />
              Open chat
            </Menu.Item>
            <Menu.Item className={MENU_ITEM} onClick={() => void request("bots.pause", { id: bot.id, paused: !bot.paused }).catch(() => {})}>
              {bot.paused ? <Play aria-hidden /> : <Pause aria-hidden />}
              {bot.paused ? "Resume bot" : "Pause bot"}
            </Menu.Item>
            <Menu.Item className={MENU_ITEM} onClick={() => router.push(`/bots/settings/?bot=${encodeURIComponent(bot.id)}`)}>
              <Settings aria-hidden />
              Bot settings
            </Menu.Item>
          </Menu.Popup>
        </Menu.Positioner>
      </Menu.Portal>
    </Menu.Root>
  );
}

// The Team room's card (WF2): the last two bot-to-bot posts, and who talked last.
export function TeamRoomCard({ messages, bots }: { messages: Message[]; bots: Bot[] }) {
  const posts = messages.filter((message) => message.kind === "text" && ExchangePostData.safeParse(message.data).success).slice(-2);
  return (
    <article className={cn(CARD, "border-card-stroke hover:border-input-stroke")}>
      <div className="flex h-[181px] shrink-0 flex-col justify-center gap-3 overflow-hidden rounded-[10px] bg-popover p-3">
        {posts.length === 0 ? (
          <p className="text-center text-[12px] text-placeholder">No bot chats yet</p>
        ) : (
          posts.map((post) => {
            const author = bots.find((each) => each.id === post.author);
            const { toBotId } = ExchangePostData.parse(post.data);
            return (
              <div key={post.id} className="flex gap-2">
                {author ? <BotAvatar color={author.color} shape={author.shape} size={14} className="mt-0.5 shrink-0" /> : <MissingAvatar size={14} />}
                <div className="min-w-0">
                  <p className="text-[11px] leading-[16px] text-caption">
                    <span className="font-semibold text-title">{author?.name ?? DELETED_BOT}</span> to {botName(bots, toBotId)}
                  </p>
                  <p className="line-clamp-3 text-[11px] leading-[16px] text-body">{post.body}</p>
                </div>
              </div>
            );
          })
        )}
      </div>
      <div className="mt-4 flex items-center gap-2.5">
        <TeamMark size={28} />
        <h2 className="text-[15px] font-semibold leading-[20px] text-title">
          <Link href="/room/" className={STRETCHED}>
            Team room
          </Link>
        </h2>
      </div>
      <p className="mt-3 truncate text-[13px] leading-[18px] text-body">{roomLine(messages, bots)}</p>
      <div className="mt-auto flex h-10 items-center justify-end pt-3">
        <span className="rounded-full bg-pill px-3 py-1 text-[13px] text-text">Open room</span>
      </div>
    </article>
  );
}

export function NewBotCard() {
  return (
    <Link href="/bots/new/" className="flex h-[368px] flex-col items-center justify-center gap-3 rounded-[14px] border border-dashed border-selected text-center hover:bg-card">
      <span className="flex size-12 items-center justify-center rounded-full bg-popover text-icon">
        <Plus className="size-6" aria-hidden />
      </span>
      <span className="text-[15px] font-semibold text-title">New bot</span>
      <span className="text-[12px] text-secondary">Start from a suggestion or from scratch</span>
    </Link>
  );
}
```

Create `packages/ui/app/agents/page.tsx`:

```tsx
"use client";

import Link from "next/link";
import { useEffect, useState } from "react";
import { Plus, Search } from "lucide-react";
import type { Message } from "@tjommie/protocol";
import { BotCard, NewBotCard, TeamRoomCard } from "@/components/agents/cards";
import { FIELD, Segmented } from "@/components/controls";
import { IconButton } from "@/components/icon-button";
import { request } from "@/lib/connection";
import { FILTERS, matches, overviewSummary, type OverviewFilter } from "@/lib/overview";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";

const NO_MESSAGES: Message[] = [];

// The Agents overview (WF2 Phase 1).
export default function AgentsPage() {
  const bots = useApp((state) => state.bots);
  const routines = useApp((state) => state.routines);
  const ready = useApp((state) => state.connection.status === "ready");
  const [filter, setFilter] = useState<OverviewFilter>("all");
  const [query, setQuery] = useState("");
  const [searching, setSearching] = useState(false);
  const [digests, setDigests] = useState<Record<string, Message[]>>({});
  const [room, setRoom] = useState<Message[]>(NO_MESSAGES);

  // Each card reads its bot's newest messages: fetched when the page opens, and again whenever a bot changes.
  useEffect(() => {
    if (!ready) return;
    let live = true;
    for (const bot of bots) {
      request("chat.history", { botId: bot.id, limit: 30 }).then(
        ({ messages }) => {
          if (live) setDigests((current) => ({ ...current, [bot.id]: messages }));
        },
        () => {},
      );
    }
    request("room.history", { limit: 10 }).then(
      ({ messages }) => {
        if (live) setRoom(messages);
      },
      () => {},
    );
    return () => {
      live = false;
    };
  }, [bots, ready]);

  const shown = bots.filter((bot) => matches(bot, filter, query));
  const everything = filter === "all" && !query.trim();
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <header className="drag flex shrink-0 items-start justify-between gap-6 px-6 pb-6 pt-12">
        <div>
          <h1 className="text-[20px] font-semibold leading-[26px] text-title">Agents</h1>
          <p className="text-[13px] leading-[18px] text-secondary">{overviewSummary(bots)}</p>
        </div>
        <div className="flex items-center gap-2">
          <Segmented label="Show" value={filter} options={FILTERS} onChange={setFilter} />
          {searching ? (
            <input
              autoFocus
              value={query}
              onChange={(event) => setQuery(event.target.value)}
              onBlur={() => {
                if (!query) setSearching(false);
              }}
              placeholder="Search bots"
              aria-label="Search bots"
              className={cn(FIELD, "h-8 w-[200px] rounded-full px-3 text-[13px]")}
            />
          ) : (
            <IconButton label="Search bots" onClick={() => setSearching(true)} className="rounded-full bg-pill">
              <Search aria-hidden />
            </IconButton>
          )}
          <Link href="/bots/new/" className="press inline-flex h-8 items-center gap-1.5 rounded-full bg-user-bubble px-3.5 text-[13px] font-medium text-user-text">
            <Plus className="size-4" aria-hidden />
            New bot
          </Link>
        </div>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto px-6 pb-6">
        <div className="grid grid-cols-[repeat(auto-fill,minmax(280px,1fr))] gap-4">
          {shown.map((bot) => (
            <BotCard key={bot.id} bot={bot} bots={bots} messages={digests[bot.id] ?? NO_MESSAGES} routines={routines} />
          ))}
          {everything && <TeamRoomCard messages={room} bots={bots} />}
          {everything && <NewBotCard />}
        </div>
        {!everything && shown.length === 0 && <p className="mt-10 text-center text-[13px] text-secondary">No bots match.</p>}
      </div>
    </div>
  );
}
```

- [ ] **Step 5: Run the tests and checks**

Run: `npx vitest run packages/ui`
Expected: PASS, including the 4 overview tests.

Run: `npm run typecheck`, `npm run lint` and `npm run build`
Expected: all pass. `packages/ui/out/agents/index.html` exists.

- [ ] **Step 6: Check it by hand**

In development mode with the bots from Tasks 10 and 11, check against `docs/wireframes/wf2-phase1-agents-overview-dark.png`:
1. The rail's grid icon opens Agents: "2 bots", a card per bot, the Team room card showing their last exchange, and the New bot card.
2. Researcher's card says "Talking with" with Writer's avatar. The next-routine line reads "No routine".
3. While a bot works, its card shows "Working" with a loader. Afterwards it shows the first line of its reply.
4. Hovering a card shows its menu. Pause bot dims the avatar and makes the chip "Paused". The Paused filter then shows only that bot; resume it.
5. Search: typing "wri" leaves only Writer.
6. Clicking a card opens that bot's chat. The Team room card opens the room.

- [ ] **Step 7: Commit**

```bash
git add packages/ui
git commit -m "Add the Agents overview

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 13: End-to-end: the new screens with real bots

**Files:**
- Create: `packages/desktop/src/real-app.ts`, `packages/desktop/src/screens.real.test.ts`
- Replace: `packages/desktop/src/desktop.real.test.ts`
- Modify: `package.json` (root, the `test:real` script)

**Interfaces:**
- Consumes: every screen in this plan, and plan 1D's app.
- Produces: `real-app.ts`, the helpers both real test files share, and the screenshot script (Task 15) uses:
  - `launchRealApp(executablePath?)`, which starts the app and waits for first-run setup;
  - `closeRealApp(app, quit)` and `quitApp(app)`;
  - `daemonConnection(page)` and `claudeBot(client, name, extra?)`;
  - `openChat(page, name)`, `send(page, name, text)` and `botReply(page, text)`;
  - `listeningPid(port)`, `refused(port)` and `eventually(check, timeoutMs?)`.

  Plan 1D's real test moves onto them. `npm run test:real` builds the UI first, so the test files no longer do.

- [ ] **Step 1: Share the real-app helpers**

Create `packages/desktop/src/real-app.ts`:

```ts
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, rmSync } from "node:fs";
import { createRequire } from "node:module";
import { createConnection } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { connect, type Client } from "@tjommie/client";
import type { Bot, BotInput } from "@tjommie/protocol";
import { _electron, type ElectronApplication, type Page } from "playwright-core";
import { realAccountsDir, tempRoot } from "../../daemon/src/testing.ts";

type Bridge = { tjommie: { connection(): Promise<{ ok: true; port: number; token: string } | { ok: false; error: string }> } };

export type RealApp = { electronApp: ElectronApplication; page: Page; client: Client; root: string; userData: string };

const desktopDir = resolve(import.meta.dirname, "..");

// The real desktop app for the real tests and the screenshot script: production mode from source (or a packaged
// Tjommie.exe), a disposable data root and Electron profile, and the account folders signed in with
// `npm run signin -w @tjommie/daemon`. Production mode serves packages/ui/out: build the UI first.
export async function launchRealApp(executablePath?: string): Promise<RealApp> {
  const root = tempRoot();
  const userData = mkdtempSync(join(tmpdir(), "tjommie-electron-"));
  const inherited = Object.fromEntries(Object.entries(process.env).filter((entry): entry is [string, string] => entry[1] !== undefined && entry[0] !== "TJOMMIE_DEV"));
  const electronApp = await _electron.launch({
    executablePath: executablePath ?? (createRequire(import.meta.url)("electron") as string),
    args: executablePath ? [] : [desktopDir],
    env: {
      ...inherited,
      TJOMMIE_DESKTOP: "production",
      TJOMMIE_HOME: root,
      TJOMMIE_PORT: "0",
      TJOMMIE_ACCOUNTS_DIR: realAccountsDir(),
      TJOMMIE_USER_DATA: userData,
      TJOMMIE_BROWSER_HEADLESS: "1",
    },
  });
  const page = await electronApp.firstWindow();
  await page.getByRole("heading", { name: "Set up Tjommie" }).waitFor({ timeout: 120_000 });
  const { port, token } = await daemonConnection(page);
  return { electronApp, page, client: await connect(port, token), root, userData };
}

// Closes the test's client and the app (unless a test already quit it), then removes the disposable folders.
export async function closeRealApp(app: RealApp | undefined, quit: boolean): Promise<void> {
  if (!app) return;
  await app.client.close().catch(() => {});
  if (!quit) await app.electronApp.close().catch(() => {});
  for (const folder of [app.root, app.userData]) await remove(folder);
}

// Quits the way the tray's Quit Tjommie does, and waits for Electron to exit.
export async function quitApp(app: RealApp): Promise<void> {
  const closed = app.electronApp.waitForEvent("close");
  await app.electronApp.evaluate(({ app: electron }) => electron.quit()).catch(() => {}); // the app may exit before it answers
  await closed;
}

// The connection the app handed its UI, read through the same bridge.
export async function daemonConnection(page: Page): Promise<{ port: number; token: string }> {
  const result = await page.evaluate(() => (globalThis as unknown as Bridge).tjommie.connection());
  if (!result.ok) throw new Error(result.error);
  return { port: result.port, token: result.token };
}

export const claudeBot = (client: Client, name: string, extra: Partial<BotInput> = {}): Promise<Bot> =>
  client.call("bots.create", {
    name,
    color: "green",
    shape: "pill",
    instructions: "You are a terse test bot. Answer in one short line.",
    provider: "claude",
    model: "haiku",
    ...extra,
  });

// Names match exactly: Playwright's default is a case-insensitive substring, so "Writer" would also match "CV Writer".
export async function openChat(page: Page, name: string): Promise<void> {
  await page.getByRole("link", { name: new RegExp(`^${name},`) }).click();
  await page.getByRole("heading", { level: 1, name, exact: true }).waitFor();
}

export async function send(page: Page, name: string, text: string): Promise<void> {
  const box = page.getByRole("textbox", { name: `Message ${name}` });
  await box.fill(text);
  await box.press("Enter");
  await page.locator('[data-author="you"]', { hasText: text }).waitFor();
}

export const botReply = (page: Page, text: string) => page.locator('[data-author="bot"]', { hasText: text });

export function listeningPid(port: number): number {
  const output = execFileSync("powershell.exe", ["-NoProfile", "-Command", `(Get-NetTCPConnection -LocalPort ${port} -State Listen).OwningProcess`], { encoding: "utf8" });
  return Number(output.trim().split(/\s+/)[0]);
}

export function refused(port: number): Promise<boolean> {
  return new Promise((done) => {
    const socket = createConnection({ host: "127.0.0.1", port });
    socket.once("connect", () => {
      socket.destroy();
      done(false);
    });
    socket.once("error", () => done(true));
  });
}

export async function eventually<T>(check: () => Promise<T | undefined>, timeoutMs = 60_000): Promise<T> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await check().catch(() => undefined);
    if (value !== undefined) return value;
    if (Date.now() > deadline) throw new Error(`Timed out after ${timeoutMs} ms`);
    await new Promise((done) => setTimeout(done, 500));
  }
}

// Windows can hold a file for a moment after its process exits.
async function remove(folder: string): Promise<void> {
  for (let attempt = 0; attempt < 20 && existsSync(folder); attempt++) {
    try {
      rmSync(folder, { recursive: true, force: true });
    } catch {
      await new Promise((done) => setTimeout(done, 500));
    }
  }
}
```

Replace `packages/desktop/src/desktop.real.test.ts` with plan 1D's tests, now on the shared helpers:

```ts
import { existsSync } from "node:fs";
import { join } from "node:path";
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { botReply, claudeBot, closeRealApp, daemonConnection, eventually, launchRealApp, listeningPid, openChat, quitApp, refused, send, type RealApp } from "./real-app.ts";

// The real desktop app with real Claude bots: the chat (plan 1D).
let app: RealApp | undefined;
let quit = false;

beforeAll(async () => {
  app = await launchRealApp();
});

afterAll(async () => {
  await closeRealApp(app, quit);
});

function running(): RealApp {
  if (!app) throw new Error("The desktop app didn't start.");
  return app;
}

describe("the desktop app with real bots", () => {
  it("sends a message and shows the reply", async () => {
    const { client, page } = running();
    await claudeBot(client, "Echo");
    await openChat(page, "Echo");
    await send(page, "Echo", "Reply with exactly: PONG");
    await botReply(page, "PONG").waitFor({ timeout: 180_000 });
  });

  it("asks before a change and carries on after Allow once", async () => {
    const { client, page } = running();
    const bot = await claudeBot(client, "Careful", { autonomy: "ask" });
    await openChat(page, "Careful");
    await send(page, "Careful", "Use the Write tool to create hello.txt containing the word hi. Then reply DONE.");
    const card = page.getByRole("group", { name: "Waiting for you" });
    await card.waitFor({ timeout: 180_000 });
    await card.getByRole("button", { name: "Allow once" }).click();
    await page.getByText(/^Allowed once: /).waitFor();
    await botReply(page, "DONE").waitFor({ timeout: 180_000 });
    expect(existsSync(join(bot.homeFolder, "hello.txt"))).toBe(true);
  });

  it("stops a bot that's working", async () => {
    const { client, page } = running();
    await claudeBot(client, "Sleeper", { autonomy: "full" });
    await openChat(page, "Sleeper");
    await send(page, "Sleeper", `Use the Bash tool, not in the background, to run exactly: node -e "setTimeout(() => console.log('done'), 60000)". Then tell me what it printed.`);
    await page.getByRole("button", { name: /^\d+ steps?: / }).waitFor({ timeout: 180_000 }); // the command is running
    await page.getByRole("button", { name: "Stop" }).click();
    await page.getByText("Stopped.", { exact: true }).waitFor({ timeout: 30_000 });
    await page.getByRole("link", { name: "Sleeper, Idle" }).waitFor({ timeout: 30_000 });
  });

  it("reconnects after the daemon dies", async () => {
    const { page } = running();
    const before = await daemonConnection(page);
    const pid = listeningPid(before.port);
    process.kill(pid);
    // The app starts a new daemon with a new token, and hands the UI the new connection.
    const after = await eventually(async () => {
      const next = await daemonConnection(page);
      return next.token === before.token ? undefined : next;
    });
    expect(listeningPid(after.port)).not.toBe(pid);
    await page.getByText("Reconnecting to Tjommie").waitFor({ state: "hidden", timeout: 60_000 });
    // The UI's calls and events now go to the new daemon.
    await openChat(page, "Echo");
    await page.getByRole("button", { name: "More" }).click();
    await page.getByRole("menuitem", { name: "Pause bot" }).click();
    await page.getByRole("link", { name: "Echo, Paused" }).waitFor();
    await page.getByRole("button", { name: "More" }).click();
    await page.getByRole("menuitem", { name: "Resume bot" }).click();
    await page.getByRole("link", { name: "Echo, Idle" }).waitFor();
  });

  it("quitting Tjommie stops its daemon", async () => {
    const { page } = running();
    const { port } = await daemonConnection(page);
    await quitApp(running());
    quit = true;
    expect(await refused(port)).toBe(true);
  });
});
```

In the root `package.json`, build the UI before the real tests. The script becomes:

```json
    "test:real": "npm run build && vitest run --mode real --passWithNoTests",
```

- [ ] **Step 2: Write the screen tests**

Create `packages/desktop/src/screens.real.test.ts`:

```ts
import { afterAll, beforeAll, describe, expect, it } from "vitest";
import { botReply, claudeBot, closeRealApp, launchRealApp, openChat, send, type RealApp } from "./real-app.ts";

// The screens from plan 1E, used the way a person would, with real Claude bots. The tests run in order and build
// on each other: first run, a bot from a Suggestion, its routines and settings, the Team room, an exchange, the
// Agents overview, Settings, and deleting a bot.
let app: RealApp | undefined;

beforeAll(async () => {
  app = await launchRealApp();
});

afterAll(async () => {
  await closeRealApp(app, false);
});

function running(): RealApp {
  if (!app) throw new Error("The desktop app didn't start.");
  return app;
}

describe("the Phase 1 screens with real bots", () => {
  it("shows first-run setup with both providers signed in", async () => {
    const { page } = running();
    await expect.poll(() => page.getByText(/^Signed in/).count(), { timeout: 120_000 }).toBe(2);
  });

  it("creates a bot from a suggestion, with its routine, and chats with it", async () => {
    const { page } = running();
    await page.getByRole("link", { name: "Create your first bot" }).click();
    await page.getByRole("button", { name: /^Job hunter/ }).click();
    await expect.poll(() => page.getByLabel("Name").inputValue()).toBe("Job hunter");
    await page.getByRole("button", { name: /^Model/ }).click();
    await page.getByRole("button", { name: "Haiku (latest)" }).click();
    await page.getByRole("button", { name: "Create bot" }).click();
    await page.getByRole("heading", { level: 1, name: "Job hunter", exact: true }).waitFor();
    await page.getByRole("button", { name: /^Job search \(3h\)/ }).waitFor(); // the bot panel lists its routine
    await send(page, "Job hunter", "Reply with exactly: PONG");
    await botReply(page, "PONG").waitFor({ timeout: 180_000 });
  });

  it("adds, edits and deletes a routine from the bot panel", async () => {
    const { page } = running();
    await page.getByRole("button", { name: "Add", exact: true }).click();
    const create = page.getByRole("dialog", { name: "New routine" });
    await create.getByLabel("Name").fill("Morning check");
    await create.getByLabel("What to do").fill("Check the news and tell me one headline.");
    await create.getByRole("button", { name: "Save" }).click();
    await page.getByRole("button", { name: /^Morning check Weekdays at 9:00 AM/ }).click();
    const edit = page.getByRole("dialog", { name: "Edit routine" });
    await edit.getByRole("radio", { name: "1 hour" }).click();
    await edit.getByRole("button", { name: "Save" }).click();
    await page.getByRole("button", { name: /^Morning check \(1h\)/ }).click();
    await page.getByRole("dialog", { name: "Edit routine" }).getByRole("button", { name: "Delete routine" }).click();
    await page.getByRole("button", { name: /^Morning check/ }).waitFor({ state: "detached" });
  });

  it("renames a bot in Bot settings and shows its notes", async () => {
    const { page } = running();
    await page.getByRole("link", { name: "Bot settings", exact: true }).click();
    await page.getByText("# Job hunter's notes").waitFor();
    await page.getByLabel("Name").fill("Job scout");
    await page.getByRole("button", { name: "Save changes" }).click();
    await page.getByRole("heading", { level: 1, name: "Job scout", exact: true }).waitFor();
  });

  it("answers a mention in the Team room, and hints when nobody was mentioned", async () => {
    const { page } = running();
    await page.getByRole("link", { name: "Team room", exact: true }).click();
    await page.getByText("No posts yet. @mention a bot to ask it something.").waitFor();
    const box = page.getByRole("combobox", { name: "Message the Team room" });
    await box.fill("Hello team");
    await box.press("Enter");
    await page.getByText("No bot was mentioned, so nobody will reply. Type @ to ask a bot.").waitFor();
    await box.fill("@Jo");
    await page.getByRole("option", { name: /Job scout/ }).click();
    await box.pressSequentially("reply with exactly: ROOM-OK");
    await box.press("Enter");
    await page.locator('[data-author="you"]', { hasText: "@Job scout reply with exactly: ROOM-OK" }).waitFor();
    await botReply(page, "ROOM-OK").waitFor({ timeout: 180_000 });
  });

  it("opens an exchange from a handoff row and replies in the Team room", async () => {
    const { client, page } = running();
    const writer = await claudeBot(client, "Writer");
    await claudeBot(client, "Researcher", { canMessage: [writer.id] });
    await openChat(page, "Researcher");
    await send(page, "Researcher", "Use the message_bot tool to ask Writer to reply with exactly the word MANGO. When Writer's answer arrives, tell me what Writer said.");
    const row = page.getByRole("button", { name: /Researcher asked Writer/ });
    await row.getByText(/messages?$/).waitFor({ timeout: 240_000 }); // replied
    await row.click();
    const view = page.getByRole("complementary", { name: "Exchange" });
    await view.getByText(/Replied after/).waitFor({ timeout: 30_000 });
    await view.locator('[data-author="bot"]', { hasText: "MANGO" }).waitFor();
    await view.getByRole("textbox", { name: "Reply in Team room" }).fill("Thanks, both of you.");
    await view.getByRole("button", { name: "Send" }).click();
    await view.getByText("Posted in the Team room.").waitFor();
  });

  it("shows every bot on the Agents overview, and filters them", async () => {
    const { page } = running();
    await page.getByRole("link", { name: "Agents", exact: true }).click();
    await page.getByRole("heading", { level: 1, name: "Agents", exact: true }).waitFor();
    for (const name of ["Job scout", "Writer", "Researcher"]) await page.getByRole("link", { name, exact: true }).waitFor();
    await page.getByRole("button", { name: "Menu for Writer" }).click();
    await page.getByRole("menuitem", { name: "Pause bot" }).click();
    await page.getByRole("radio", { name: "Paused" }).click();
    await page.getByRole("link", { name: "Writer", exact: true }).waitFor();
    await page.getByRole("link", { name: "Job scout", exact: true }).waitFor({ state: "detached" });
    await page.getByRole("button", { name: "Menu for Writer" }).click();
    await page.getByRole("menuitem", { name: "Resume bot" }).click();
    await page.getByRole("radio", { name: "All" }).click();
  });

  it("switches the theme and keeps the chain limit", async () => {
    const { client, page } = running();
    await page.getByRole("link", { name: "Settings", exact: true }).click();
    await page.getByRole("radio", { name: "Light" }).click();
    await expect.poll(async () => (await page.locator("html").getAttribute("class")) ?? "").not.toContain("dark");
    await page.getByRole("radio", { name: "Dark" }).click();
    await page.getByRole("link", { name: "Bots", exact: true }).click();
    await page.getByRole("button", { name: "Increase" }).click();
    await expect.poll(async () => (await client.call("settings.get", {})).chainLimit).toBe(6);
  });

  it("deletes a bot after confirming", async () => {
    const { client, page } = running();
    await openChat(page, "Writer");
    await page.getByRole("link", { name: "Bot settings", exact: true }).click();
    await page.getByRole("button", { name: "Delete bot" }).click();
    await page.getByRole("dialog", { name: "Delete Writer?" }).getByRole("button", { name: "Delete bot" }).click();
    await expect.poll(async () => (await client.call("bots.list", {})).some((bot) => bot.name === "Writer"), { timeout: 60_000 }).toBe(false);
  });
});
```

- [ ] **Step 3: Run them**

Run: `npm run typecheck`
Expected: no errors.

Run: `npm run build`, then `npx vitest run --mode real packages/desktop/src`
Expected: PASS, 5 desktop tests and 8 screen tests, in several minutes. If a step times out, the screenshot Playwright saves on failure and `logs\desktop.log` in the test's data root show where it stopped.

- [ ] **Step 4: Commit**

```bash
git add package.json packages/desktop/src
git commit -m "Drive every Phase 1 screen with real bots in the end-to-end tests

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 14: The Windows installer

**Files:**
- Create: `packages/desktop/scripts/package.ts`, `packages/desktop/src/packaged.real.test.ts`
- Modify: `packages/desktop/src/main.ts`, `package.json` (root: a script and two dev dependencies), `.gitignore`, `README.md`

**Interfaces:**
- Consumes:
  - plan 1D's desktop main process and daemon;
  - Task 13's `launchRealApp`, `quitApp`, `closeRealApp`, `daemonConnection` and `refused`.
- Produces:
  - `npm run package`: `dist-desktop\installer\Tjommie Setup <version>.exe`, and the unpacked app in `dist-desktop\installer\win-unpacked`.
  - The main process knows when it's installed (`app.isPackaged`). Then it uses production mode, reads the daemon, UI and icons from `process.resourcesPath`, and uses the app id `com.tsnyders.tjommie`.

The installed layout:

| Where | What | How it gets there |
|---|---|---|
| `resources\app.asar` | `src\main.mjs` (the main process, bundled), `src\preload.cjs`, `package.json` | rolldown, and a copy |
| `resources\daemon\main.mjs` | The daemon, bundled with the protocol, `ws` and `zod` | rolldown |
| `resources\daemon\node_modules\` | `@playwright/mcp`, `playwright`, `playwright-core` | copied from the workspace's own installs |
| `resources\ui\` | The static export | a copy of `packages/ui/out` |
| `resources\assets\` | The icons | a copy of `packages/desktop/assets` |

- [ ] **Step 1: Teach the main process where the installed files are**

In `packages/desktop/src/main.ts`, replace the block of constants from `const here = import.meta.dirname;` through `const appIcon = join(assets, "icon-256.png");` with:

```ts
const here = import.meta.dirname;
const packaged = app.isPackaged;
// Three ways to run. Development: the development daemon and `npm run dev` run separately. Production from source
// (TJOMMIE_DESKTOP=production): this process starts the daemon and serves the static export. Installed: the same,
// with the daemon, the UI and the icons as plain files in resources (npm run package).
const production = packaged || process.env.TJOMMIE_DESKTOP === "production";
const dataRoot = process.env.TJOMMIE_HOME ?? join(process.env.LOCALAPPDATA ?? join(homedir(), ".local", "share"), "Tjommie");
const assets = packaged ? join(process.resourcesPath, "assets") : join(here, "..", "assets");
const uiDir = packaged ? join(process.resourcesPath, "ui") : join(here, "..", "..", "ui", "out");
const daemonEntry = packaged ? join(process.resourcesPath, "daemon", "main.mjs") : join(here, "..", "..", "daemon", "src", "main.ts");
const appIcon = join(assets, "icon-256.png");
const APP_ID = "com.tsnyders.tjommie";
```

Replace the two lines that set the app user model id with:

```ts
  // Windows shows notifications under this id; the installer gives the Start menu shortcut the same one.
  app.setAppUserModelId(packaged ? APP_ID : process.execPath);
```

- [ ] **Step 2: Write the package script**

Run: `npm install -D electron-builder@^26.15.3 rolldown@^1.2.11`
Expected: the root `package.json` lists both under `devDependencies`.

In the root `package.json`, add this script:

```json
    "package": "node --disable-warning=ExperimentalWarning packages/desktop/scripts/package.ts",
```

Add `dist-desktop/` to `.gitignore`.

Create `packages/desktop/scripts/package.ts`:

```ts
// Builds the Windows installer: npm run package. It lands in dist-desktop\installer, next to the unpacked app
// (win-unpacked) that the packaged smoke test runs.
import { execSync } from "node:child_process";
import { cpSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import builder from "electron-builder";
import { rolldown } from "rolldown";

const repo = resolve(import.meta.dirname, "..", "..", "..");
const desktop = join(repo, "packages", "desktop");
const out = join(repo, "dist-desktop");
const app = join(out, "app"); // what goes into app.asar: the main process and the preload
const resources = join(out, "resources"); // plain files beside app.asar: the daemon, the UI and the icons
const require = createRequire(import.meta.url);
const version = (require(join(repo, "package.json")) as { version: string }).version;
const electronVersion = (require("electron/package.json") as { version: string }).version;

// Node refuses to strip types under node_modules, so the installed app runs bundles, not the .ts sources.
async function bundle(input: string, dir: string, external: string[]): Promise<void> {
  const build = await rolldown({ input, platform: "node", external });
  await build.write({ dir, format: "esm", entryFileNames: "main.mjs", chunkFileNames: "[name]-[hash].mjs" });
  await build.close();
}

// Playwright MCP runs as its own process, so it ships as a package beside the daemon, with what it needs. These are
// the exact copies the daemon resolves in the workspace.
function copyPlaywrightMcp(target: string): void {
  const mcp = dirname(createRequire(join(repo, "packages", "daemon", "package.json")).resolve("@playwright/mcp/package.json"));
  const playwright = dirname(createRequire(join(mcp, "package.json")).resolve("playwright/package.json"));
  const core = dirname(createRequire(join(playwright, "package.json")).resolve("playwright-core/package.json"));
  const packages: [string, string][] = [
    ["@playwright/mcp", mcp],
    ["playwright", playwright],
    ["playwright-core", core],
  ];
  for (const [name, from] of packages) cpSync(from, join(target, "node_modules", name), { recursive: true });
}

rmSync(out, { recursive: true, force: true });
execSync("npm run build", { cwd: repo, stdio: "inherit" }); // the static export

await bundle(join(desktop, "src", "main.ts"), join(app, "src"), ["electron"]);
cpSync(join(desktop, "src", "preload.cjs"), join(app, "src", "preload.cjs"));
writeFileSync(
  join(app, "package.json"),
  JSON.stringify({ name: "tjommie", productName: "Tjommie", version, description: "AI bots on your own PC", author: "TSnyders", license: "Apache-2.0", type: "module", main: "src/main.mjs" }, null, 2),
);

// ws's optional native helpers stay out; ws uses its own JavaScript without them.
await bundle(join(repo, "packages", "daemon", "src", "main.ts"), join(resources, "daemon"), ["@playwright/mcp", "bufferutil", "utf-8-validate"]);
copyPlaywrightMcp(join(resources, "daemon"));
cpSync(join(repo, "packages", "ui", "out"), join(resources, "ui"), { recursive: true });
cpSync(join(desktop, "assets"), join(resources, "assets"), { recursive: true });

await builder.build({
  targets: builder.Platform.WINDOWS.createTarget("nsis", builder.Arch.x64),
  config: {
    appId: "com.tsnyders.tjommie",
    productName: "Tjommie",
    electronVersion,
    directories: { app, output: join(out, "installer") },
    extraResources: [{ from: resources, to: ".", filter: ["**/*"] }],
    asar: true,
    npmRebuild: false,
    win: { target: "nsis", icon: join(desktop, "assets", "icon-256.png") },
    // One click, for this Windows user only, no admin. Uninstalling keeps %LOCALAPPDATA%\Tjommie and the bots.
    nsis: { oneClick: true, perMachine: false, deleteAppDataOnUninstall: false },
    publish: null,
  },
});
```

- [ ] **Step 3: Write the packaged smoke test**

Create `packages/desktop/src/packaged.real.test.ts`:

```ts
import { existsSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { closeRealApp, daemonConnection, launchRealApp, quitApp, refused, type RealApp } from "./real-app.ts";

// The installed layout, from `npm run package`. Skipped when there's no build.
const exe = join(resolve(import.meta.dirname, "..", "..", ".."), "dist-desktop", "installer", "win-unpacked", "Tjommie.exe");

describe.skipIf(!existsSync(exe))("the packaged app", () => {
  it("the packaged app starts its own daemon and shows first-run setup", async () => {
    let app: RealApp | undefined;
    let quit = false;
    try {
      app = await launchRealApp(exe); // it waits for "Set up Tjommie", served from resources\ui
      expect(await app.client.call("bots.list", {})).toEqual([]);
      // Playwright MCP starts only when a bot with the browser works, so check the files it runs from.
      expect(existsSync(join(dirname(exe), "resources", "daemon", "node_modules", "@playwright", "mcp", "cli.js"))).toBe(true);
      const { port } = await daemonConnection(app.page);
      await quitApp(app);
      quit = true;
      expect(await refused(port)).toBe(true);
    } finally {
      await closeRealApp(app, quit);
    }
  });
});
```

- [ ] **Step 4: Build it and run the test**

Run: `npm run package`
Expected:
- `dist-desktop\installer\Tjommie Setup 0.1.0.exe` exists.
- `dist-desktop\installer\win-unpacked\resources` holds `app.asar`, `daemon\main.mjs`, `daemon\node_modules\@playwright\mcp`, `ui\index.html` and `assets\tray-32.png`.

The first run downloads Electron and NSIS for electron-builder.

Run: `npx vitest run --mode real packages/desktop/src/packaged.real.test.ts`
Expected: PASS, 1 test.

Run: `npm run typecheck`
Expected: no errors (the script is under `packages/desktop/scripts`, which the root tsconfig includes).

- [ ] **Step 5: Install it by hand**

1. Run `dist-desktop\installer\Tjommie Setup 0.1.0.exe`. Windows SmartScreen says it's unrecognized: choose More info, then Run anyway.
2. Tjommie installs without an admin prompt and opens.
3. The Start menu has Tjommie, and the tray shows the three blobs.
4. Your existing bots are there, because the installed app uses `%LOCALAPPDATA%\Tjommie`.
5. Quit from the tray. `Get-NetTCPConnection -LocalPort 47100 -State Listen -ErrorAction SilentlyContinue` returns nothing.
6. Keep it installed for the Phase 1 demo (Task 16).

- [ ] **Step 6: Update the README**

In `README.md`, add this section before `## Autonomy`:

```markdown
## Install

`npm run package` builds the Windows installer, `dist-desktop\installer\Tjommie Setup 0.1.0.exe`. It installs Tjommie for the current Windows user, without admin rights, and starts it. The installer isn't signed yet, so Windows SmartScreen asks once: choose "More info", then "Run anyway". Uninstalling keeps your bots and their data in `%LOCALAPPDATA%\Tjommie`. `npm run test:real` also runs a smoke test on the unpacked build when there is one.
```

- [ ] **Step 7: Commit**

```bash
git add package.json package-lock.json .gitignore README.md packages/desktop
git commit -m "Build a Windows installer with the daemon, UI and Playwright MCP inside

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 15: Screenshot checks against the wireframes

**Files:**
- Create: `packages/desktop/scripts/screenshots.ts`, `docs/screenshots/phase1/README.md`, and the PNGs the script writes to `docs/screenshots/phase1/`
- Modify: any UI file the comparison shows is off

**Interfaces:**
- Consumes: Task 13's `real-app.ts`, and every screen.
- Produces:
  - The script `node --disable-warning=ExperimentalWarning packages/desktop/scripts/screenshots.ts`: it drives the running app with real Claude bots through every Phase 1 screen, at 1440 × 900, and saves one PNG per frame.
  - A review record, `docs/screenshots/phase1/README.md`.

What real bots can't be made to show on demand (the user's rule: no faked states) isn't in the screenshots. Here's where each is covered instead:

| State | Covered by |
|---|---|
| Limited, and the Limited rail badge | The daemon's limit tests (plan 1A), and plan 1D's rail rules |
| Needs setup and the banner | Task 4's `cardView` tests; the banner by hand with a signed-out account |
| A failed or missed routine run | Task 6's `runLine` tests |
| The streaming caret, a queued message, an error with Retry | Plan 1D's state and bubble tests, and its end-to-end run |

- [ ] **Step 1: Write the script**

Create `packages/desktop/scripts/screenshots.ts`:

```ts
// Screenshots of every Phase 1 screen, from the running app with real bots: nothing is faked. Build the UI first:
// npm run build, then node --disable-warning=ExperimentalWarning packages/desktop/scripts/screenshots.ts
import { mkdirSync } from "node:fs";
import { join, resolve } from "node:path";
import type { Page } from "playwright-core";
import { botReply, claudeBot, closeRealApp, eventually, launchRealApp, openChat, send } from "../src/real-app.ts";

const out = join(resolve(import.meta.dirname, "..", "..", ".."), "docs", "screenshots", "phase1");
mkdirSync(out, { recursive: true });
const shot = (page: Page, name: string) => page.screenshot({ path: join(out, `${name}.png`) });

async function theme(page: Page, choice: "Light" | "Dark"): Promise<void> {
  await page.getByRole("link", { name: "Settings", exact: true }).click();
  await page.getByRole("radio", { name: choice }).click();
}

const app = await launchRealApp();
try {
  const { electronApp, page, client } = app;
  await electronApp.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setContentSize(1440, 900)); // the frames' size

  // WF9: first run, once both providers are checked.
  await eventually(async () => ((await page.getByText(/^Signed in/).count()) === 2 ? true : undefined), 120_000);
  await shot(page, "wf9-first-run-dark");

  // WF10: Settings.
  await page.getByRole("link", { name: "Settings", exact: true }).click();
  await page.getByRole("heading", { name: "General", exact: true }).waitFor();
  await shot(page, "wf10-settings-general-dark");
  await page.getByRole("link", { name: "Bots", exact: true }).click();
  await page.getByText("Requests between bots per chain").waitFor();
  await shot(page, "wf10-settings-bots-dark");

  // Bots like WF1's. CV Writer is made through the API to save time; Pnet apply the way a person would.
  await claudeBot(client, "CV Writer", { color: "green", shape: "rounded-square", instructions: "You tailor CVs. Answer in one short line." });
  await page.getByRole("link", { name: "New bot", exact: true }).click();
  await page.getByRole("button", { name: /^Job hunter/ }).click();
  await page.getByLabel("Name").fill("Pnet apply");
  await page.getByRole("button", { name: /^Can message/ }).click();
  await page.getByRole("switch", { name: "Can message CV Writer" }).click();
  await shot(page, "wf3-new-bot-dark"); // WF3
  await page.getByRole("button", { name: /^Model/ }).click();
  await page.getByRole("button", { name: "Haiku (latest)" }).click();
  await page.getByRole("radio", { name: "Always ask" }).click();
  await page.getByRole("button", { name: "Create bot" }).click();
  await page.getByRole("heading", { level: 1, name: "Pnet apply", exact: true }).waitFor();

  // WF1 and WF6: an exchange with a shared file, then an approval card left waiting.
  await send(page, "Pnet apply", "Use the message_bot tool to ask CV Writer to write a file called cv-note.txt containing the word hello in its own folder, share it with share_file, and reply DONE. When CV Writer answers, tell me in one line.");
  await page.getByRole("button", { name: /Pnet apply asked CV Writer/ }).getByText(/messages?$/).waitFor({ timeout: 300_000 });
  await send(page, "Pnet apply", "Use the Write tool to create notes-test.txt containing hi. Then reply DONE.");
  await page.getByRole("group", { name: "Waiting for you" }).waitFor({ timeout: 180_000 });
  await shot(page, "wf1-main-chat-dark");
  await theme(page, "Light");
  await openChat(page, "Pnet apply");
  await shot(page, "wf1-main-chat-light");
  await theme(page, "Dark");
  await openChat(page, "Pnet apply");
  await page.getByRole("button", { name: /Pnet apply asked CV Writer/ }).click();
  await page.getByRole("complementary", { name: "Exchange" }).getByText(/Replied after/).waitFor();
  await shot(page, "wf6-exchange-view-dark");
  await page.getByRole("button", { name: "Close the exchange" }).click();

  // WF7: a routine that has really run. It's due a minute or two from now, after the card is answered.
  await page.getByRole("group", { name: "Waiting for you" }).getByRole("button", { name: "Allow once" }).click();
  await page.getByText(/^Allowed once: /).waitFor();
  const pnet = (await client.call("bots.list", {})).find((bot) => bot.name === "Pnet apply");
  if (!pnet) throw new Error("Pnet apply wasn't created.");
  const due = new Date(Date.now() + 120_000);
  const time = `${String(due.getHours()).padStart(2, "0")}:${String(due.getMinutes()).padStart(2, "0")}`;
  const routine = await client.call("routines.create", { botId: pnet.id, name: "Pnet apply 9am", prompt: "Reply with exactly: ROUTINE-OK", days: [due.getDay()], times: [time], timeLimitMinutes: 180 });
  await eventually(async () => ((await client.call("routines.runs", { routineId: routine.id })).some((run) => run.status === "done") ? true : undefined), 600_000);
  await page.getByRole("button", { name: /^Pnet apply 9am/ }).click();
  await page.getByRole("dialog", { name: "Edit routine" }).getByText("Done", { exact: true }).waitFor();
  await shot(page, "wf7-routine-editor-dark");
  await page.keyboard.press("Escape");

  // WF8, and WF11's dialogs and model picker.
  await page.getByRole("link", { name: "Bot settings", exact: true }).click();
  await page.getByText("# Pnet apply's notes").waitFor();
  await shot(page, "wf8-bot-settings-dark");
  await page.getByRole("radio", { name: "Full auto" }).click();
  const fullAuto = page.getByRole("dialog", { name: "Let Pnet apply work without asking?" });
  await fullAuto.waitFor();
  await shot(page, "wf11-full-auto-dark");
  await fullAuto.getByRole("button", { name: "Cancel" }).click();
  await page.getByRole("button", { name: "Delete bot" }).click();
  const remove = page.getByRole("dialog", { name: "Delete Pnet apply?" });
  await remove.waitFor();
  await shot(page, "wf11-delete-bot-dark");
  await remove.getByRole("button", { name: "Cancel" }).click();
  await page.getByRole("button", { name: /^Model/ }).click();
  await page.getByRole("button", { name: "Haiku (latest)" }).waitFor();
  await shot(page, "wf11-model-picker-dark");
  await page.keyboard.press("Escape");

  // WF5: an answered mention, and the mention popover open over a new post.
  await page.getByRole("link", { name: "Team room", exact: true }).click();
  const box = page.getByRole("combobox", { name: "Message the Team room" });
  await box.fill("@CV Writer reply with exactly: ROOM-OK");
  await box.press("Enter");
  await botReply(page, "ROOM-OK").waitFor({ timeout: 180_000 });
  await box.fill("Can someone check careers24.com for new React roles? @C");
  await page.getByRole("option").first().waitFor();
  await shot(page, "wf5-team-room-dark");
  await box.fill("");

  // WF2.
  await page.getByRole("link", { name: "Agents", exact: true }).click();
  await page.getByRole("heading", { level: 1, name: "Agents", exact: true }).waitFor();
  await shot(page, "wf2-agents-overview-dark");
} finally {
  await closeRealApp(app, false);
}
```

- [ ] **Step 2: Take the screenshots**

Run: `npm run build`, then `node --disable-warning=ExperimentalWarning packages/desktop/scripts/screenshots.ts`
Expected: 14 PNGs in `docs/screenshots/phase1/`, in 10 to 20 minutes, because the routine waits for its minute.

Run: `npm run typecheck`
Expected: no errors.

- [ ] **Step 3: Compare each screenshot with its wireframe, and fix what's off**

Open each pair with the Read tool, the screenshot and the export in `docs/wireframes/`. Compare the layout, sizes, colors and copy rules, not the content: the real bots' words differ from the samples. Checkpoints, from `docs/design/wireframe-spec.md`:

| Screenshot | Wireframe | Check |
|---|---|---|
| wf1-main-chat-dark | wf1-phase1-main-chat-dark | <ul><li>The 88 px rail with its state marks; the 40 px header chip, centered.</li><li>Bot bubbles up to 600 px, radius 18.</li><li>The handoff rows, the file card, and the Waiting for you card with the Always allow note.</li><li>The 44 px composer, radius 22, 16 px in.</li><li>The 460 px panel: summary, Browser with Show window, Routines with Add, Recent activity.</li><li>The caption buttons over the panel's bar.</li></ul> |
| wf1-main-chat-light | wf1-main-chat-light | <ul><li>Every surface and text in its light token.</li><li>The composer's 1 px stroke.</li><li>Dark caption buttons.</li></ul> |
| wf2-agents-overview-dark | wf2-phase1-agents-overview-dark | <ul><li>The summary line; filter, search and New bot.</li><li>314 × 368 cards, 4 across.</li><li>The placeholders' opacity by state.</li><li>The Team room and New bot cards.</li></ul> |
| wf3-new-bot-dark | wf3-phase1-new-bot-dark | <ul><li>The 156 px tile with the 96 px avatar; swatches with the ring on the chosen one; shape tiles in the bot's color.</li><li>44 px fields; 56 px rows, radius 12; Autonomy open with its helper.</li><li>Suggestions at the right; the 64 px action bar.</li></ul> |
| wf5-team-room-dark | wf5-team-room-dark | <ul><li>Posts: the 28 px avatar, name, "to <bot>", time and Open exchange.</li><li>Your post at the right, with blue mentions.</li><li>The popover above the composer.</li><li>Members with their state colors, and the chain line with Change.</li></ul> |
| wf6-exchange-view-dark | wf6-exchange-view-dark | <ul><li>The header's avatars, arrow and meta line.</li><li>Author rows; bubbles up to 400 px; the file card and caption.</li><li>The composer's two chips.</li><li>The selected handoff row in the chat.</li></ul> |
| wf7-routine-editor-dark | wf7-routine-editor-dark | <ul><li>The 560 px dialog over the 60% scrim, with On and the close button.</li><li>The day chips, time chips and Add time; the zone line; Stop after.</li><li>Recent runs with the Done row and its length; the footer.</li></ul> |
| wf8-bot-settings-dark | wf8-bot-settings-dark | <ul><li>The chip with the state.</li><li>About this bot with the notes preview.</li><li>Delete bot (red, no pill) and Pause bot; Cancel and Save changes.</li></ul> |
| wf9-first-run-dark | wf9-first-run-dark | <ul><li>Title and intro.</li><li>Two cards with the 36 px icon tiles and versions; "Signed in as" with the plan.</li><li>Create your first bot and the line under it.</li><li>The rail with only the dimmed Team room.</li></ul> |
| wf10-settings-general-dark | wf10-settings-general-dark | <ul><li>The 220 px nav, with General selected.</li><li>The Appearance card with Dark, Light, System.</li><li>The provider cards under it (a 1E addition).</li></ul> |
| wf10-settings-bots-dark | wf10-settings-bots-dark | The Bots asking each other card with the 120 × 32 stepper at 5. |
| wf11-full-auto-dark, wf11-delete-bot-dark, wf11-model-picker-dark | wf11-chat-states-dark (Dialogs, the picker) | <ul><li>320 px dialogs, padding 20, the buttons right-aligned.</li><li>Delete bot's red text and icon.</li><li>The picker's 36 px rows, the check, the caption.</li></ul> |

For each difference that isn't content, fix the UI code. Then rerun the script, and compare again until every row matches.

Write `docs/screenshots/phase1/README.md` as the record. It needs three parts:
- The run's date and app version.
- The table above, with a Result column. Each row is either "Matches", or "Matches after <fix>" with the fix's commit.
- The uncovered-states table from this task.

- [ ] **Step 4: Run the tests and checks after the fixes**

Run: `npm test`, `npm run typecheck`, `npm run lint` and `npm run build`
Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add packages/desktop/scripts/screenshots.ts docs/screenshots packages/ui
git commit -m "Check every Phase 1 screen against its wireframe with real bots

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 16: The Phase 1 demo

**Files:**
- Create: `docs/superpowers/demos/tjommie-phase1-demo.md` (the record of the run)

**Interfaces:**
- Consumes: the installed app from Task 14 and the screenshot record from Task 15.
- Produces: the Phase 1 demo from spec §12.2, run once by hand on the installed app. The record lists every item as Pass or Fail, with notes; each Fail becomes an item for a follow-up plan.

Some items need a person at the PC:
- SmartScreen;
- the Windows admin prompt for Codex's sandbox;
- signing in to a site by hand;
- the bot's own Chrome window.

So the person runs this, with an agent at hand to read logs (`%LOCALAPPDATA%\Tjommie\logs`).

- [ ] **Step 1: Run the demo**

Use the installed Tjommie (Task 14 Step 5), signed in to both providers. For each item, do the steps and compare with the expected result:

| # | Spec item | Steps | Expected |
|---|---|---|---|
| 1 | The installer installs | Install from `dist-desktop\installer` (Task 14 Step 5) | It installs without admin rights, opens, and sits in the Start menu and the tray |
| 2 | Create a Claude bot and a Codex bot, one from a Suggestion | New bot:<ul><li>"Job hunter" from Suggestions (Claude), Create bot.</li><li>From scratch, "Writer" on Codex (its default model), Own folder, Create bot.</li><li>If Writer shows Needs setup: Settings, then Set up Codex's sandbox, then answer the Windows prompt.</li></ul> | Both chats open. Writer runs once the sandbox is set up |
| 3 | Chat with both; steer a busy bot; Stop; Always allow | <ul><li>Ask each bot a question.</li><li>Give Job hunter a long task, and while it works, send "Also add a one-line summary at the end".</li><li>Give it another long task, and press Stop.</li><li>Ask Job hunter to run `node --version`. A card appears: choose Always allow. Ask again.</li></ul> | <ul><li>Both reply.</li><li>The follow-up changes the running answer.</li><li>Stop ends it with "Stopped.".</li><li>The second `node --version` runs without a card.</li></ul> |
| 4 | The Claude bot asks the Codex bot; rows in both chats; the exchange in the Team room; join from the exchange view | <ul><li>Bot settings for Job hunter: Can message Writer on, then Save changes.</li><li>Ask Job hunter to "ask Writer for a one-line summary of a Full Stack role, using message_bot".</li><li>Open the handoff row.</li><li>Reply from the exchange view.</li></ul> | <ul><li>Rows in both chats.</li><li>The Team room shows both posts.</li><li>The exchange view shows the request and reply.</li><li>The reply posts to the room with both bots mentioned, and they answer there.</li></ul> |
| 5 | A bot uses its own Chrome window; a site you signed in to by hand stays signed in | <ul><li>Job hunter has the browser on. Ask it to open a site that needs signing in (for example github.com).</li><li>Show window, and sign in by hand.</li><li>Ask the bot to open the site again.</li></ul> | A separate Chrome window. After signing in once, the bot finds itself signed in |
| 6 | A routine runs on schedule with the window closed | Add a routine to Writer due in 3 minutes, close the window (Tjommie stays in the tray), and wait | The run happens. Reopened, Writer's chat shows it under "Routine: <name>", and the editor lists it under Recent runs |
| 7 | A bot proposes a routine from chat, and you approve it | Ask Job hunter to "propose a routine: every weekday at 8:00, check pnet.co.za for new roles" | A Waiting for you card appears; Add routine puts it in the panel's Routines |
| 8 | Screenshots match WF1 to WF3, apart from later phases' parts | Task 15's record | Every row reads Matches |

- [ ] **Step 2: Record the run**

Create `docs/superpowers/demos/tjommie-phase1-demo.md` with:
- the date, the app version, and both CLIs' versions (from Settings);
- the table above, with a Result column (Pass, or Fail with what happened);
- a list of the follow-ups each Fail needs.

- [ ] **Step 3: Commit**

```bash
git add docs/superpowers/demos/tjommie-phase1-demo.md
git commit -m "Record the Phase 1 demo on the installed app

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Not measured yet

No code in this plan was compiled or run while writing it. Each task's typecheck, lint and tests are the first check. These points are the likeliest to need a change on first contact; the implementer settles each and records any change in that task's commit:
- **electron-builder 26:** the config keys and the `build()` call (`targets`, `directories`, `extraResources` with a filter, `npmRebuild`, `nsis`, `publish: null`) come from its documented API, not from a run. The first `npm run package` names anything renamed.
- **rolldown:**
  - That it bundles `ws` (CommonJS) into the daemon's ESM output.
  - That leaving out `bufferutil` and `utf-8-validate` is enough. If the bundle fails at startup, mark `ws` external and copy it beside the daemon, like Playwright MCP.
- **Playwright and the packaged exe:** that `_electron.launch` drives it. It relies on electron-builder not turning off Electron's inspector fuse, which it doesn't by default.
- **Base UI:**
  - That `Radio.Root` inside `RadioGroup` exposes `role="radio"` with the label text as its name, as the real tests expect.
  - That `Popover.Trigger` and `Menu.Trigger` take a plain `className`.
- **The time input:** that Chromium's fires `change` only once a full time is entered. If it fires per field, add the time on blur instead.
- **The zone name:** `zoneLine`'s expected "South Africa Standard Time" is ICU's en-US long name for Africa/Johannesburg. Another ICU version may word it differently; the test then takes whatever `Intl` returns.
- **Screens against the wireframes:** exact spacing is settled in Task 15, against the frames.
