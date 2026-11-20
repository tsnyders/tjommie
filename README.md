<p align="center">
  <img src="packages/desktop/assets/icon.svg" width="88" height="88" alt="Tjommie">
</p>

<h1 align="center">Tjommie</h1>

<p align="center">
  A desktop app for AI agents that chat with you, work on your PC,<br>
  talk to each other and run routines.
</p>

<p align="center">
  <a href="https://github.com/tsnyders/tjommie/releases/latest"><img src="https://img.shields.io/github/v/release/tsnyders/tjommie?label=Download&style=for-the-badge&color=b57a1f" alt="Download the latest release"></a>
</p>

<p align="center">
  <sub>Windows&nbsp;&nbsp;·&nbsp;&nbsp;Android companion&nbsp;&nbsp;·&nbsp;&nbsp;Free and open source</sub>
</p>

<p align="center">
  <a href="#install">Install</a>&nbsp;&nbsp;·&nbsp;&nbsp;<a href="#android-companion">Android</a>&nbsp;&nbsp;·&nbsp;&nbsp;<a href="#accounts-and-usage">Accounts</a>&nbsp;&nbsp;·&nbsp;&nbsp;<a href="#agent-browser">Browser</a>&nbsp;&nbsp;·&nbsp;&nbsp;<a href="#safety">Safety</a>&nbsp;&nbsp;·&nbsp;&nbsp;<a href="docs/DEVELOPMENT.md">Develop</a>
</p>

<br>

Tjommie runs your agents on your own computer, through the official Claude Code and Codex CLIs, signed in with your own accounts. An Android companion connects over Wi-Fi, or over mobile data through the relay, and receives push notifications.

## What it does

<table>
  <tr>
    <td width="50%" valign="top">
      <b>Agents on your PC</b><br>
      Each agent runs Claude Code or Codex in its own folder, with the model and the level of autonomy you choose. It asks before it changes anything, unless you let it work on its own.
    </td>
    <td width="50%" valign="top">
      <b>A team room</b><br>
      Agents message each other, share files and hand work back and forth. You can read every exchange.
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <b>Routines</b><br>
      Give an agent a schedule, such as weekdays at 9:00, and it runs the routine on its own. A routine that fails tells you.
    </td>
    <td width="50%" valign="top">
      <b>A browser for each agent</b><br>
      Turn on Browser and the agent gets its own browser, with its own sign-ins, kept apart from yours. Take control whenever you want.
    </td>
  </tr>
  <tr>
    <td width="50%" valign="top">
      <b>Your phone</b><br>
      Pair the Android app to follow your agents and chat with them away from your PC. It gets a push notification when an agent needs you.
    </td>
    <td width="50%" valign="top">
      <b>Your accounts</b><br>
      Use your Claude and Codex subscriptions or API keys. Tjommie tracks usage per account, and every API key gets a spending cap.
    </td>
  </tr>
</table>

## Install

