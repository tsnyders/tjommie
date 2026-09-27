# Tjommie

Tjommie is a free, open-source desktop and phone app for AI bots that chat with you, work on your PC, talk to each other and run routines. The bots run on your own computer through the official Claude Code and Codex CLIs, signed in with your own accounts.

**Status:** early development, nothing to install yet. The design is in [`docs/superpowers/specs/2026-09-25-tjommie-design.md`](docs/superpowers/specs/2026-09-25-tjommie-design.md).

## Develop

You need Windows and Node 24 or newer.

Install the Claude Code and Codex CLIs and put them on PATH. Tjommie stores its data in `%LOCALAPPDATA%\Tjommie`; set `TJOMMIE_HOME` to use another folder and `TJOMMIE_PORT` to change the default port, 47100. The browser feature needs Chrome or Edge. From the repo root, `npm run chat -w @tjommie/daemon -- <name>` creates the named bot if it does not exist.

`npm start -w @tjommie/daemon` starts development mode. It saves a `daemon-token` file in the Tjommie data folder so the terminal chat can connect. Development mode also allows the UI at `http://localhost:3000`. Running `src/main.ts --dev` directly or setting `$env:TJOMMIE_DEV = '1'` does the same thing. The desktop origin `app://tjommie` is always allowed.

The installed desktop app starts the daemon with `--token-stdin`. It must start the daemon with `TJOMMIE_DEV` unset. It passes a fresh token as the first line of a pipe to the daemon's standard input and keeps that pipe open. The daemon requires at least 43 base64url characters within 5 seconds, never saves this token to a file, deletes an old `daemon-token` file left by development mode, and stops when the pipe closes, so quitting or crashing the desktop app never leaves a daemon running. The terminal chat only works with a development daemon. Starting the daemon with neither mode exits with instructions to use the desktop app or the development command. Tests can set `TJOMMIE_ACCOUNTS_DIR` to use signed-in account folders with a disposable data root.

```sh
npm install
npm run signin -w @tjommie/daemon    # once: sign Tjommie's own Claude and Codex accounts in
npm start -w @tjommie/daemon         # development daemon, on http://127.0.0.1:47100
npm run chat -w @tjommie/daemon -- "Researcher" claude   # chat with a bot from the terminal
#   in the chat: /room <text> posts in the Team room; /allow <id> answers a card, including a routine a bot proposes
npm run dev        # UI at http://localhost:3000
npm run desktop    # Electron window showing that UI (keep npm run dev running)
npm test           # fast tests
npm run test:real  # tests that drive the real Claude and Codex CLIs (needs the sign-in above)
npm run typecheck
npm run lint
npm run build      # static export to packages/ui/out
```

| Package | What it is |
|---|---|
| `packages/protocol` | Messages shared by the daemon and the UI |
| `packages/client` | The typed WebSocket client for the daemon, shared by the UI, the desktop app and the terminal chat |
| `packages/daemon` | The background process that runs the bots |
| `packages/ui` | The UI: Next.js static export, shadcn/ui, Tailwind |
| `packages/desktop` | Electron shell |

## Autonomy

On Windows, Codex bots at Own folder need Codex's elevated sandbox. Setting it up may show one Windows admin prompt. Until it is ready, those bots show Needs setup and do not run. Tjommie denies sandboxed Codex commands access to its sign-in folders, Claude bots' folders, logs, database and shared-files folder. The installed daemon token is passed through a pipe; a development token file is protected if present.

Codex bots can still read the rest of your user profile. A Codex bot at Own folder can read the folders of your other Codex bots (their notes and attached files), whether or not they're running. Claude bots' folders are protected. Browser cookies stay unreadable. Local services, including Tjommie's health endpoint, may be reachable from a sandboxed command.

## License

Apache-2.0. Tjommie is an independent project, not affiliated with the makers of Grok Bot, Claude Code or Codex.
