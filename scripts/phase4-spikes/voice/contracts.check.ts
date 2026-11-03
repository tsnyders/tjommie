import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import test from "node:test";

// This models the proposed boundary only; it is not wired into a composer or speech engine.
type Scope = { host: string; kind: "chat" | "room" | "exchange"; target: string; mount: number; generation: number };
type Session = { id: number; scope: Scope; range: { start: number; end: number } | null; phase: "permission" | "listening" | "transcribing" | "cancelled" | "done" };
type Draft = { text: string; files: string[] };
type Result = { outcome: "inserted" | "stale" | "conflict" | "no-speech"; draft: Draft };

function sameScope(left: Scope, right: Scope): boolean {
  return left.host === right.host && left.kind === right.kind && left.target === right.target && left.mount === right.mount && left.generation === right.generation;
}

function edit(draft: Draft, session: Session, start: number, end: number, text: string): Draft {
  assert(Number.isInteger(start) && Number.isInteger(end) && start >= 0 && end >= start && end <= draft.text.length);
  const range = session.range;
  if (range) {
    if (end <= range.start) {
      const shift = text.length - (end - start);
      session.range = { start: range.start + shift, end: range.end + shift };
    } else if (start < range.end) {
      session.range = null;
    }
  }
  return { ...draft, text: draft.text.slice(0, start) + text + draft.text.slice(end) };
}

function finish(draft: Draft, current: Scope, session: Session, resultId: number, transcript: string, speechDetected: boolean): Result {
  if (session.id !== resultId || !sameScope(current, session.scope) || session.phase !== "transcribing") return { outcome: "stale", draft };
  session.phase = "done";
  if (!speechDetected || !transcript.trim()) return { outcome: "no-speech", draft };
  if (!session.range) return { outcome: "conflict", draft };
  const { start, end } = session.range;
  assert(start >= 0 && end >= start && end <= draft.text.length);
  return { outcome: "inserted", draft: { ...draft, text: draft.text.slice(0, start) + transcript + draft.text.slice(end) } };
}

const scope: Scope = { host: "synthetic-host", kind: "chat", target: "synthetic-bot", mount: 1, generation: 1 };
const draft = (): Draft => ({ text: "before OLD after", files: ["synthetic-attachment"] });
const session = (): Session => ({ id: 1, scope: { ...scope }, range: { start: 7, end: 10 }, phase: "transcribing" });

test("final text replaces the captured range and preserves attachments without a send operation", () => {
  assert.deepEqual(finish(draft(), scope, session(), 1, "new words", true), {
    outcome: "inserted", draft: { text: "before new words after", files: ["synthetic-attachment"] },
  });
});

test("edits before and after the captured range survive final insertion", () => {
  const active = session();
  let current = edit(draft(), active, 0, 0, "typed ");
  current = edit(current, active, current.text.length, current.text.length, " typed");
  assert.equal(finish(current, scope, active, 1, "new words", true).draft.text, "typed before new words after typed");
});

test("overlapping edit keeps all current text and offers an explicit conflict instead of overwriting it", () => {
  const active = session();
  const current = edit(draft(), active, 7, 10, "typed words");
  assert.deepEqual(finish(current, scope, active, 1, "new words", true), { outcome: "conflict", draft: current });
});

test("typing at a collapsed insertion point stays before the transcript", () => {
  const active = session();
  active.range = { start: 7, end: 7 };
  const current = edit(draft(), active, 7, 7, "typed ");
  assert.equal(finish(current, scope, active, 1, "spoken ", true).draft.text, "before typed spoken OLD after");
});

test("host, conversation kind/id, mount and generation changes independently reject late results", () => {
  const replacements: Scope[] = [
    { ...scope, host: "another-host" }, { ...scope, kind: "room" }, { ...scope, target: "another-target" },
    { ...scope, mount: 2 }, { ...scope, generation: 2 },
  ];
  for (const current of replacements) assert.deepEqual(finish(draft(), current, session(), 1, "late", true), { outcome: "stale", draft: draft() });
});

test("cancel, pending permission, active capture, stale request and duplicate final cannot insert", () => {
  for (const phase of ["permission", "listening", "cancelled", "done"] as const) {
    assert.equal(finish(draft(), scope, { ...session(), phase }, 1, "late", true).outcome, "stale");
  }
  assert.equal(finish(draft(), scope, session(), 2, "late", true).outcome, "stale");
  const active = session();
  const accepted = finish(draft(), scope, active, 1, "once", true);
  assert.equal(finish(accepted.draft, scope, active, 1, "twice", true).outcome, "stale");
});

test("incoming messages do not change draft identity; unsupported edits invalidate the insertion range", () => {
  assert.equal(finish(draft(), { ...scope }, session(), 1, "words", true).outcome, "inserted");
  assert.equal(finish(draft(), scope, { ...session(), range: null }, 1, "words", true).outcome, "conflict");
});

test("empty, silence and noise-only synthetic recognition outcomes do not become successful dictation", () => {
  for (const result of [{ text: "", speech: true }, { text: "hallucinated silence", speech: false }, { text: "hallucinated noise", speech: false }]) {
    assert.deepEqual(finish(draft(), scope, session(), 1, result.text, result.speech), { outcome: "no-speech", draft: draft() });
  }
});

test("sixty seconds of 16 kHz mono PCM16 stays under the proposed audio limit", () => {
  const maxFrames = 60 * 16000;
  const pcm = new Int16Array(maxFrames);
  assert.equal(pcm.byteLength, 1920000);
  assert(pcm.every((sample) => sample === 0));
  const accepts = (frames: number): boolean => Number.isInteger(frames) && frames > 0 && frames <= maxFrames;
  assert(accepts(maxFrames));
  assert(!accepts(maxFrames + 1));
  assert(!accepts(-1));
  assert(!accepts(Number.NaN));
  assert.equal(44 + pcm.byteLength, 1920044);
});

test("synthetic model data is not ready until byte count and SHA256 both match", () => {
  const bytes = Buffer.from("disposable synthetic model bytes");
  const expected = createHash("sha256").update(bytes).digest("hex");
  const verified = (candidate: Uint8Array): boolean => candidate.byteLength === bytes.byteLength && createHash("sha256").update(candidate).digest("hex") === expected;
  assert(verified(bytes));
  assert(!verified(bytes.subarray(0, bytes.length - 1)));
  const changed = Buffer.from(bytes);
  changed[0] = changed.readUInt8(0) ^ 1;
  assert(!verified(changed));
});

test("provenance records a source build, exact model identity, and no invented binary hash", () => {
  const value: unknown = JSON.parse(readFileSync(new URL("./provenance.json", import.meta.url), "utf8"));
  assert(value !== null && typeof value === "object" && "runtime" in value && "model" in value);
  const runtime = value.runtime;
  const model = value.model;
  assert(runtime !== null && typeof runtime === "object" && "commit" in runtime && "binarySha256" in runtime && "binaryBytes" in runtime);
  assert(model !== null && typeof model === "object" && "sha256" in model && "bytes" in model);
  assert.match(String(runtime.commit), /^[a-f0-9]{40}$/);
  assert.equal(runtime.binarySha256, null);
  assert.equal(runtime.binaryBytes, null);
  assert.match(String(model.sha256), /^[a-f0-9]{64}$/);
  assert.equal(model.bytes, 147964211);
});
