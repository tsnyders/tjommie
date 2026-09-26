import { existsSync, mkdirSync, mkdtempSync } from "node:fs";
import { join, resolve } from "node:path";
import { describe, expect, it } from "vitest";
import type { StoredBot } from "../bots.ts";
import { browserServer } from "../browser.ts";
import { resolveCli } from "../cli.ts";
import { accountEnv } from "../setup.ts";
import { firstOf, realAccount, tempRoot, waitFor } from "../testing.ts";
import { CodexSession } from "./codex.ts";
import type { ProviderEvent, StartOptions } from "./types.ts";

function session(overrides: Partial<StartOptions> = {}, cwd = tempRoot()) {
  const codex = new CodexSession(resolveCli("codex")!, {
    cwd,
    extraDirs: [],
    model: "", // the account's default model
    instructions: "You are a terse test bot.",
    autonomy: "folder",
    allowRules: [],
    resumeId: null,
    env: accountEnv(realAccount("codex")),
    mcpServers: [],
    ...overrides,
  });
  const events: ProviderEvent[] = [];
  codex.onEvent((event) => events.push(event));
  return { codex, events, cwd };
}

// A headless browser on a profile in `home`, as the daemon attaches it to a bot with the browser on.
const browser = (home: string, autonomy: StoredBot["autonomy"]) => browserServer({ homeFolder: home, autonomy } as StoredBot, { TJOMMIE_BROWSER_HEADLESS: "1" });

const text = (events: ProviderEvent[]) => events.map((e) => (e.type === "text" ? e.delta : "")).join("");

describe("CodexSession against the real CLI", () => {
  it("answers and reports its thread id and token usage", async () => {
    const { codex, events } = session();
    await codex.start();
    await codex.send("Reply with exactly: PONG");
    expect(await waitFor(() => firstOf(events, "turn_done"))).toEqual({ type: "turn_done", ok: true, error: null });
    expect(text(events)).toContain("PONG");
    expect(firstOf(events, "session")?.cliSessionId).toBeTruthy();
    expect(firstOf(events, "usage")?.output).toBeGreaterThan(0);
    codex.close();
  });

  it("adds a steered message to the running turn", async () => {
    const { codex, events } = session();
    await codex.start();
    await codex.send(`Run this shell command and wait for it: node -e "setTimeout(() => console.log('first'), 8000)". Then tell me what it printed.`);
    await waitFor(() => firstOf(events, "tool"));
    expect(await codex.steer("Also end your final answer with the word BANANA.")).toBe(true);
    await waitFor(() => firstOf(events, "turn_done"));
    expect(text(events)).toContain("BANANA");
    codex.close();
  });

  it("asks before writing outside its folder, and a denial keeps the file from being written", async () => {
    // Keep the denied target outside the sandbox's temp allowance and away from the real data root.
    const cache = resolve("node_modules/.cache/fix-A");
    mkdirSync(cache, { recursive: true });
    const outside = join(mkdtempSync(join(cache, "codex-outside-")), "denied.txt");
    const { codex, events } = session();
    await codex.start();
    await codex.send(`Write the text hi to the file ${outside} with a shell command.`);
    const approval = await waitFor(() => firstOf(events, "approval"));
    codex.answer(approval.id, "deny");
    await waitFor(() => firstOf(events, "turn_done"));
    expect(existsSync(outside)).toBe(false);
    codex.close();
  });

  it("asks before each browser tool of a server that isn't allowed outright, then browses in its own profile", async () => {
    // The thread runs in Own folder (workspace-write), where spike 5 measured Playwright; only the browser server asks.
    const cwd = tempRoot();
    const { codex, events } = session({ mcpServers: [browser(cwd, "ask")] }, cwd);
    await codex.start();
    await codex.send("Use the browser MCP tools, not the shell or web search, to open https://example.com. Reply with the page title only.");
    const answered = new Set<string>();
    await waitFor(() => {
      for (const event of events) {
        if (event.type === "approval" && !answered.has(event.id)) {
          answered.add(event.id);
          codex.answer(event.id, "once");
        }
      }
      return firstOf(events, "turn_done");
    });
    const approvals = events.filter((event) => event.type === "approval");
    expect(approvals.length).toBeGreaterThan(0);
    for (const approval of approvals) expect(approval).toMatchObject({ tool: expect.stringMatching(/^browser\./) });
    expect(text(events)).toContain("Example Domain");
    expect(existsSync(join(cwd, "chrome-profile", "Default"))).toBe(true);
    codex.close();
  });
});