1. Download `Tjommie-Setup-<version>.exe` from the [latest release](https://github.com/tsnyders/tjommie/releases/latest).
2. Run it. It installs Tjommie for your Windows user, without admin rights, and starts it.
3. The installer isn't signed yet, so Windows SmartScreen asks once. Choose <kbd>More info</kbd>, then <kbd>Run anyway</kbd>.
4. Install the Claude Code CLI, the Codex CLI or both, then sign in from <kbd>Settings</kbd> → <kbd>Accounts</kbd>.

Uninstalling keeps your agents and their data in `%LOCALAPPDATA%\Tjommie`.

## Android companion

The [latest release](https://github.com/tsnyders/tjommie/releases/latest) also has a release-signed Android APK.

1. Install the APK on your phone.
2. On the PC, open <kbd>Settings</kbd> → <kbd>Devices</kbd> → <kbd>Pair a device</kbd>.
3. Scan or paste the code in the Android app. The code expires after ten minutes and works once.

The PC must be awake. On the same network the phone connects directly over TCP port 47101, and elsewhere through the relay; Windows may ask to allow the app on the private network. Every paired device has full owner authority, so revoke a lost phone from Devices. See [`packages/mobile/README.md`](packages/mobile/README.md) for signing, build requirements and current limits.

<details>
<summary>On a narrow screen</summary>

Below 640 px, the Quiet pill replaces the desktop rail with Agents, Team and Settings navigation. Details, exchanges and members become full-screen sheets, and forms and account dialogs fit the narrow layout. Narrowing the desktop window still provides its native browser. The Android companion connects through the encrypted pairing flow described above.

</details>

## Accounts and usage

- <kbd>Settings</kbd> → <kbd>Accounts</kbd> keeps separate Claude Code and Codex accounts, and each agent is pinned to one of them.
- A subscription signs in through your browser. An API key is kept in Windows Credential Manager, never in Tjommie's database.
- An API-key account needs a spending cap.
- <kbd>Settings</kbd> → <kbd>Usage</kbd> shows today, this week, this month and this year, with costs and allowance windows.

<details>
<summary>How accounts, keys and usage work</summary>

Settings → Accounts manages separate Claude Code and Codex accounts. Existing accounts keep their IDs and sign-in folders when upgraded. Each agent has an account pin; changing the provider's default affects new agents, while changing an agent's pin starts a fresh provider session and keeps its chat.

Subscription sign-in is an explicit action that opens the provider's page in the system browser. Claude uses the browser's loopback callback without opening a terminal; manual-code entry is not offered by this integration. API keys are stored in Windows Credential Manager, with only an opaque reference in the database. They are not returned in account status. Windows protects these entries for the current user; this is not protection against unrestricted code running as that same user.

Claude API-key work requires Claude Code 2.1.251 or later for subprocess environment scrubbing. A synthetic loopback API test with the installed CLI verified that its Bash subprocess did not inherit the API key. This does not isolate secrets from arbitrary code running under the same Windows account.

An API-key account needs a spending cap. The cap covers its total recorded API spending, with no automatic monthly reset. Prices and provider reports are local accounting inputs, not an invoice or a guaranteed billing ceiling. Unsupported Codex pricing prevents a paid turn; an unpriced completed turn blocks further API-key work rather than treating its cost as zero. A subscription can use the provider's explicitly selected API-key fallback when limited, then return to its pinned account when the limit resets. Tjommie does not silently rotate between subscription accounts.

Settings → Usage shows Today, This week, This month and This year (calendar year to date). A combined cost estimate gives the API-equivalent value of all recorded token usage, alongside separate API-key spending and subscription API value. It is not your subscription bill. Missing costs remain unknown; incomplete totals show a known minimum. Date ranges use the displayed time zone; the year chart groups tokens by month.

The provider cards show reported allowance windows, reset times, plan/account details, provider credits and extra-usage status where available. Credits are provider units, not dollars. Refresh asks for metadata only, without starting a model turn. Codex uses its app-server metadata RPC. Claude's selected-account reader uses the same fixed OAuth usage endpoint as Paseo; it is an unofficial metadata endpoint and can become unavailable. The backend does not expose, log, modify or refresh the CLI token. A failed reading retains and labels the last successful data. Removing an account requires moving its assigned agents first, disconnects it locally, and retains historical usage.

</details>

## Agent browser

Enable Browser for an agent, then choose Open browser in its chat to browse inside the right panel. The desktop app owns a live native browser view with a separate persistent profile for each agent. The person and the agent use the same tabs. Browser tools need the desktop app running, including when its window is hidden in the tray.

Take control before typing or clicking. Resume agent gives its browser tools control again. Returning to Details or hiding the panel releases manual control without closing the browser session. Website pages run in isolated native guest contents without the application's preload or Node access. Human browsing no longer uses JPEG polling, and manual input is not stored as chat messages.

<details>
<summary>More about the agent browser</summary>

Use the Details / Browser tabs to return to agent settings, routines and activity without losing the browser page or address draft. Drag the panel divider to change its width; the focused divider also accepts arrow keys. Expand browser fills the application content area, and Restore browser returns to the previous width.

Closing or disabling an agent's browser keeps its sign-ins. Deleting the agent clears its browser storage and cookies before deleting the agent. Windows may temporarily lock cleared profile files; startup retries removing those orphaned files. Camera, microphone, screen capture and device access are denied.

Old headless Chrome profiles are retained for rollback and are not imported into the native browser. Local synthetic-site checks establish cookie persistence, not compatibility with every third-party sign-in service. Google rejected sign-in in the previous automated panel during owner testing. Native rendering does not establish that Google accepts sign-in; owner-controlled real-site acceptance remains required. Media codecs, protected playback, passkeys and operating-system authentication also require site-specific validation.

The separate Google API connection is on hold while native website browsing is validated. OAuth API consent does not sign the browser into Google websites. No Google API connection is offered in the agent panel in this version.

Startup routine failures are retained until the desktop handles and acknowledges them. Reconnecting retries an unacknowledged notice. A desktop crash between showing a notification and acknowledging it can cause one replay after restart.

</details>

## Safety

On Windows, Codex agents at Own folder need Codex's elevated sandbox. Setting it up may show one Windows admin prompt. Until it is ready, those agents show Needs setup and do not run. Tjommie denies sandboxed Codex commands access to its sign-in folders, Claude agents' folders, logs, database and shared-files folder. The installed daemon token is passed through a pipe; a development token file is protected if present.

Codex agents can still read the rest of your user profile. A Codex agent at Own folder can read the folders of your other Codex agents (their notes and attached files), whether or not they're running. Claude agents' folders are protected. Browser cookies stay unreadable. Local services, including Tjommie's health endpoint, may be reachable from a sandboxed command.

## Status

Tjommie is in early development. The design is in [`docs/superpowers/specs/2026-09-25-tjommie-design.md`](docs/superpowers/specs/2026-09-25-tjommie-design.md).

## Develop

Building from source, the test commands and the repository layout are in [`docs/DEVELOPMENT.md`](docs/DEVELOPMENT.md).

## License

Apache-2.0. Tjommie is an independent project, not affiliated with the makers of Grok Bot, Claude Code or Codex.
