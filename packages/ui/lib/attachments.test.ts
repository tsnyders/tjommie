import { describe, expect, it } from "vitest";
import { addFiles, fileName, MAX_FILES } from "./attachments";

describe("attachments", () => {
  it("adds picked or dropped files once each, and skips dropped items that aren't files", () => {
    expect(addFiles(["C:\\a.pdf"], ["C:\\b.pdf", "C:\\a.pdf", ""])).toEqual(["C:\\a.pdf", "C:\\b.pdf"]);
  });

  it("refuses to go over the daemon's limit of 20 files", () => {
    const twenty = Array.from({ length: MAX_FILES }, (_, index) => `C:\\f${index}.txt`);
    expect(addFiles(twenty.slice(0, 19), ["C:\\f19.txt"])).toEqual(twenty);
    expect(addFiles(twenty, ["C:\\extra.txt"])).toBeNull();
  });

  it("shows a file's name from its Windows path", () => {
    expect(fileName("C:\\Users\\tyrel\\Documents\\CV final.pdf")).toBe("CV final.pdf");
  });
});
