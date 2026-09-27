const oneLine = (text: string): string => text.replace(/\s+/g, " ").trim().slice(0, 240);

// A short line for what an approval card asks about. Claude sends the tool's input as JSON; Codex sends the
// command or folder as text.
export function approvalDetail(detail: string): string {
  try {
    const input: unknown = JSON.parse(detail);
    if (input && typeof input === "object" && !Array.isArray(input)) {
      const record = input as Record<string, unknown>;
      for (const key of ["file_path", "path", "notebook_path", "command", "url", "pattern", "query"]) {
        const value = record[key];
        if (typeof value === "string") return oneLine(value);
      }
      const firstText = Object.values(record).find((value): value is string => typeof value === "string");
      if (firstText) return oneLine(firstText);
    }
  } catch {
    // not JSON: already plain text
  }
  return oneLine(detail);
}
