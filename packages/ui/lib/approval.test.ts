import { describe, expect, it } from "vitest";
import { approvalDetail, approvalRule } from "./approval";

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

describe("approvalRule", () => {
  it("says what Always allow saves in words, including Codex's command prefixes and tools", () => {
    expect(approvalRule("Write")).toBe("Write");
    expect(approvalRule("Bash(npm test:*)")).toBe("Bash(npm test:*)");
    expect(approvalRule('codex:["Set-Content","-LiteralPath","notes.txt","-Value","hi"]')).toBe("Set-Content -LiteralPath notes.txt -Value hi");
    expect(approvalRule("codex-mcp:browser.browser_navigate")).toBe("browser.browser_navigate");
    expect(approvalRule('codex-edit:["C:\\\\bots\\\\writer","C:\\\\project"]')).toBe("Edit files in its folders");
  });
});
