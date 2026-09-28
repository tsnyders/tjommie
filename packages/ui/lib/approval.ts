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

// What "Always allow" saves, in words. Claude's rules read as they are ("Write", "Bash(npm test:*)"). Codex's are a
// command prefix (codex:["npm","test"]) or an MCP tool (codex-mcp:browser.browser_navigate).
export function approvalRule(rule: string): string {
  if (rule.startsWith("codex-mcp:")) return oneLine(rule.slice("codex-mcp:".length));
  if (rule.startsWith("codex:")) {
    try {
      const words: unknown = JSON.parse(rule.slice("codex:".length));
      if (Array.isArray(words) && words.length > 0) return oneLine(words.join(" "));
    } catch {
      // not the list Codex sends: show it as it is
    }
  }
  return oneLine(rule);
}
