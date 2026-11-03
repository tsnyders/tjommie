import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { expect, it, vi } from "vitest";
import { createProjectReviewRuntime } from "./project-review-runtime.ts";
import type { ProviderEvent } from "./providers/types.ts";
import { projectFixture } from "./project-test-support.ts";
import { TaskIntegrations } from "./task-integration.ts";
import { tempRoot } from "./testing.ts";

// Explicit, separately provisioned acceptance credentials only. Never consult realAccount(),
// an account login folder, keychain or inherited generic API-key variables in this fixture.
it.skipIf(process.env.TJOMMIE_REAL_REVIEW_ACCEPTANCE !== "1")("accepts the installed authenticated tool-free CLI, then reviews/checks/integrates an owned synthetic developer commit", async () => {
  const key = process.env.TJOMMIE_REVIEW_ACCEPTANCE_API_KEY, model = process.env.TJOMMIE_REVIEW_ACCEPTANCE_MODEL;
  const maximum = Number(process.env.TJOMMIE_REVIEW_ACCEPTANCE_MAX_USD);
  if (!key || !model || !Number.isFinite(maximum) || maximum <= 0) throw new Error("Provide a dedicated acceptance API key, exact model and explicit positive USD allowance. Owner account profiles are not read.");
  const root = tempRoot(), events: ProviderEvent[] = []; let spent = 0;
  const runtime = createProjectReviewRuntime({ root: join(root, "reviewer"), async resolve() {
    return { provider: "claude", kind: "api_key", model, authentication: { kind: "api_key", key }, assertCurrent() {},
      getMaxBudgetUsd() { return maximum - spent; }, onEvent(event) { events.push(event); if (event.type === "usage") { if (event.costUsd === null) throw new Error("Acceptance usage cost was unavailable."); spent += event.costUsd; } }, release() {} };
  } });
  const h = projectFixture({ review: runtime }); let integrations: TaskIntegrations | undefined;
  try {
    h.git(h.repoPath, "branch", "delivery", h.base); h.projects.update(h.project.id, 1, { targetRef: "refs/heads/delivery" });
    writeFileSync(join(h.repoPath, "src/b.txt"), "Synthetic owner working bytes retained\n"); const before = h.owner();
    const task = h.newTask(), attempt = await h.complete(h.tasks.start(task.id, task.revision, "owned-synthetic-developer"), "src/a.txt", "A1\n");
    const reviewed = await h.tasks.evaluate(attempt.id, attempt.revision, attempt.headSha!, "review");
    expect(reviewed.state, reviewed.reason ?? "Review did not complete").toBe("passed"); expect(reviewed.reviewCoverage?.sourceSha).toBe(attempt.headSha);
    expect(events.filter((event) => event.type === "usage")).toHaveLength(2); expect(spent).toBeGreaterThanOrEqual(0); expect(spent).toBeLessThanOrEqual(maximum);
    const receipts = readdirSync(join(root, "reviewer", "receipts")); expect(receipts).toHaveLength(1);
    const receipt = JSON.parse(readFileSync(join(root, "reviewer", "receipts", receipts[0]), "utf8")) as { identity: { version: string; platform: string }; attempts: string[]; costUsd: number };
    expect(receipt.identity.platform).toBe(process.platform); expect(receipt.identity.version).toMatch(/^2\.1\.\d+$/); expect(receipt.attempts).toHaveLength(6);
    expect((await h.tasks.evaluate(attempt.id, attempt.revision, attempt.headSha!, "test")).state).toBe("passed"); expect(h.tasks.accepted(attempt)).toBe(true);
    integrations = new TaskIntegrations(h.tasks);
    const record = integrations.start({ attemptId: attempt.id, expectedRevision: attempt.revision, sourceSha: attempt.headSha!, expectedTargetSha: h.base, requestId: "owned-real-review-integration" });
    await vi.waitFor(() => expect(integrations!.get(record.id).state).toBe("integrated"), { timeout: 30_000 });
    expect(h.owner()).toEqual(before);
    // Receipt/evaluation files retain actual CLI/policy/source/config fingerprints and reported costs.
    console.info(`Owned real reviewer acceptance: ${events.filter((event) => event.type === "usage").length} paid turns; reported total USD ${spent.toFixed(6)}; CLI ${receipt.identity.version}.`);
  } finally { await integrations?.shutdown(); await h.tasks.shutdown(); h.db.close(); }
}, 600_000);
