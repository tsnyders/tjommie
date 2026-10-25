import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

// Static checks on the senders: the rule they follow (clear only what was sent, never while anything newer is there) is in lib/use-draft and tested there;
// these check each composer actually goes through it. The real typing-while-sending flow is the root's real-DOM test.
const read = (path: string): string => readFileSync(new URL(path, import.meta.url), "utf8");

describe("the chat composer keeps what was typed while a message was on its way", () => {
  const source = read("./composer.tsx");

  it("sends what was in the box when Send was pressed, and clears the text only if it is unchanged since", () => {
    expect(source).toContain("const sentText = text;");
    expect(source).toContain("text: sentText");
    expect(source).toContain("if (revision.now() === sentRevision) setText(\"\");");
    expect(source).toContain("revision.bump();"); // every edit counts, even typing the same text again
  });

  it("clears only the files that went, keeping ones added meanwhile, and stopping clears nothing", () => {
    expect(source).toContain("withoutSent(");
    expect(source).toContain("sentFiles.map((file) => file.id)");
    expect(source).toContain('daemon().call("chat.stop", { botId: bot.id })); // stopping clears nothing');
  });

  it("adds picked files to the attachments as they are now, so a cleared file never comes back", () => {
    expect(source).toContain("attachPaths(latestFiles.current, paths, addFiles,");
    expect(source).toContain("latestFiles.current = next;");
    expect(source).toMatch(/withoutSent\(\s*latestFiles\.current,/);
  });

  it("can't start two sends in the same frame, and never disables the box while one is on its way", () => {
    expect(source).toContain("if (!guard.begin()) return;");
    expect(source).toContain(".finally(guard.end)");
    expect(source).not.toMatch(/<textarea[^>]*disabled/);
  });

  it("caps the box on a phone, as the Team room's is", () => {
    expect(source).toContain("max-sm:max-h-[min(200px,25dvh)]");
  });
});

describe("the exchange's reply box", () => {
  const source = read("../exchange/exchange-view.tsx");

  it("clears only an unchanged reply, and counts every edit", () => {
    expect(source).toContain("if (revision.now() === sentRevision) {");
    expect(source).toContain("${sentText.trim()}");
    expect(source).toContain("revision.bump();");
    expect(source).toContain("!guard.begin()");
  });
});

describe("the routine editor", () => {
  const source = read("../routines/routine-editor.tsx");

  it("closes itself after Save or Delete only while it is still the editor on screen", () => {
    expect(source).toContain("ifAlive(alive, () => close.current());");
    expect(source).toContain("const alive = useAlive();");
  });
});
