import { TextDecoder } from "node:util";
import { createHash } from "node:crypto";
import { ReviewCoverage, type TaskEvaluation } from "../../protocol/src/projects.ts";
import { Worktrees, type Repository } from "./worktrees.ts";

export type ReviewSnapshot = Readonly<{ sourceSha: string; files: readonly string[]; diff: string; read(path: string): string }>;
export type ReviewContext = Readonly<{ botId: string; projectId: string; taskId: string; attemptId: string; evaluationId: string; revision: number;
  projectRevision: number; taskRevision: number;
  instructions: Readonly<{ reviewer: string; role: string; title: string; task: string; acceptance: readonly string[] }> }>;
export type ReviewResult = Pick<TaskEvaluation, "state" | "reason" | "findings"> & { coverage?: ReviewCoverage | null };
export type ReviewRuntime = { review(snapshot: ReviewSnapshot, signal: AbortSignal, context: ReviewContext): Promise<ReviewResult> };
export const reviewHash = (bytes: string | Uint8Array): string => createHash("sha256").update(bytes).digest("hex");
export function frozenReviewContext(context: ReviewContext): ReviewContext {
  if (![context.projectRevision, context.taskRevision, context.revision].every((value) => Number.isSafeInteger(value) && value > 0)
    || !context.instructions || ![context.instructions.reviewer, context.instructions.role, context.instructions.title, context.instructions.task].every((value) => typeof value === "string")
    || !Array.isArray(context.instructions.acceptance) || context.instructions.acceptance.length > 50 || !context.instructions.acceptance.every((value) => typeof value === "string")) throw new Error("review_instruction_context_missing");
  if (Buffer.byteLength(JSON.stringify(context.instructions)) > 1_048_576) throw new Error("review_instruction_context_limit");
  return Object.freeze({ ...context, instructions: Object.freeze({ ...context.instructions, acceptance: Object.freeze([...context.instructions.acceptance]) }) });
}
export const reviewInstructionsHash = (context: ReviewContext): string => reviewHash(JSON.stringify({ projectRevision: context.projectRevision, taskRevision: context.taskRevision, revision: context.revision, instructions: context.instructions }));

export function frozenReviewInput(snapshot: ReviewSnapshot): { text: string; coverage: ReviewCoverage } {
  if (snapshot.files.length > 2048 || new Set(snapshot.files).size !== snapshot.files.length) throw new Error("review_coverage_incomplete: file count");
  let bytes = 0;
  const files = snapshot.files.map((path) => {
    if (!path || path.length > 2000) throw new Error("review_coverage_incomplete: file path");
    const text = snapshot.read(path), size = Buffer.byteLength(text);
    bytes += size;
    if (size > 262_144 || bytes > 2_097_152 || text.includes("\0")) throw new Error("review_coverage_incomplete: source size or encoding");
    return { path, text };
  });
  if (Buffer.byteLength(snapshot.diff) > 524_288) throw new Error("review_coverage_incomplete: diff size");
  const text = JSON.stringify({ sourceSha: snapshot.sourceSha, files, diff: snapshot.diff });
  if (Buffer.byteLength(text) > 4_194_304) throw new Error("review_coverage_incomplete: serialized source size");
  return { text, coverage: ReviewCoverage.parse({ sourceSha: snapshot.sourceSha, complete: true,
    snapshotSha256: reviewHash(text), diffSha256: reviewHash(snapshot.diff),
    files: files.map((file) => ({ path: file.path, bytes: Buffer.byteLength(file.text), sha256: reviewHash(file.text) })) }) };
}

export const reviewCoverage = (snapshot: ReviewSnapshot): ReviewCoverage => frozenReviewInput(snapshot).coverage;

export function reviewSnapshot(worktrees: Worktrees, repo: Repository, base: string, sourceSha: string): ReviewSnapshot {
  const files = new Map<string, string>();
  const decoder = new TextDecoder("utf-8", { fatal: true });
  let total = 0;
  let tree: string;
  try { tree = decoder.decode(worktrees.git(repo.path, ["ls-tree", "-r", "-z", sourceSha])); }
  catch { throw new Error("review_coverage_incomplete: unsupported file name encoding"); }
  for (const record of tree.split("\0").filter(Boolean)) {
    const tab = record.indexOf("\t"), [mode, kind, oid] = record.slice(0, tab).split(" "), path = record.slice(tab + 1);
    if (kind !== "blob" || !["100644", "100755"].includes(mode)) throw new Error("review_coverage_incomplete: symlink or submodule");
    const size = Number(worktrees.git(repo.path, ["cat-file", "-s", oid]).toString("utf8"));
    total += size;
    if (!Number.isSafeInteger(size) || size < 0 || size > 262_144 || total > 2_097_152 || files.size >= 2048) throw new Error("review_coverage_incomplete: source size limit");
    const bytes = worktrees.git(repo.path, ["cat-file", "blob", oid]);
    if (bytes.includes(0)) throw new Error("review_coverage_incomplete: binary file");
    try { files.set(path, decoder.decode(bytes)); } catch { throw new Error("review_coverage_incomplete: unsupported text encoding"); }
  }
  const diff = worktrees.diff(repo, base, sourceSha);
  if (diff.truncated) throw new Error("review_coverage_incomplete: diff size limit");
  return Object.freeze({ sourceSha, files: Object.freeze([...files.keys()]), diff: diff.text, read(path: string): string {
    if (!files.has(path)) throw new Error("The file is outside the immutable review snapshot.");
    return files.get(path)!;
  } });
}

// These flags are a candidate policy; no provider is enabled until a local acceptance probe establishes enforcement.
export const CLAUDE_REVIEW_FLAGS = ["--restricted", "--safe-mode", "--tools", "", "--strict-mcp-config", "--disable-slash-commands", "--setting-sources", "", "--permission-prompts", "none", "--no-session-persistence"] as const;
