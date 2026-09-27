import { describe, expect, it } from "vitest";
import { approvalDetail } from "./approval";

describe("approvalDetail", () => {
  it("picks the path, command or address out of Claude's tool input", () => {
    const path = "C:\\Users\\tyrel\\Documents\\Job applications\\CV-Sabenza-Angular.pdf";
    expect(approvalDetail(JSON.stringify({ file_path: path, content: "..." }, null, 2))).toBe(path);
    expect(approvalDetail(JSON.stringify({ command: "npm test", description: "Run the tests" }, null, 2))).toBe("npm test");
    expect(approvalDetail(JSON.stringify({ url: "https://pnet.co.za/jobs" }, null, 2))).toBe("https://pnet.co.za/jobs");
  });

  it("keeps Codex's command text on one line, and shortens very long details", () => {
    expect(approvalDetail("Set-Content -LiteralPath 'notes.txt'\n  -Value 'hi'")).toBe("Set-Content -LiteralPath 'notes.txt' -Value 'hi'");
    expect(approvalDetail("x".repeat(500))).toHaveLength(240);
  });
});
