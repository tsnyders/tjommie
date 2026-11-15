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

