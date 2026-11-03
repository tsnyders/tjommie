import { rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";
import { expect, it } from "vitest";
import { reviewSourceHash, reviewWorkerPolicyHash } from "./project-review-policy.ts";
import { tempRoot } from "./testing.ts";

it("binds packaged acceptance to the actual bundle and external worker launcher bytes and paths", () => {
  const root = tempRoot(), bundle = join(root, "daemon.mjs"), helper = join(root, "windows-worker.ps1");
  writeFileSync(bundle, "// synthetic daemon bundle\n"); writeFileSync(helper, "# synthetic worker launcher\n");
  const url = pathToFileURL(bundle).href, original = reviewSourceHash(url);
  writeFileSync(helper, "# changed synthetic worker launcher\n"); expect(reviewSourceHash(url)).not.toBe(original);
  writeFileSync(helper, "# synthetic worker launcher\n"); expect(reviewSourceHash(url)).toBe(original);
  writeFileSync(bundle, "// changed synthetic daemon bundle\n"); expect(reviewSourceHash(url)).not.toBe(original);
  const moved = tempRoot(); writeFileSync(join(moved, "daemon.mjs"), "// synthetic daemon bundle\n"); writeFileSync(join(moved, "windows-worker.ps1"), "# synthetic worker launcher\n");
  expect(reviewSourceHash(pathToFileURL(join(moved, "daemon.mjs")).href)).not.toBe(original);
  rmSync(helper); expect(() => reviewSourceHash(url)).toThrow();
});

it("binds worker evidence to its SID, observer and effective machine policy", () => {
  const receipt = { workerSid: "S-1-5-21-1-2-3-1001", observerSha256: "a".repeat(64), policySha256: "b".repeat(64) };
  const original = reviewWorkerPolicyHash(receipt);
  for (const patch of [{ workerSid: "S-1-5-21-1-2-3-1002" }, { observerSha256: "c".repeat(64) }, { policySha256: "d".repeat(64) }]) expect(reviewWorkerPolicyHash({ ...receipt, ...patch })).not.toBe(original);
  for (const patch of [{ workerSid: "owner" }, { observerSha256: "missing" }, { policySha256: "B".repeat(64) }]) expect(() => reviewWorkerPolicyHash({ ...receipt, ...patch })).toThrow("review_worker_policy_observer_unavailable");
});
