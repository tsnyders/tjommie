# Screenshot shot list for the README

The README uses the pictures listed here, under `docs/screens/`. None of them exist yet. This file says what to capture for each one, on what data, and how.

## Rules

1. **Invented data only.** Every agent, room, routine, account and device in a picture is made up for it. No real account, project, folder or conversation appears.
2. **The window is 1000 by 700 logical pixels.** Never larger than 1024 by 768, so the same pictures can be retaken on a hosted runner's display. The one exception is the two SmartScreen pictures, which are cropped to a Windows dialog.
3. **Dark is the default.** A fresh install opens in dark, so every picture is taken in dark. The only light picture is `agents-light.png`. No other picture needs a light twin.
4. **Only the app's own window.** Pictures come from the window itself, never from the screen, so nothing else on the desktop can get in.
5. **Check every picture by eye before it is committed.** Open each PNG at full size and look at every part of it for a username, a folder path, a computer name, an email address, a port or IP address, a token or a pairing code. The list under "What to look for" says where these hide. Retake the picture if you find one. Do not blur or paint over it.

## How the pictures are captured

The desktop e2e test `e2e_screens_dark_light` (`tjommie-rs/crates/desktop/tests/e2e_screens_dark_light.rs`) already does most of this. It starts the e2e build of the app on a temporary data root, with the daemon beside it. It clicks through every screen and Settings section. For each one it calls `picture()`, which finds the app's own window and draws it with `PrintWindow` and the `PW_RENDERFULLCONTENT` flag. It saves a PNG under `tjommie-rs/target/` (build output, not tracked). It then switches Appearance to Light and does the tour again.

The README pictures use the same pieces, with three differences:

- **A new test, not that one.** A new e2e test (for example `e2e_readme_shots.rs`) holds one step per picture in the tables below. It reuses `picture()`, the driver (`Client`: `click`, `key`, `text`, `wait`, `snapshot`, `resize`) and the helpers in `tests/common/mod.rs` (`launch`, `make_agent`, `tab_to`). It writes to `tjommie-rs/target/readme-shots/`. A person copies the pictures that pass the eye check into `docs/screens/`.
- **The size is set, not default.** The existing screens test never resizes, so its pictures are the app's default 1440 by 900. The new test calls `app.resize(1000., 700.)` right after launch, as `make_agent` already does. Take the pictures with the display scale at 100%, so the PNG is 1000 by 700 pixels. At 125% or 150% the PNG is larger, and the window rectangle may carry a few pixels of invisible frame. Look at the edges of the first picture and crop the frame off if there is one.
- **A fixtures daemon.** Build the daemon with the test-only `provider-fixtures` feature: `cargo build -p daemon --bin tjommie-daemon --features provider-fixtures --locked`, inside `tjommie-rs`. The test points `TJOMMIE_DAEMON_EXE` at it and `TJOMMIE_PROVIDER_FIXTURES` at a folder of scripted conversations. No real Claude or Codex starts, and no account is touched. With the switch on, both providers read as signed in as `fixtures@example.com`, and the model lists are canned. The scripts are one JSON file per agent, named after the agent, for example `Forge.json`. The format is documented at the top of `tjommie-rs/crates/daemon/src/provider_fixtures.rs`. It supports streamed text, tool lines, approval cards, browser tool calls, token usage and usage limits.

The build is the e2e identity (`TJOMMIE_IDENTITY=e2e`). It has no update feed, never touches an installed Tjommie, and uses a temporary data root.

### What to look for

Every full-window picture has these, so check them first:

- **The host button at the foot of the sidebar.** On This PC its face is the first letter of the Windows account's name. Take the pictures under a Windows account with an invented name (for example `Demo`), or on a clean test machine. If the letter is anything else, retake.
- **The computer's name.** The daemon names this PC after the machine's host name. It can show in Settings, Devices, in About, and in Diagnostics. Take the pictures on a machine named for the demo (for example `DEMO-PC`), or keep those screens out of the picture.
- **Folder paths.** A temporary data root lives under the user's profile (`%TEMP%`), and some screens show paths: the agent's folder, Files, About, Diagnostics. Create a folder such as `C:\demo\sample-project` first and type that as an agent's folder. Settings screens that print a real path are not used.
- **The window caption.** The e2e build calls itself "Tjommie E2E". If the title shows anywhere in a picture, retake it with a build named "Tjommie" (the production product name) or crop the caption off.
- **Addresses and codes.** The browser's address bar shows a loopback address and a port, which is fine. A pairing QR code and its text are live for ten minutes and may carry this PC's network address. Do not capture a shown code (see `pair-dialog-dark.png`).
- **Real accounts and projects.** The fixtures daemon keeps the account to `fixtures@example.com`. If any other address, account name, plan or project name appears, the picture came from the wrong daemon or root. Stop and retake it.
- **The Logins window.** It is kept out of screenshots on purpose, so it can't be captured. Never try to capture it.

