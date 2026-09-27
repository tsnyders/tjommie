# Tjommie

Tjommie is a free, open-source desktop and phone app for AI bots that chat with you, work on your PC, talk to each other and run routines. The bots run on your own computer through the official Claude Code and Codex CLIs, signed in with your own accounts.

**Status:** early development, nothing to install yet. The design is in [`docs/superpowers/specs/2026-09-25-tjommie-design.md`](docs/superpowers/specs/2026-09-25-tjommie-design.md).

## Develop

You need Windows and Node 24 or newer.

Install the Claude Code and Codex CLIs and put them on PATH. Tjommie stores its data in `%LOCALAPPDATA%\Tjommie`; set `TJOMMIE_HOME` to use another folder and `TJOMMIE_PORT` to change the default port, 47100. The browser feature needs Chrome or Edge. From the repo root, `npm run chat -w @tjommie/daemon -- <name>` creates the named bot if it does not exist.

`npm start -w @tjommie/daemon` starts development mode. It saves a `daemon-token` file in the Tjommie data folder so the terminal chat can connect. Development mode also allows the UI at `http://localhost:3000`. Running `src/main.ts --dev` directly or setting `$env:TJOMMIE_DEV = '1'` does the same thing. The desktop origin `app://tjommie` is always allowed.

The installed desktop app starts the daemon with `--token-stdin`. It must start the daemon with `TJOMMIE_DEV` unset. It passes a fresh token as the first line of a pipe to the daemon's standard input. The daemon requires at least 43 base64url characters within 5 seconds, never saves this token to a file, and deletes an old `daemon-token` file left by development mode. The terminal chat only works with a development daemon. Starting the daemon with neither mode exits with instructions to use the desktop app or the development command.

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
| `packages/daemon` | The background process that runs the bots |
| `packages/ui` | The UI: Next.js static export, shadcn/ui, Tailwind |
| `packages/desktop` | Electron shell |

## Autonomy

Own folder protects your files from a bot's mistakes. It doesn't contain a bot that has been tricked by what it reads. On Windows, a Codex bot at Own folder can still read files your account can read and reach programs on this PC, including Tjommie itself. Until the desktop app ships, don't give a Codex bot at Own folder untrusted input such as web browsing or email; use Always ask for those bots.

## License

Apache-2.0. Tjommie is an independent project, not affiliated with the makers of Grok Bot, Claude Code or Codex.
