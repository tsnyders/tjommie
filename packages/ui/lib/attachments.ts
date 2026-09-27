export const MAX_FILES = 20; // chat.send takes at most 20 files

// Adds files the person picked or dropped, once each. webUtils.getPathForFile gives "" for a dropped item
// that isn't a file on disk. Returns null when the message would carry too many files.
export function addFiles(current: string[], incoming: string[]): string[] | null {
  const next = [...current];
  for (const path of incoming) if (path && !next.includes(path)) next.push(path);
  return next.length > MAX_FILES ? null : next;
}

export const fileName = (path: string): string => path.split(/[\\/]/).pop() ?? path;
