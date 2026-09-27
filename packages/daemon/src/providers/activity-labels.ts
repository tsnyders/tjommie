// Activity labels are stored with the message, so both terminal and UI see the same words.
export function plainActivityText(value: string): string {
  return value
    .replace(/select:\s*/gi, "")
    .replace(/mcp__[a-z0-9_-]+?__([a-z0-9_-]+)/gi, (_match, tool: string) => tool.replace(/_/g, " "))
    .replace(/mcp__/gi, "")
    .replace(/__+/g, " ")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, 200);
}

function textArg(input: Record<string, unknown>, key: string): string | null {
  const value = input[key];
  return typeof value === "string" && value.trim() ? plainActivityText(value.trim()) || null : null;
}

function fileName(path: string | null): string {
  return path?.split(/[\\/]/).filter(Boolean).at(-1) || "a file";
}

function browserTarget(input: Record<string, unknown>): string {
  const url = textArg(input, "url");
  if (!url) return "a page";
  try {
    return new URL(url).hostname || url;
  } catch {
    return url;
  }
}

function browserLabel(tool: string, input: Record<string, unknown>): string {
  const action = tool.replace(/^browser_/, "");
  switch (action) {
    case "navigate": return `Browser: opened ${browserTarget(input)}`;
    case "navigate_back": return "Browser: went back";
    case "click": return `Browser: clicked ${textArg(input, "element") ?? "an element"}`;
    case "snapshot": return "Browser: viewed the page";
    case "take_screenshot": return "Browser: took a screenshot";
    case "type": return "Browser: typed into a field";
    case "fill_form": return "Browser: filled a form";
    case "press_key": return `Browser: pressed ${textArg(input, "key") ?? "a key"}`;
    case "select_option": return "Browser: chose an option";
    case "hover": return `Browser: hovered over ${textArg(input, "element") ?? "an element"}`;
    default: return `Browser: ${action.replace(/_/g, " ")}`;
  }
}

export function activityLabel(name: string, input: Record<string, unknown>, serverName?: string): string | null {
  if (/^ToolSearch$/i.test(name) || /^select:/i.test(name) || /^select:/i.test(String(input.query ?? "").trim())) return null;

  const fullName = /^mcp__(.+?)__(.+)$/.exec(name);
  const server = fullName?.[1] ?? serverName;
  const tool = fullName?.[2] ?? name;
  let label: string;

  if (server === "tjommie") {
    switch (tool) {
      case "list_bots": label = "Looked at the team"; break;
      case "message_bot": label = `Messaged ${textArg(input, "bot") ?? "a bot"}`; break;
      case "share_file": label = `Shared ${fileName(textArg(input, "path"))}`; break;
      case "add_routine": label = `Proposed a routine: ${textArg(input, "name") ?? "a routine"}`; break;
      default: label = tool.replace(/_/g, " ");
    }
  } else if (server === "browser") {
    label = browserLabel(tool, input);
  } else if (server) {
    label = tool.replace(/_/g, " ");
  } else {
    label = name;
    for (const key of ["description", "command", "file_path", "url", "pattern", "query"]) {
      const value = textArg(input, key);
      if (value) {
        label = value;
        break;
      }
    }
  }

  return plainActivityText(label);
}
