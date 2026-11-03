import { spawn } from "node:child_process";
import { mkdirSync, symlinkSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { afterEach, expect, it, vi } from "vitest";
import { tempRoot } from "./testing.ts";
import { validateWorkerAcceptance, validateWorkerPair, WindowsWorkerBoundary, workerConfigurationHash, type WindowsWorkerConfiguration } from "./windows-worker.ts";

vi.mock("node:child_process", () => ({ spawn: vi.fn(() => { throw new Error("Native launching is forbidden in this synthetic pair fixture."); }) }));
afterEach(() => { vi.restoreAllMocks(); vi.clearAllMocks(); });
function fixture() {
  const root = tempRoot(), ownerDataRoot = join(root, "owner"), runtimeRoot = join(root, "runtime"), providerRoot = join(root, "provider"), reviewRoot = join(root, "review");
  for (const path of [ownerDataRoot, runtimeRoot, providerRoot, reviewRoot]) mkdirSync(path);
  const config = (purpose: "provider" | "review"): WindowsWorkerConfiguration => ({ version: 2, purpose, ownerDataRoot, runtimeRoot, ownerSid: "S-1-5-21-1-2-3-1000", workerSid: purpose === "provider" ? "S-1-5-21-1-2-3-1001" : "S-1-5-21-1-2-3-1002", peerSid: purpose === "provider" ? "S-1-5-21-1-2-3-1002" : "S-1-5-21-1-2-3-1001", workerRoot: purpose === "provider" ? providerRoot : reviewRoot, peerRoot: purpose === "provider" ? reviewRoot : providerRoot, workerName: purpose === "provider" ? "TjWorker-12345678" : "TjReview-12345678", credentialTarget: `Tjommie/${"a".repeat(24)}/isolated-${purpose === "provider" ? "worker" : "review"}`, provisionedAt: "2026-11-03T00:00:00.000Z" });
  const provider = config("provider"), review = config("review"), providerPath = join(ownerDataRoot, "worker.json"), reviewPath = join(ownerDataRoot, "review-worker.json");
  writeFileSync(providerPath, JSON.stringify(provider)); writeFileSync(reviewPath, JSON.stringify(review));
  return { root, provider, review, providerPath, reviewPath, providerBoundary: new WindowsWorkerBoundary({ configurationPath: providerPath, exclusive: () => false }), reviewBoundary: new WindowsWorkerBoundary({ configurationPath: reviewPath, expectedPurpose: "review", exclusive: () => false }) };
}
it("binds both purpose, SID and canonical root pairs and refuses aliasing", () => {
  const f = fixture(); expect(() => validateWorkerPair(f.provider, f.review)).not.toThrow();
  for (const change of [{ workerSid: f.provider.workerSid }, { purpose: "provider" as const }, { workerRoot: f.provider.workerRoot }, { workerRoot: join(f.provider.workerRoot, "nested") }, { ownerSid: f.provider.workerSid }]) expect(() => validateWorkerPair(f.provider, { ...f.review, ...change })).toThrow();
  expect(f.providerBoundary.configuration()).toEqual(f.provider); expect(f.reviewBoundary.configuration()).toEqual(f.review);
  writeFileSync(f.reviewPath, JSON.stringify({ ...f.review, purpose: "provider" })); expect(() => f.providerBoundary.configuration()).toThrow();
});
it("never grants either peer profile, including through a directory junction or ancestor", async () => {
  const f = fixture(), alias = join(f.provider.workerRoot, "peer-link"); symlinkSync(f.review.workerRoot, alias, process.platform === "win32" ? "junction" : "dir");
  for (const path of [f.review.workerRoot, alias, f.root, f.provider.runtimeRoot, f.provider.ownerDataRoot]) await expect(f.providerBoundary.grantDirectory(path)).rejects.toThrow();
  for (const path of [f.provider.workerRoot, f.root, f.review.runtimeRoot, f.review.ownerDataRoot]) await expect(f.reviewBoundary.grantDirectory(path)).rejects.toThrow();
  expect(spawn).not.toHaveBeenCalled();
});
it("never grants owner metadata, credentials, attachments or trusted executable descendants", async () => {
  const f = fixture(), helperRoot = join(f.root, "helper"), helperChild = join(helperRoot, "scripts");
  mkdirSync(helperChild, { recursive: true });
  const boundary = new WindowsWorkerBoundary({ configurationPath: f.providerPath, helperPath: join(helperRoot, "windows-worker.ps1"), exclusive: () => false });
  const paths = [helperRoot, helperChild, join(f.provider.runtimeRoot, "claude"), ...["data", "accounts", "logs", "shared-files", "worker-receipts", "browser-profiles", "bots", "workspaces", "workspaces/.owners", "workspaces/.empty-hooks"].map((name) => join(f.provider.ownerDataRoot, name))];
  for (const path of paths) { mkdirSync(path, { recursive: true }); await expect(boundary.grantDirectory(path)).rejects.toThrow("isolated Windows provider"); }
  const bot = join(f.provider.ownerDataRoot, "bots", "bot-1"); mkdirSync(bot);
  for (const [name, target] of [["data-link", paths[3]!], ["runtime-link", f.provider.runtimeRoot], ["helper-link", helperRoot]]) {
    const alias = join(bot, name!); symlinkSync(target!, alias, process.platform === "win32" ? "junction" : "dir"); await expect(boundary.grantDirectory(alias)).rejects.toThrow();
  }
  expect(spawn).not.toHaveBeenCalled();
});
it("refuses owner metadata reached through an aliased managed root", async () => {
  const f = fixture(), metadata = join(f.provider.ownerDataRoot, "data"), nested = join(metadata, "looks-like-a-bot");
  mkdirSync(nested, { recursive: true }); symlinkSync(metadata, join(f.provider.ownerDataRoot, "bots"), process.platform === "win32" ? "junction" : "dir");
  await expect(f.providerBoundary.grantDirectory(join(f.provider.ownerDataRoot, "bots", "looks-like-a-bot"))).rejects.toThrow("isolated Windows provider");
  expect(spawn).not.toHaveBeenCalled();
});
it("preserves assigned bot/task folders and explicit external projects without granting their parents", async () => {
  const f = fixture();
  const paths = [f.provider.workerRoot, join(f.provider.ownerDataRoot, "bots", "bot-1", "sessions"), join(f.provider.ownerDataRoot, "workspaces", "task-1"), join(f.root, "assigned-project")];
  for (const path of paths) {
    mkdirSync(path, { recursive: true });
    vi.mocked(spawn).mockImplementationOnce(() => { throw new Error("Synthetic native grant boundary reached."); });
    await expect(f.providerBoundary.grantDirectory(path)).rejects.toThrow("Synthetic native grant boundary reached");
  }
  expect(spawn).toHaveBeenCalledTimes(paths.length);
  await expect(f.reviewBoundary.grantDirectory(paths[2]!)).rejects.toThrow("isolated Windows provider");
  expect(spawn).toHaveBeenCalledTimes(paths.length);
});
it("refuses review launches outside policy and never permits provider policy observations", async () => {
  const f = fixture();
  await expect(f.reviewBoundary.spawn("unused.exe", [], { cwd: f.review.workerRoot, env: {} })).rejects.toThrow("review_isolated_identity_unavailable");
  await expect(f.providerBoundary.withReviewPolicy(async () => "unsafe")).rejects.toThrow("review_isolated_identity_unavailable");
  expect(spawn).not.toHaveBeenCalled();
});
it("requires both purpose/SID/configuration hashes and every actual peer denial in acceptance", () => {
  const f = fixture();
  const accepted = { version: 2, purpose: "provider", workerSid: f.provider.workerSid, peerSid: f.review.workerSid, configurationHash: workerConfigurationHash(f.provider), peerConfigurationHash: workerConfigurationHash(f.review), ownerFileDenied: true, ownerCredentialDenied: true, ownerProcessDenied: true, ownerDesktopDenied: true, workerWriteAllowed: true, peerFileDenied: true, peerWriteDenied: true, peerRegistryDenied: true, peerProcessDenied: true, claudeStarted: true, codexStarted: true, reviewPolicyVerified: true };
  expect(() => validateWorkerAcceptance(f.provider, f.review, accepted)).not.toThrow();
  for (const patch of [{ purpose: "review" }, { workerSid: f.review.workerSid }, { peerSid: f.provider.workerSid }, { configurationHash: "0".repeat(64) }, { peerConfigurationHash: "0".repeat(64) }, { peerRegistryDenied: false }, { peerProcessDenied: false }, { peerWriteDenied: false }, { reviewPolicyVerified: false }, { version: 1 }]) expect(() => validateWorkerAcceptance(f.provider, f.review, { ...accepted, ...patch })).toThrow();
  expect(() => validateWorkerAcceptance(f.review, f.provider, accepted)).toThrow();
});
