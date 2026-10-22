import { describe, expect, it, vi } from "vitest";
import { gmailMessage, googleTools } from "./google-tools.ts";

describe("read-only Gmail tools", () => {
  it("exposes no mail-writing tools and keeps the calling bot's identity", async () => {
    const request = vi.fn(async () => ({ messages: [{ id: "abc123" }] }));
    const tools = googleTools({ request });
    expect(tools.map((tool) => tool.name)).toEqual(["google_gmail_search", "google_gmail_read", "google_gmail_labels"]);
    const search = tools[0];
    await search.run("bot-one", search.input.parse({ query: "in:inbox from:fixture@example.invalid&access_token=not-a-token" }));
    expect(request).toHaveBeenCalledOnce();
    const call = request.mock.calls[0] as unknown as [string, string, string];
    expect(call.slice(0, 2)).toEqual(["bot-one", "gmail"]);
    const url = new URL(call[2], "https://gmail.googleapis.com");
    expect(url.pathname).toBe("/gmail/v1/users/me/messages");
    expect(url.searchParams.get("q")).toBe("in:inbox from:fixture@example.invalid&access_token=not-a-token");
    expect(url.searchParams.has("access_token")).toBe(false);
  });

  it("rejects path-like message IDs before reaching the backend", () => {
    const read = googleTools({ request: vi.fn() })[1];
    expect(read.input.safeParse({ messageId: "../other-user/messages" }).success).toBe(false);
  });

  it("propagates denied access rather than reading another bot's account", async () => {
    const search = googleTools({ request: async () => { throw new Error("Google access has not been granted to this bot."); } })[0];
    await expect(search.run("unapproved-bot", search.input.parse({ query: "in:inbox" }))).rejects.toThrow("not been granted");
  });

  it("extracts bounded plain text and headers, excluding HTML and attachments", () => {
    const result = gmailMessage({ id: "123", payload: { headers: [{ name: "Subject", value: "Fixture" }], parts: [
      { mimeType: "text/plain", body: { data: Buffer.from("A synthetic note").toString("base64url") } },
      { mimeType: "text/html", body: { data: Buffer.from("<script>unsafe()</script>").toString("base64url") } },
      { mimeType: "text/plain", filename: "private.txt", body: { data: Buffer.from("attachment").toString("base64url") } },
    ] } });
    expect(result).toMatchObject({ subject: "Fixture", text: "A synthetic note", contentTruncated: false });
    expect(JSON.stringify(result)).not.toContain("unsafe()");
    expect(result.contentNotice).toContain("Untrusted");
  });

  it("caps large bodies and adversarial nesting", () => {
    expect(gmailMessage({ id: "123", payload: { mimeType: "text/plain", body: { data: Buffer.from("a".repeat(40_000)).toString("base64url") } } })).toMatchObject({ text: "a".repeat(32_000), contentTruncated: true });
    let payload: Record<string, unknown> = { mimeType: "text/plain", body: { data: Buffer.from("deep").toString("base64url") } };
    for (let i = 0; i < 15; i++) payload = { parts: [payload] };
    expect(gmailMessage({ id: "123", payload })).toMatchObject({ text: "", contentTruncated: true });
  });
});
