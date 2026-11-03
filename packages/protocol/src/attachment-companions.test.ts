import { expect, it } from "vitest";
import { AttachmentCompanions, attachmentCompanionName } from "./attachments.ts";

it("normalizes explicit companion names without accepting host paths, URLs or traversal", () => {
  expect(attachmentCompanionName("./images/chart%20one.png")).toBe("images/chart one.png");
  for (const name of ["../private.png", "a/../private.png", "%2e%2e/private.png", "a/%2f../private", "/private.png", "//host/file", "C:\\private.png", "https://host/image", "image.png?secret", "a/%252e%252e/file", "a//b", "a\u0000b"]) expect(attachmentCompanionName(name)).toBeNull();
  expect(AttachmentCompanions.safeParse([{ name: "chart.png", attachmentId: "41f8bac7-358c-48cf-9f48-a3a358804da6", attachmentIndex: 1 }]).success).toBe(true);
  expect(AttachmentCompanions.safeParse([1, 2].map((attachmentIndex) => ({ name: "chart.png", attachmentId: "41f8bac7-358c-48cf-9f48-a3a358804da6", attachmentIndex }))).success).toBe(false);
});
