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
