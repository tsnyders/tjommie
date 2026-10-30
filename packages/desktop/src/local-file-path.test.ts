import { expect, it } from "vitest";
import { localFilePath } from "./local-file-path.ts";

it("rejects UNC and device paths before any filesystem lookup", () => {
  expect(localFilePath("C:\\Users\\Alice\\file.txt")).toBe(true);
  for (const path of ["\\\\attacker\\share\\file.txt", "\\\\?\\C:\\Windows\\file.txt", "\\\\.\\C:\\file.txt", "//attacker/share/file.txt", "relative\\file.txt"]) {
    expect(localFilePath(path)).toBe(false);
  }
});
