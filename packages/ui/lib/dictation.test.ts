import { describe, expect, it } from "vitest";
import { inputEdit, insertTranscript, moveRange } from "./dictation";

describe("draft insertion", () => {
  it("moves the original range across typed edits before and after it", () => {
    const before = "a OLD z";
    const range = moveRange({ start: 2, end: 5 }, { before, start: 0, end: 0 }, before, "typed a OLD z");
    expect(range).toEqual({ start: 8, end: 11 });
    expect(moveRange(range, { before: "typed a OLD z", start: 12, end: 13 }, "typed a OLD z", "typed a OLD end")).toEqual(range);
    expect(insertTranscript("typed a OLD end", range!, "words")).toEqual({ text: "typed a words end", caret: 13 });
  });
  it("refuses overlapping, ambiguous, IME, and same-text replacement edits", () => {
    for (const edit of [null, { before: "OLD", start: 0, end: 3 }, { before: "different", start: 0, end: 0 }]) expect(moveRange({ start: 0, end: 3 }, edit, "OLD", "OLD")).toBeNull();
    expect(moveRange({ start: 1, end: 2 }, { before: "aaaa", start: 0, end: 0 }, "aaaa", "aaaaa")).toEqual({ start: 2, end: 3 });
  });
  it("keeps typing at a collapsed anchor before the final transcript", () => {
    expect(moveRange({ start: 1, end: 1 }, { before: "ab", start: 1, end: 1 }, "ab", "a typed b")).toEqual({ start: 8, end: 8 });
    expect(() => insertTranscript("a", { start: 1, end: 2 }, "words")).toThrow();
  });
  it("maps ordinary browser edits and refuses composition/history edits", () => {
    const field = { value: "hello", selectionStart: 3, selectionEnd: 3 } as HTMLInputElement;
    expect(inputEdit(field, { inputType: "deleteContentBackward", isComposing: false } as InputEvent)).toEqual({ before: "hello", start: 2, end: 3 });
    expect(inputEdit(field, { inputType: "historyUndo", isComposing: false } as InputEvent)).toBeNull();
    expect(inputEdit(field, { inputType: "insertCompositionText", isComposing: true } as InputEvent)).toBeNull();
  });
});
