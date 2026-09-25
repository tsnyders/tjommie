import { existsSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";
import { resolveCli } from "../cli.ts";
import { dataPaths } from "../paths.ts";
import { accountEnv } from "../setup.ts";
import { firstOf, realAccount, tempRoot, waitFor } from "../testing.ts";
import { CodexSession } from "./codex.ts";
import type { ProviderEvent, StartOptions } from "./types.ts";

function session(overrides: Partial<StartOptions> = {}) {
  const cwd = tempRoot();
  const codex = new CodexSession(resolveCli("codex")!, {
    cwd,
    extraDirs: [],
    model: "", // the account's default model
    instructions: "You are a terse test bot.",
    autonomy: "folder",
    allowRules: [],
    resumeId: null,
    env: accountEnv(realAccount("codex")),
    ...overrides,
  });
  const events: ProviderEvent[] = [];
  codex.onEvent((event) => events.push(event));
  return { codex, events, cwd };
}

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
    const outside = join(dataPaths().root, `tjommie-outside-${Date.now()}.txt`); // not under %TEMP%, which the sandbox allows
    const { codex, events } = session();
    await codex.start();
    await codex.send(`Write the text hi to the file ${outside} with a shell command.`);
    const approval = await waitFor(() => firstOf(events, "approval"));
    codex.answer(approval.id, "deny");
    await waitFor(() => firstOf(events, "turn_done"));
    expect(existsSync(outside)).toBe(false);
    codex.close();
  });
});
