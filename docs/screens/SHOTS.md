# Screenshot shot list for the README

The README uses the pictures listed here, under `docs/screens/`. None of them exist yet. This file says what to capture for each one, on what data, and how.

## Rules

1. **Invented data only.** Every agent, room, routine, project and account in a picture is made up for it. No real account, project, folder or conversation appears.
2. **The window is 1000 by 700 logical pixels.** Never larger than 1024 by 768, which is the display of a GitHub hosted runner.
3. **Dark is the default.** A fresh install opens in dark, so every picture is taken in dark. The only light picture is `agents-light.png`. No other picture needs a light twin.
4. **Only the app's own window.** Pictures come from the window itself, never from the screen, so nothing else on the desktop can get in. There are no pictures of Windows dialogs: the README describes SmartScreen in text.
5. **Capture on a GitHub hosted runner, never on the owner's PC.** The runner's account name and machine name are generic, not the owner's, so the sidebar's initial and the host name can't give the owner away. No real account, and no saved credential, exists on a runner.
6. **Check every picture by eye before it is committed.** Open each PNG at full size and look at every part of it for a username, a folder path, a computer name, an email address, a port or IP address, a token or a pairing code. The list under "What to look for" says where these hide. Retake the picture if you find one. Do not blur or paint over it.

## How the pictures are captured

### The capture job

A manually started workflow does all the capturing. This section is its specification, and the workflow file is `.github/workflows/readme-shots.yml`.

- **Trigger:** `workflow_dispatch` only. It never runs on a push, a pull request or a tag. Start it with `gh workflow run readme-shots.yml --ref <branch>`.
- **Permissions:** `contents: read`. It uses no secrets, and it never pushes, commits or comments.
- **Runner:** `windows-latest`, the same image as the e2e job in `ci.yml`. Its display is 1024 by 768 at 100% scale, so a 1000 by 700 window fits and the PNGs are 1000 by 700 pixels.
- **Steps, in order:**
  1. Check out the branch (`persist-credentials: false`), with the same pinned actions as `ci.yml`.
  2. Install the Rust toolchain from inside `tjommie-rs` (`rustup toolchain install`).
  3. Check that neither `claude` nor `codex` is on PATH, and fail if one is. The "tools not installed" picture depends on it.
  4. Build the **normal** daemon (`cargo build -p daemon --bin tjommie-daemon --locked`) and copy it to a folder of its own, for example `target\readme-shots\plain\tjommie-daemon.exe`.
  5. Build the **fixtures** daemon (`cargo build -p daemon --bin tjommie-daemon --features provider-fixtures --locked`). It writes `target\debug\tjommie-daemon.exe`, which is why step 4 copies the normal one away first.
  6. Run the capture test with `TJOMMIE_IDENTITY=e2e`: `cargo test -p desktop --features test-driver --test e2e_readme_shots --locked -- --ignored --nocapture`. The test is ignored in a normal run, because `ci.yml` runs every `e2e_*` test and this one takes pictures instead of checking anything. It calls `app.resize(1000., 700.)` right after each launch and takes one picture per row in the tables below.
  7. Upload `tjommie-rs/target/readme-shots/*.png` with `actions/upload-artifact`, named `readme-shots`, `if-no-files-found: error`, a short retention (7 days).
- **After the run:** a person downloads the artifact (`gh run download`), checks every picture by eye (rule 6), copies the ones that pass into `docs/screens/`, and commits them in an ordinary commit.

### The capture test

The desktop e2e test `e2e_screens_dark_light` (`tjommie-rs/crates/desktop/tests/e2e_screens_dark_light.rs`) already does most of this. It starts the e2e build of the app on a temporary data root, with the daemon beside it. It clicks through every screen and Settings section, and for each one it calls `picture()`, which finds the app's own window and draws it with `PrintWindow` and the `PW_RENDERFULLCONTENT` flag. It then switches Appearance to Light and does the tour again.

The new test, `e2e_readme_shots.rs`, reuses `picture()`, the driver (`Client`: `click`, `key`, `text`, `wait`, `snapshot`, `resize`, `answer_path_prompt`) and the helpers in `tests/common/mod.rs` (`launch`, `make_agent`, `tab_to`). It differs from the screens test in three ways:

