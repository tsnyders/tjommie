import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, expect, it } from "vitest";
import { projectFixture } from "./project-test-support.ts";
import { frozenReviewInput, reviewCoverage, reviewSnapshot, type ReviewSnapshot } from "./project-review.ts";
import { reviewCliArgs, reviewEffectivePolicy, reviewEnvironment } from "./project-review-policy.ts";
import { validateReviewReply } from "./project-review-runtime.ts";
import { tempRoot } from "./testing.ts";

const open: ReturnType<typeof projectFixture>[] = [];
afterEach(() => { for (const fixture of open.splice(0)) fixture.db.close(); });
const snapshot: ReviewSnapshot = Object.freeze({ sourceSha: "a".repeat(40), files: Object.freeze(["src/example.ts"]), diff: "+return 1;",
  read(path: string) { if (path !== "src/example.ts") throw new Error("outside snapshot"); return "export const example = 1;\n"; } });
const reply = () => ({ sourceSha: snapshot.sourceSha, snapshotSha256: reviewCoverage(snapshot).snapshotSha256, complete: true,
  reviewedFiles: [...snapshot.files], state: "passed", reason: null, findings: [] });

it("reviews exact Git objects without reading changed worktree files, and fails closed on binary coverage", () => {
  const h = projectFixture(); open.push(h);
  const frozen = reviewSnapshot(h.worktrees, h.repository, h.base, h.base), before = reviewCoverage(frozen);
  writeFileSync(join(h.repoPath, "src/a.txt"), "uncommitted owner text\n");
  expect(frozen.read("src/a.txt")).toBe("A0\n"); expect(reviewCoverage(frozen)).toEqual(before);
  expect(Object.isFrozen(frozen)).toBe(true); expect(Object.isFrozen(frozen.files)).toBe(true);
  expect(() => frozen.read("../private")).toThrow(/outside/);
  writeFileSync(join(h.repoPath, "binary"), Buffer.from([1, 0, 2])); h.git(h.repoPath, "add", "binary"); h.git(h.repoPath, "commit", "-m", "Synthetic unsupported binary");
  expect(() => reviewSnapshot(h.worktrees, h.repository, h.base, h.git(h.repoPath, "rev-parse", "HEAD"))).toThrow(/coverage_incomplete/);
}, 30_000);

it("bounds and hashes the whole frozen input and rejects omitted, repeated or oversized source", () => {
  expect(JSON.parse(frozenReviewInput(snapshot).text)).toMatchObject({ files: [{ path: "src/example.ts", text: "export const example = 1;\n" }] });
  expect(reviewCoverage(snapshot)).toMatchObject({ complete: true, sourceSha: snapshot.sourceSha, files: [{ bytes: 26 }] });
  expect(() => frozenReviewInput({ ...snapshot, files: ["src/example.ts", "src/example.ts"] })).toThrow(/coverage_incomplete/);
  expect(() => frozenReviewInput({ ...snapshot, read: () => "x".repeat(262_145) })).toThrow(/coverage_incomplete/);
  expect(() => frozenReviewInput({ ...snapshot, diff: "x".repeat(524_289) })).toThrow(/coverage_incomplete/);
});

it("requires complete exact source coverage and confines findings to real snapshot lines", () => {
  expect(validateReviewReply(JSON.stringify(reply()), snapshot)).toMatchObject({ state: "passed", coverage: reviewCoverage(snapshot) });
  for (const patch of [{ sourceSha: "b".repeat(40) }, { snapshotSha256: "0".repeat(64) }, { complete: false }, { reviewedFiles: [] }, { reviewedFiles: ["src/example.ts", "src/example.ts"] },
    { findings: [{ severity: "high", path: "../private", line: 1, message: "Outside" }] }, { findings: [{ severity: "high", path: "src/example.ts", line: 99, message: "Outside line" }] }]) {
    expect(() => validateReviewReply(JSON.stringify({ ...reply(), ...patch }), snapshot)).toThrow();
  }
  const findings = [{ severity: "high", path: "src/example.ts", line: 1, message: "A concrete synthetic finding" }];
  expect(validateReviewReply(JSON.stringify({ ...reply(), findings }), snapshot).state).toBe("findings");
});

it("inspects effective tool and plugin exposure and pins a tool-free, hook-free API policy", () => {
  expect(reviewEffectivePolicy({ type: "system", subtype: "init", tools: [], mcp_servers: [], plugins: [] })).toEqual({ tools: [], mcpServers: [], plugins: [] });
  for (const patch of [{ tools: ["Bash"] }, { tools: undefined }, { mcp_servers: [{ name: "team" }] }, { plugins: [{ name: "unknown", path: "builtin", source: "unknown@builtin" }] }]) {
    expect(() => reviewEffectivePolicy({ type: "system", subtype: "init", tools: [], mcp_servers: [], plugins: [], ...patch })).toThrow(/unavailable/);
  }
  const args = reviewCliArgs("test", "owned-settings.json", 0.02);
  expect(args).toContain("--bare"); expect(args).toContain("--safe-mode"); expect(args[args.indexOf("--tools") + 1]).toBe("");
  expect(args[args.indexOf("--setting-sources") + 1]).toBe(""); expect(args).not.toContain("--resume");
  expect(args[args.indexOf("--max-budget-usd") + 1]).toBe("0.02");
  expect(() => reviewCliArgs("test", "owned-settings.json", 0)).toThrow(/budget/);
  const env = reviewEnvironment(tempRoot());
  expect(env.NODE_OPTIONS).toBeUndefined(); expect(env.ANTHROPIC_API_KEY).toBeUndefined(); expect(env.OPENAI_API_KEY).toBeUndefined();
  expect(env.CLAUDE_CONFIG_DIR).not.toBe(process.env.CLAUDE_CONFIG_DIR); expect(env.HOME).not.toBe(process.env.HOME);
});
