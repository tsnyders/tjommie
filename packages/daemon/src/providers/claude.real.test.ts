import { existsSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import type { StoredBot } from "../bots.ts";
import { browserServer } from "../browser.ts";
import { resolveCli } from "../cli.ts";
import { accountEnv } from "../setup.ts";
import { firstOf, realAccount, tempRoot, waitFor } from "../testing.ts";
import { ClaudeSession } from "./claude.ts";
import type { ProviderEvent, StartOptions } from "./types.ts";

function session(overrides: Partial<StartOptions> = {}, cwd = tempRoot()) {
  const claude = new ClaudeSession(resolveCli("claude")!, {
    cwd,
    extraDirs: [],
    model: "haiku",
    instructions: "You are a terse test bot.",
    autonomy: "folder",
    allowRules: [],
    resumeId: null,
    env: accountEnv(realAccount("claude")),
    mcpServers: [],
    ...overrides,
  });
  const events: ProviderEvent[] = [];
  claude.onEvent((event) => events.push(event));
  return { claude, events, cwd };
}

// A headless browser on a profile in `home`, as the daemon attaches it to a bot with the browser on.
const browser = (home: string, autonomy: StoredBot["autonomy"]) => browserServer({ homeFolder: home, autonomy } as StoredBot, { TJOMMIE_BROWSER_HEADLESS: "1" });

const text = (events: ProviderEvent[]) => events.map((e) => (e.type === "text" ? e.delta : "")).join("");

describe("ClaudeSession against the real CLI", () => {
  it("answers, reports its session id and the turn's cost", async () => {
    const { claude, events } = session();
    await claude.start();
    await claude.send("Reply with exactly: PONG");
    expect(await waitFor(() => firstOf(events, "turn_done"))).toEqual({ type: "turn_done", ok: true, error: null });
    expect(text(events)).toContain("PONG");
    expect(firstOf(events, "session")?.cliSessionId).toMatch(/^[0-9a-f-]{36}$/);
    expect(firstOf(events, "usage")?.costUsd).toBeGreaterThan(0);
    claude.close();
  });

  it("asks before writing in Always ask mode, then writes once allowed", async () => {
    const { claude, events, cwd } = session({ autonomy: "ask" });
    await claude.start();
    await claude.send("Use the Write tool to create hello.txt containing the word hi. Then reply DONE.");
    const approval = await waitFor(() => firstOf(events, "approval"));
    expect(approval).toMatchObject({ tool: "Write", rule: "Write" });
    claude.answer(approval.id, "once");
    await waitFor(() => firstOf(events, "turn_done"));
    expect(existsSync(join(cwd, "hello.txt"))).toBe(true);
    claude.close();
  });

  it("stops a running turn", async () => {
    const { claude, events } = session({ autonomy: "full" });
    await claude.start();
    await claude.send(`Use the Bash tool, not in the background, to run exactly: node -e "setTimeout(() => console.log('late'), 30000)". Then tell me what it printed.`);
    await waitFor(() => firstOf(events, "tool"));
    await claude.interrupt();
    expect(await waitFor(() => firstOf(events, "turn_done"), 20_000)).toMatchObject({ ok: false });
    claude.close();
  });

  it("browses in its own profile through the bundled Playwright MCP, without asking", async () => {
    const cwd = tempRoot();
    const { claude, events } = session({ mcpServers: [browser(cwd, "folder")] }, cwd);
    await claude.start();
    await claude.send("Use the browser tools to open https://example.com, then reply with the page title only.");
    await waitFor(() => firstOf(events, "turn_done"));
    expect(text(events)).toContain("Example Domain");
    expect(firstOf(events, "approval")).toBeUndefined();
    expect(existsSync(join(cwd, "chrome-profile", "Default"))).toBe(true);
    claude.close();
  });
});