- **The size is set.** The screens test never resizes, so its pictures are the app's default 1440 by 900. The new test resizes to 1000 by 700, as `make_agent` already does. Look at the edges of the first picture: if the window rectangle carries a few pixels of invisible frame, crop it off.
- **A fixtures daemon.** The test points `TJOMMIE_DAEMON_EXE` at the fixtures daemon and `TJOMMIE_PROVIDER_FIXTURES` at a folder of scripted conversations, one JSON file per agent, named after the agent (for example `Forge.json`). No real Claude or Codex starts, and no account is touched. With the switch on, both providers read as signed in as `fixtures@example.com`, and the model lists are canned. The format is documented at the top of `tjommie-rs/crates/daemon/src/provider_fixtures.rs`. It supports streamed text, tool lines, approval cards, browser tool calls, token usage and usage limits.
- **One picture uses the normal daemon.** `setup-not-installed-dark.png` is taken with the normal daemon (the copy from step 4), on a runner with no Claude Code or Codex installed.

The build is the e2e identity. It has no update feed, never touches an installed Tjommie, and uses a temporary data root.

### What to look for

Every full-window picture has these, so check them first:

- **The host button at the foot of the sidebar.** On This PC its face is the first letter of the Windows account's name. On a runner that is the runner's own account, and it must not be anything that points to a person. If it does, stop and find out why.
- **The computer's name.** The daemon names this PC after the machine's host name. It can show in Settings, Devices, in About, and in Diagnostics. On a runner it is a generic name. Look at it anyway.
- **Folder paths.** The temporary data root, and any folder the test makes, are under the runner's profile or temp folder. Some screens print paths: the agent's folder, Files, About, Diagnostics. A path that shows is not wrong on a runner, but it adds nothing: retake without it if you can.
- **The window caption.** The e2e build calls itself "Tjommie E2E". If the title shows anywhere in a picture, retake it with a build named "Tjommie" or crop the caption off.
- **Addresses and codes.** The browser's address bar shows a loopback address and a port, which is fine. A pairing QR code and its text are live for ten minutes. Do not capture a shown code (see `pair-dialog-dark.png`).
- **The Logins window.** It is kept out of screenshots on purpose, so it can't be captured. Never try to capture it.
- **Real accounts and projects.** The fixtures daemon keeps the account to `fixtures@example.com`. If any other address, account name, plan or project name appears, the picture came from the wrong daemon or root. Stop and retake it.

## Invented data

Use these names everywhere, so the pictures agree with each other.

| Thing | Value |
|---|---|
| Agents | Forge (Claude, Own folder), Scout (Codex, Always ask), Atlas (Claude, Always ask), Quill (Claude, Own folder) |
| Account | `fixtures@example.com`, from the fixtures daemon |
| Rooms | Everyone, and Release crew (Forge, Scout, Atlas) |
| Routines | Forge: Morning summary, weekdays at 08:00. Quill: Weekly cleanup, Fridays at 16:00 |
| Project | Team "Orders app" on a small Git repository the test creates (`git init`, one commit, in a folder called `sample-project`). Forge is the Developer and Scout the Reviewer. Tasks: "Add order search" and "Fix date format", saved and not started |
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
| `setup-not-installed-dark.png` | The first-run **Set up Tjommie** screen with both cards saying the tool is not installed: "Install Claude Code from claude.com/claude-code, then check again." and Codex's `npm install -g @openai/codex` line, each with **Check again**. | The normal daemon, with no Claude Code or Codex on the runner, and no agent made yet. | A real state, produced with no provider at all. Check the cards read "Not installed" and not "Checking". |
| `setup-signed-in-dark.png` | The same screen with both cards signed in: "Signed in as fixtures@example.com", and **Create your first agent** below. | The fixtures daemon, with no agent made yet. | The screen shows until the first agent exists. Take it before `make_agent`. |

**Dropped:** `setup-signed-out-dark.png`. A signed-out card needs the real Claude Code or Codex installed on the machine, and installing a real provider tool on the runner only for a picture is not worth it. The "not installed" picture replaces it.

### Chat and approvals

These need the Chat lane's work (the chat screen, approval cards and file cards).

| File | Screen and state | Fixture data | Notes |
|---|---|---|---|
| `chat-dark.png` | Forge's chat. A user message, a tool line ("Reading a file", `src/orders.rs`), then a reply with a short Markdown list and a file card for `summary.md`. The composer is empty and its round buttons show. | Forge, one turn: a `tool` and `tool_done` pair, then `text` events with the list, then `usage`. | The file card needs a file shared into the chat. If the fixtures can't share one, leave the card out and say so in the caption. |
| `approval-dark.png` | Atlas's chat with the pending approval card: the command `git status`, the autonomy line, and **Approve**, **Deny** and **Always allow**. | Atlas: `approval` with `tool` "Bash", `summary` "Run git status", `detail` `{ "command": "git status" }` and `rule` `Bash(git status:*)`. The turn waits for the answer. | Take it while the card is waiting and not yet answered. |

### Rooms and exchanges

