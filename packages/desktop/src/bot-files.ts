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