## Invented data

Use these names everywhere, so the pictures agree with each other.

| Thing | Value |
|---|---|
| Agents | Forge (Claude, Own folder), Scout (Codex, Always ask), Atlas (Claude, Always ask), Quill (Claude, Own folder) |
| Account | `fixtures@example.com`, from the fixtures daemon |
| Rooms | Everyone, and Release crew (Forge, Scout, Atlas) |
| Routines | Forge: Morning summary, weekdays at 08:00. Quill: Weekly cleanup, Fridays at 16:00 |
| Folder | `C:\demo\sample-project` (create it first) |
| Browser page | a loopback page served by the test (the `common` module has a page server), titled Example Orders |
| Usage | turns on several days of this week, with `usage` events: models `sonnet` and `haiku`, between 800 and 20,000 input tokens, an output of 100 to 2,000 |

## The pictures

All are 1000 by 700 and dark unless the notes say otherwise. "Fixture" means the scripted conversation for that agent.

### Hero and agents

| File | Screen and state | Fixture data | Notes |
|---|---|---|---|
| `agents-dark.png` | Agents overview, the four agents as cards. Forge shows a finished reply. Atlas has a waiting approval, so a **Needs you** group shows. Scout is working. | Forge: one short reply. Atlas: an `approval` event for `git status`, never answered. Scout: a slow `text` turn with `delay_ms`. Quill: idle. | The README hero and the right-hand picture of step 3. Wait for the cards' entrance motion to settle (500 ms, as the existing tour does). |
| `agents-light.png` | The same screen and state, with Appearance set to Light. | Same as above. | The Features table. The only light picture. |
| `new-agent-dark.png` | New agent form, filled in: name "Forge", a colour and a shape chosen, a line of instructions, Autonomy set to Always ask, Browser off. Nothing saved. | None. Type the name and the instructions through the driver. | Tab to a lower row before taking it if a row is below the fold, so the Autonomy row shows. |

### First run

| File | Screen and state | Fixture data | Notes |
|---|---|---|---|
| `setup-signed-in-dark.png` | The first-run **Set up Tjommie** screen with both cards signed in: "Signed in as fixtures@example.com", and **Create your first agent** below. | The fixtures daemon, with no agent made yet. | The screen shows until the first agent exists. Take it before `make_agent`. |
| `setup-signed-out-dark.png` | The same screen with both cards signed out: **Sign in to Claude** and **Sign in to Codex**. | The normal daemon (built without `provider-fixtures`) on a temporary root, with the Claude and Codex command-line tools on PATH and no Tjommie sign-in. | Open question: confirm the normal daemon on a temporary root reads nothing from the real Codex home. If it does, drop this picture and show only the signed-in one. The card text shows each tool's version, which is fine. |
| `install-smartscreen-1.png` | The Windows SmartScreen dialog "Windows protected your PC", before More info is clicked. | None. | Manual capture, not from the app: SmartScreen is a Windows window. Run the real `Tjommie-Setup-<version>.exe` from a clean folder. Crop to the dialog. Take it from a clean Windows account, and check the dialog for the file path or the account's name. |
| `install-smartscreen-2.png` | The same dialog after More info, showing the **Run anyway** button. | None. | Same notes. |

### Chat and approvals

These need the Chat lane's work (the chat screen, approval cards and file cards), which is on `rs3/chat` until it merges.

| File | Screen and state | Fixture data | Notes |
|---|---|---|---|
| `chat-dark.png` | Forge's chat. A user message, a tool line ("Reading a file", `src/orders.rs`), then a reply with a short Markdown list and a file card for `summary.md`. The composer is empty and its round buttons show. | Forge, one turn: a `tool` and `tool_done` pair, then `text` events with the list, then `usage`. | The file card needs a file shared into the chat. If the fixtures can't share one, leave the card out and say so in the caption. |
| `approval-dark.png` | Atlas's chat with the pending approval card: the command `git status`, the autonomy line, and **Approve**, **Deny** and **Always allow**. | Atlas: `approval` with `tool` "Bash", `summary` "Run git status", `detail` `{ "command": "git status" }` and `rule` `Bash(git status:*)`. The turn waits for the answer. | Take it while the card is waiting and not yet answered. |