| File | Screen and state | Fixture data | Notes |
|---|---|---|---|
| `room-dark.png` | The Everyone room: three or four posts from the user and agents, one mentioning an agent by name, the composer under them. | Forge, Scout and Atlas answer a mention each, in short plain text. | Use the mention flow of `e2e_room_mention`. |
| `exchange-dark.png` | A room with the exchange panel open beside it: who asked whom, and the box that replies in Everyone. | Forge asks Scout a question and Scout answers, through the agents' "Can message" setting. | Needs the Chat lane's exchange view. |

### Projects

| File | Screen and state | Fixture data | Notes |
|---|---|---|---|
| `projects-dark.png` | Projects, with the team "Orders app" chosen: its repository and branches, its agents with their roles, **New task**, **Open team room**, and the two saved tasks. | The test makes the Git repository, registers it by answering the folder prompt (`answer_path_prompt`), and creates the team and tasks through the editors. | If the fixtures daemon refuses to register a repository, take the Projects empty state ("Create a project team...") and the **New project team** dialog instead, and say so here. |
| `project-task-dark.png` | A saved task's detail: its state ("Not started"), instructions, assigned agents, **Start task**, **Run review**, **Run checks**. | The task "Add order search". | Nothing is started, so there is no diff and no worktree. Do not press **Start task**. |

### Routines

| File | Screen and state | Fixture data | Notes |
|---|---|---|---|
| `routines-dark.png` | Routines: each agent with its routines, and **Add routine**. | Morning summary (Forge) and Weekly cleanup (Quill), made through the editor. | None. |
| `routine-editor-dark.png` | The routine editor over Routines, for Morning summary: on, the name, what to do, the days and times, the time zone, Stop after, and Recent runs. | One routine, with a recorded run if the fixtures can run it. | If Recent runs is empty it says "No runs yet.", which is fine. The time zone line shows the runner's zone: check it. |

### The agent's browser and Logins

| File | Screen and state | Fixture data | Notes |
|---|---|---|---|
| `browser-dark.png` | Forge's chat with the browser panel open beside it: the tab row, the address bar with the loopback address, and the page. | Forge made with the browser on. A `browser` event that navigates to the test's page. | The panel's page is a WebView2 window, and `PrintWindow` may draw it black. If it does, use the panel's snapshot variant (it slides a still picture of the page) and say so here. Check the page area by eye. |
| `logins-menu-dark.png` | The browser panel with the logins menu open from the key button: "Logins for 127.0.0.1", **Fill a saved login**, **Saved logins**, **Manage logins**, and the note that this PC confirms the exact HTTPS site. | None. The menu lists the same three items whatever the vault holds, so no login is saved and nothing touches Credential Manager. | **Fill a saved login** may carry a hint such as "Take control first", depending on the page and who holds it. Take the picture in the state that reads best, and describe that state in the caption. Never put a login in the vault. |

### Pairing the Android companion

| File | Screen and state | Fixture data | Notes |
|---|---|---|---|
| `devices-dark.png` | Settings, Devices, with **Pair a device**, the empty list ("No devices paired yet."), and the relay row. | None. | The relay row may show the relay's address. It is public, but look at it. Check the PC name. |
| `pair-dialog-dark.png` | The **Pair a device** dialog before the code is shown: the explanation (works once, ten minutes, same Wi-Fi or mobile data) and the **Show pairing code** button. | None. | Do not press **Show pairing code**. A shown QR code and its text are live and may carry the PC's address. |

### Features table

| File | Screen and state | Fixture data | Notes |
|---|---|---|---|
| `accounts-dark.png` | Settings, Accounts, with the Claude Code and Codex subscription accounts as cards, each signed in as `fixtures@example.com`. | The fixtures daemon. | **No API-key account:** adding one stores the key in Windows Credential Manager, which a hosted runner can't reach reliably. So the picture shows no spending cap, and the README's table caption says only "Accounts". |
| `usage-dark.png` | Settings, Usage, range "This week": the limit cards, tokens, the day bars, and the breakdown by agent. | `usage` events spread over the week for the four agents. | The `limit` event can fill a limit card with a made-up reading. Dates are real, so retake it if the week rolls over. |

Dictation and Files are Features bullets with no picture.

## After the capture

1. Run every picture through the eye check in rule 6, at full size.
2. Copy the ones that pass to `docs/screens/`, with the file names above.
3. Scan the PNG files, not only the text, for private strings before committing: search the raw bytes of each file for the owner's username, the owner's computer name and profile path (a metadata chunk can carry them even when the picture looks clean). A runner has none of them, so a hit means something is wrong. Prove the scan works first by planting a known string in a test copy and finding it.
4. Open the README on the branch and check that every image link resolves.
