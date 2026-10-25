// @mentions in room posts. The daemon uses them to pick who answers, and the UI uses the same rule for its
// hint and its highlighting, so the two never disagree.
const WORD = /[\p{L}\p{N}\p{M}_]/u;
const EMAIL_END = /[.\-+%]/u;

export type MentionRange = { start: number; end: number; name: string | null }; // name null: @everyone

// Each "@everyone" and "@Name" (whole names, any case, longest first) in the text, in order. A mention can't
// touch letters, digits or "_" on either side, or follow an email-like ".", "-", "+" or "%".
export function mentionRanges(text: string, names: string[]): MentionRange[] {
  // ponytail: offsets come from the lowercased text, so they drift after a character whose lowercase is longer.
  let rest = text.toLowerCase();
  const ranges: MentionRange[] = [];
  const take = (tag: string, name: string | null): void => {
    for (let at = rest.indexOf(tag); at !== -1; at = rest.indexOf(tag, at + tag.length)) {
      const before = rest[at - 1] ?? "";
      if (WORD.test(before) || EMAIL_END.test(before) || WORD.test(rest[at + tag.length] ?? "")) continue;
      rest = rest.slice(0, at) + " ".repeat(tag.length) + rest.slice(at + tag.length);
      ranges.push({ start: at, end: at + tag.length, name });
    }
  };
  take("@everyone", null);
  for (const name of [...names].sort((a, b) => b.length - a.length)) take(`@${name.toLowerCase()}`, name);
  return ranges.sort((a, b) => a.start - b.start);
}

// The ids of the members a post mentions, in the order given; in a room, its bots. Names are read against every bot, so "@Bot 2" is never
// taken for "@Bot" in a room without Bot 2. "@everyone" mentions every member.
export function findMentions(text: string, bots: { id: string; name: string }[], members = bots): string[] {
  const ranges = mentionRanges(text, bots.map((bot) => bot.name));
  if (ranges.some((range) => range.name === null)) return members.map((bot) => bot.id);
  const named = new Set(ranges.map((range) => range.name));
  return members.filter((bot) => named.has(bot.name)).map((bot) => bot.id);
}
