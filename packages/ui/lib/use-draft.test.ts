import { describe, expect, it } from "vitest";
import { attachPaths, attachmentsFor, ifAlive, scrollToShow, withoutSent, type Attachment } from "./use-draft";

describe("attachments across a send", () => {
  let id = 0;
  const next = (): number => (id += 1);

  it("keeps an attached file's identity, and gives a newly added one its own", () => {
    const first = attachmentsFor([], ["a.txt", "b.txt"], next);
    const more = attachmentsFor(first, ["a.txt", "b.txt", "c.txt"], next);
    expect(more.slice(0, 2)).toEqual(first);
    expect(more[2].id).not.toBe(first[0].id);
  });

  it("clears only what the send carried, keeping files added while it was on its way", () => {
    const sent = attachmentsFor([], ["a.txt"], next);
    const now = attachmentsFor(sent, ["a.txt", "b.txt"], next); // b added during the send
    expect(withoutSent(now, sent.map((each) => each.id)).map((each) => each.path)).toEqual(["b.txt"]);
  });

  it("keeps a file that was removed and added again during the send: it is a new attachment", () => {
    const sent = attachmentsFor([], ["a.txt"], next);
    const removed: Attachment[] = [];
    const readded = attachmentsFor(removed, ["a.txt"], next);
    expect(withoutSent(readded, sent.map((each) => each.id)).map((each) => each.path)).toEqual(["a.txt"]);
  });
});

describe("attachPaths", () => {
  const add = (existing: string[], added: string[]): string[] | null => {
    const merged = [...existing, ...added.filter((path) => !existing.includes(path))];
    return merged.length > 3 ? null : merged;
  };
  let id = 100;
  const next = (): number => (id += 1);

  it("adds to the attachments as they are now: a file a finished send cleared doesn't come back", () => {
    const seenWhenThePickerOpened = attachmentsFor([], ["a.txt"], next);
    const now = withoutSent(seenWhenThePickerOpened, seenWhenThePickerOpened.map((each) => each.id)); // the send of a.txt finished meanwhile
    expect(attachPaths(now, ["b.txt"], add, next)?.map((each) => each.path)).toEqual(["b.txt"]);
  });

  it("keeps the identities of what is already attached, and says no when it would be too many", () => {
    const current = attachmentsFor([], ["a.txt", "b.txt"], next);
    const more = attachPaths(current, ["c.txt"], add, next);
    expect(more?.slice(0, 2)).toEqual(current);
    expect(attachPaths(current, ["c.txt", "d.txt"], add, next)).toBeNull();
  });
});

describe("ifAlive", () => {
  it("acts for the component that started the work, and not once it is gone", () => {
    let closed = 0;
    ifAlive({ current: true }, () => (closed += 1));
    ifAlive({ current: false }, () => (closed += 1));
    expect(closed).toBe(1);
  });
});

describe("scrollToShow", () => {
  it("leaves the list where it is when the item is in view", () => {
    expect(scrollToShow(0, 200, 40, 36)).toBe(0);
  });

  it("scrolls just enough to show an item below or above the view", () => {
    expect(scrollToShow(0, 200, 190, 36)).toBe(26);
    expect(scrollToShow(100, 200, 40, 36)).toBe(40);
  });
});
