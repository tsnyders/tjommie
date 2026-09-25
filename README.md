# Tjommie

Tjommie is a free, open-source desktop and phone app for AI bots that chat with you, work on your PC, talk to each other and run routines. The bots run on your own computer through the official Claude Code and Codex CLIs, signed in with your own accounts.

**Status:** early development, nothing to install yet. The design is in [`docs/superpowers/specs/2026-09-25-tjommie-design.md`](docs/superpowers/specs/2026-09-25-tjommie-design.md).

## Develop

You need Windows and Node 24 or newer.

```sh
npm install
npm run dev        # UI at http://localhost:3000
npm run desktop    # Electron window showing that UI (keep npm run dev running)
npm test
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

## License

Apache-2.0. Tjommie is an independent project, not affiliated with the makers of Grok Bot, Claude Code or Codex.
