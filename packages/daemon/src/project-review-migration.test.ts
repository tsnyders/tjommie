import { join } from "node:path";
import { expect, it } from "vitest";
import { migrations, openDb } from "./db.ts";
import { PROJECT_MIGRATION, PROJECT_REVIEW_COVERAGE_MIGRATION } from "./project-schema.ts";
import { projectFixture } from "./project-test-support.ts";
import { reviewCoverage } from "./project-review.ts";

it("upgrades legacy evaluation JSON to explicit null coverage without changing settled source results", async () => {
  const h = projectFixture({ review: { async review(snapshot) { return { state: "passed", reason: null, findings: [], coverage: reviewCoverage(snapshot) }; } } });
  let db = h.db;
  try {
    const task = h.newTask(), attempt = await h.complete(h.tasks.start(task.id, task.revision, "legacy-evaluation"), "src/a.txt", "A1\n");
    const review = await h.tasks.evaluate(attempt.id, attempt.revision, attempt.headSha!, "review");
    db.prepare("UPDATE task_evaluations SET data=json_remove(data,'$.reviewCoverage') WHERE id=?").run(review.id);
    const version = migrations.indexOf(PROJECT_REVIEW_COVERAGE_MIGRATION);
    expect(version).toBeGreaterThan(migrations.indexOf(PROJECT_MIGRATION)); db.exec(`PRAGMA user_version=${version}`);
    db.close(); db = openDb(join(h.root, "test.sqlite"));
    const row = db.prepare("SELECT state,source_sha,data FROM task_evaluations WHERE id=?").get(review.id) as { state: string; source_sha: string; data: string };
    expect(row.state).toBe("passed"); expect(row.source_sha).toBe(attempt.headSha);
    expect(JSON.parse(row.data)).toMatchObject({ reviewCoverage: null, sourceSha: attempt.headSha, state: "passed" });
  } finally { db.close(); }
}, 30_000);