### Rooms and exchanges

| File | Screen and state | Fixture data | Notes |
|---|---|---|---|
| `room-dark.png` | The Everyone room: three or four posts from the user and agents, one mentioning an agent by name, the composer under them. | Forge, Scout and Atlas answer a mention each, in short plain text. | Use the mention flow of `e2e_room_mention`. |
| `exchange-dark.png` | A room with the exchange panel open beside it: who asked whom, and the box that replies in Everyone. | Forge asks Scout a question and Scout answers, through the agents' "Can message" setting. | Needs the Chat lane's exchange view (`rs3/chat`). |

### Routines

| File | Screen and state | Fixture data | Notes |
|---|---|---|---|
| `routines-dark.png` | Routines: each agent with its routines, and **Add routine**. | Morning summary (Forge) and Weekly cleanup (Quill), made through the editor. | None. |
| `routine-editor-dark.png` | The routine editor over Routines, for Morning summary: on, the name, what to do, the days and times, the time zone, Stop after, and Recent runs. | One routine, with a recorded run if the fixtures can run it. | If Recent runs is empty it says "No runs yet.", which is fine. The time zone line may show the machine's zone: check it. |

### The agent's browser and Logins

| File | Screen and state | Fixture data | Notes |
|---|---|---|---|
| `browser-dark.png` | Forge's chat with the browser panel open beside it: the tab row, the address bar with the loopback address, and the page. | Forge made with the browser on. A `browser` event that navigates to the test's page. | The panel's page is a WebView2 window, and `PrintWindow` may draw it black. If it does, use the panel's snapshot variant (it slides a still picture of the page), or take the page area from a window capture and say so here. Check the page area by eye. |
| `logins-menu-dark.png` | The browser panel with the logins menu open from the key button: **Fill a saved login**, **Saved logins**, **Manage logins**. | The fixtures daemon's vault reads as locked, so the menu shows its unlocked-vault items only if a login is saved. | Open question: a saved login needs a real entry in Windows Credential Manager, entered in the Logins window. If that is not wanted, take the menu with the vault locked or empty. Never put a real login in the vault. If a test login is made, delete it afterwards and check Credential Manager is back to how it was. |

### Pairing the Android companion

| File | Screen and state | Fixture data | Notes |
|---|---|---|---|
| `devices-dark.png` | Settings, Devices, with **Pair a device**, the empty list ("No devices paired yet."), and the relay row. | None. | The relay row may show the relay's address. It is public, but look at it. Check the PC name. |
| `pair-dialog-dark.png` | The **Pair a device** dialog before the code is shown: the explanation (works once, ten minutes, same Wi-Fi or mobile data) and the **Show pairing code** button. | None. | Do not press **Show pairing code**. A shown QR code and its text are live and may carry the PC's address. |

### Features table

| File | Screen and state | Fixture data | Notes |
|---|---|---|---|
| `accounts-dark.png` | Settings, Accounts, with the Claude Code and Codex accounts as cards. If it can be done safely, an API-key account "Team key" with a $20 cap and $5 spent. | `fixtures@example.com`, and a fake key `sk-demo-not-a-real-key` if an API-key account is added. | Open question: adding an API-key account stores the key in Windows Credential Manager, under the test's namespace. If it is added, remove it afterwards and check Credential Manager. If not, show the subscription cards only and change the table's caption. |
| `usage-dark.png` | Settings, Usage, range "This week": the limit cards, tokens, the day bars, and the breakdown by agent. | `usage` events spread over the week for the four agents. | The `limit` event can fill a limit card with a made-up reading. Dates are real, so retake it if the week rolls over. |

## After the capture

1. Run every picture through the eye check in rule 5, at full size.
2. Copy the ones that pass to `docs/screens/`, with the file names above.
3. Scan the PNG files, not only the text, for private strings before committing: search the raw bytes of each file for the username, the computer name and the profile path (a metadata chunk can carry them even when the picture looks clean). Prove the scan works first by planting a known string in a test copy and finding it.
4. Open the README on the branch and check that every image link resolves.
