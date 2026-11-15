# Phase 3 owner checklist

The checks that need real input or the owner's PC. T8.5 adds one line per D row; the lanes add theirs here as they
finish. Run on the dev identity (`TJOMMIE_IDENTITY=dev`, Tjommie RS), beside the installed Tjommie, never on top of it.

## M6 desktop services (the Services lane)

### Tray
- [ ] Right-click the tray icon, then click on the desktop (or any other window): **the menu must close.** (It is owned by a
  hidden top-level window so Windows can bring it to the foreground; a unit test checks the window, only a real click
  shows the menu closing.)
- [ ] Right-click the tray icon, press Esc: the menu closes.
- [ ] Left-click the icon: the window shows and takes focus. The menu's Open does the same; Quit quits.
- [ ] Restart Explorer (Task Manager > Windows Explorer > Restart): the icon comes back.
- [ ] Close the window the first time ever on a fresh profile: the "Tjommie is still running" toast appears once, and not again.

### Notifications
- [ ] A routine fails while the window is hidden: a toast appears, and clicking it opens the agent's chat.
- [ ] The same, with the window visible and focused: no toast, and the notice is still acknowledged.
- [ ] Turn notifications off for the app in Windows Settings: nothing appears, and the notice shows again after a restart.

### Start at login (dev builds must never touch it)
- [ ] After a dev run, `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` has no new entry, and `%APPDATA%\Tjommie` has no
  new `tray-hint-shown` or `login-item-set` (the installed Tjommie's own are untouched).

### Updater (needs M8's installer)
- [ ] An update installs after Restart to update, and the new app starts in the tray (`--updated`).
- [ ] A forced daemon stop at quit: "Update not installed", the window returns, the daemon is back.

### Dictation
- [ ] Dictate past 60 seconds: the Windows microphone indicator goes off when the transcript appears.
- [ ] Switch to another window while dictating: the recording is cancelled and the indicator goes off.
- [ ] Windows Settings > Privacy > Microphone > "Let desktop apps access your microphone" off: the status says "Microphone
  access is off for desktop apps in Windows Settings".
- [ ] Download the speech model, dictate a sentence, remove the model.

## T3.7 browser panel (the Browser UI lane)

Needs the store wired (T1.4), so a chat has a bot with its browser on. Quiet PC, nothing else building.

### Motion, by eye (owner step F again)
- [ ] Open and close the panel with the mouse (the chat header's control, or Hide) ten times: the page moves with the
  panel, no gap opens at its moving edge, the chat column never jumps, and it settles without a bounce (calm spring).
- [ ] Turn it around mid-flight (click again while it moves): it turns smoothly from where it is, no jump.
- [ ] Ctrl+B and Escape: open and shut at once, no frames in between.
- [ ] Load the PC (two Rust builds), then open the panel twice: the second opening may slide a still picture of the page
  (the fallback). It comes back live when the panel stops. A minute after the load ends, it is live again.
- [ ] Windows Animation effects off: every open and close is instant. Turn it back on.
- [ ] Note the display's refresh rate (Settings, Display, Advanced display). The fallback's 16.7 ms budget assumes 60 Hz
  or faster: below that (a 30 Hz television, a 48 Hz battery-saving panel) every opening after the first slides a still
  picture. That is expected there, not a fault.

### Alignment sweep (owner step A again; `owner-checks-scale.ps1` restores the scale)
- [ ] At 100%, 125% and 150% display scale, and at app zoom 80%, 100% and 125%: at rest, the page's edges are within 1 px
  of the panel's page frame; while it moves, the page stays clipped to the panel.
- [ ] Move the window to a monitor with another scale while the panel is open: the page follows with no restart.

### Keyboard and overlays
- [ ] Take control, click into the page, press Shift+Tab: the keyboard lands on Return to agent. Tab past the page's last
  control: it goes on to the next control.
- [ ] With the page focused, Ctrl+= zooms the app, Ctrl+B closes the panel. (Escape also closes the panel: see the report.)
- [ ] Open the logins menu over the page: a picture of the page stands in under the menu; closing the menu brings the live
  page back.
- [ ] A page `prompt()` (or `confirm()`) while you are in control: type in its answer field and press Escape. The dialog is
  cancelled and the panel stays open. With focus elsewhere, Escape closes the panel.

### The Logins window (needs the vault host wired; no real account: a made-up login on a local test page)
- [ ] Open a Logins window (Unlock Logins from the logins menu). Take a screenshot (Win+Shift+S), share the screen in Teams
  and record it in OBS: in each, the Logins window is black or missing while the rest of the app shows. If the window
  instead says "This PC can't keep this window out of screen capture", Windows refused to hide it: note the Windows
  version for the report (an entry window then takes no login; a confirmation still asks).
- [ ] With the app in front, a Logins window is modal: the main window ignores clicks until the Logins window closes
  (Cancel, Escape or its X), then it works again and comes to the front.
- [ ] In the Logins window, Ctrl+B and Escape leave the panel behind it alone; Escape closes the Logins window as Cancel.
- [ ] Close a Logins window with its X, then ask for another at once (Save this login): it opens straight away.

### Design rulings to confirm or overrule (review F9; each is the code as it stands)
- [ ] Who is in control: always the agent's avatar. The design shows it only while the agent drives, with a mouse pointer
  when you are in control, a smartphone when another device is, and a lock when private. (The smartphone and lock icons
  are baked; the mouse pointer is not.)
- [ ] The remote view has no live indicator ("Live, 58 fps", "Reconnecting", "No new frames") and no pulsing dot. Ported
  with T1.4's store wiring, when the remote view goes live.
- [ ] The remote view has no "Waiting for the page…" after 800 ms, and no 60% dim while the page is busy (the TypeScript
  had both). Same timing as the line above.
- [ ] The address row's back, forward and reload buttons are 36 px (the app's icon button); the design's are 32 px.
- [ ] The page frame's background is the raised surface; the design uses white. It shows while "Opening the browser…"
  shows, and in the remote picture's letterbox.
- [ ] Tabs have no favicons: the engine reports none.
- [ ] Danger red is the red agent colour: Aegis has no danger colour (M2 review F7).
- [ ] The Details state pill is the TypeScript's 22 px, 8 px padding, meta type; Aegis `.pill-working` is 24 px,
  `--space-10`, `--type-label`.
- [ ] The panel's width moves on the calm spring, as the plan says (T3.7); motion card 12 says ease-out over 397 ms for
  260 px. The content's fade matches the card. The plan wins unless you rule otherwise.


## M5 screens: the keyboard-only pass (T5.6, owner decision 4; the Screens lane)

One pass with the mouse put away, on the dev identity. `e2e_keyboard_navigation` already proves every control on each
screen and dialog is reached by Tab; this pass is for what a test can't judge: that the order reads naturally, that the
focus ring is always visible, and that nothing feels stuck.

### Getting around
- [ ] From launch, Tab reaches the sidebar: Agents, Rooms, Projects, Routines, Settings, then Recents, View all, the PC
  button and New agent, in that order. Up and Down also move between the sections and between Recents.
- [ ] Enter on a section opens it. On each screen, Tab then moves through the page top to bottom, left to right, with the
  ring visible on every stop. Shift+Tab goes back the same way.
- [ ] On a long screen (Settings, a project's task detail), Tab to a control below the fold: note whether the page
  scrolls to show it (GPUI does not scroll focus into view by itself; a stop off screen is a known gap to rule on).
- [ ] The PC button: Enter opens the PC list on its first row; Up and Down move; Enter picks; Escape closes it and the
  focus is back on the button.

### Doing things
- [ ] New agent: type a name, Tab through the form (Tab leaves Instructions; it never indents), Enter on Create agent.
- [ ] An agent card's More: Enter opens it, the arrows move, Enter on Agent settings opens them; Delete agent's question
  takes the focus, Escape closes it.
- [ ] A room: type `@` in the composer, Up and Down move through the agents, Enter or Tab puts the name in, Enter sends,
  Shift+Enter starts a new line. Escape closes the list and keeps what you typed.
- [ ] Every dialog (New room, Edit room, New project team, New task, a routine, Delete confirmations): the focus starts
  inside it, Tab stays inside it, Escape closes it and returns the focus to what opened it.
- [ ] Settings: every section from its row in the list; radio groups (Theme, Text size, Spacing, the Agents filter) are
  one Tab stop each, and the arrows change the choice.
- [ ] Projects: a task card takes the focus and Enter shows its detail, which takes the focus once; a refresh never
  moves the focus.

### Not in this pass (other lanes)
- Approving a card with Ctrl+Enter (Chat lane, T4.2), sending in an agent's chat (T4.3), and moving into and out of a
  bot's page (Browser lane, T3.5): their own checklist lines.
