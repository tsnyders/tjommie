export type InsertionRange = { start: number; end: number };
export type DraftEdit = { before: string; start: number; end: number };

export function inputEdit(field: HTMLInputElement | HTMLTextAreaElement, event: InputEvent): DraftEdit | null {
  let start = field.selectionStart ?? field.value.length;
  let end = field.selectionEnd ?? start;
  if (typeof event.inputType !== "string" || event.isComposing || event.inputType.includes("Composition")) return null;
  if (start === end && event.inputType === "deleteContentBackward") start = Math.max(0, start - 1);
  else if (start === end && event.inputType === "deleteContentForward") end = Math.min(field.value.length, end + 1);
  else if (!/^(?:insertText|insertFromPaste|insertFromDrop|insertReplacementText|insertLineBreak|insertParagraph|deleteContentBackward|deleteContentForward|deleteByCut)$/.test(event.inputType)) return null;
  return { before: field.value, start, end };
}

export function moveRange(range: InsertionRange | null, edit: DraftEdit | null, before: string, after: string): InsertionRange | null {
  if (!range || !edit || edit.before !== before) return null;
  const { start, end } = edit;
  const inserted = after.length - (before.length - (end - start));
  if (inserted < 0 || after.slice(0, start) !== before.slice(0, start) || after.slice(start + inserted) !== before.slice(end)) return null;
  if (end <= range.start) return { start: range.start + inserted - (end - start), end: range.end + inserted - (end - start) };
  return start >= range.end ? range : null;
}

export function insertTranscript(text: string, range: InsertionRange, transcript: string): { text: string; caret: number } {
  if (range.start < 0 || range.end < range.start || range.end > text.length) throw new Error("The dictation insertion point changed.");
  return { text: text.slice(0, range.start) + transcript + text.slice(range.end), caret: range.start + transcript.length };
}
