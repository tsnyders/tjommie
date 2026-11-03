import { describe, expect, it } from "vitest";
import { ATTACHMENT_CHUNK_BYTES, AttachmentChunk, AttachmentRead, AttachmentRef } from "./attachments.ts";

describe("attachment wire boundaries", () => {
  const transferId = "83c70656-e163-478e-a934-6a5801f5ef18";
  it("accepts only a message-owned reference, never a filesystem or caller context", () => {
    expect(AttachmentRef.parse({ messageId: "message", attachmentIndex: 0 })).toEqual({ messageId: "message", attachmentIndex: 0 });
    for (const extra of [{ path: "C:\\private.txt" }, { connectionId: "owner" }, { deviceId: "owner" }]) expect(AttachmentRef.safeParse({ messageId: "message", attachmentIndex: 0, ...extra }).success).toBe(false);
    expect(AttachmentRef.safeParse({ messageId: "x".repeat(129), attachmentIndex: 0 }).success).toBe(false);
  });
  it("rejects oversized, negative, fractional and unsafe byte ranges", () => {
    for (const patch of [{ length: ATTACHMENT_CHUNK_BYTES + 1 }, { length: 0 }, { offset: -1 }, { offset: 0.5 }, { offset: Number.MAX_SAFE_INTEGER + 1 }]) expect(AttachmentRead.safeParse({ transferId, offset: 0, length: 1, ...patch }).success).toBe(false);
  });
  it("rejects malformed or oversized base64 responses before a native write", () => {
    const chunk = { transferId, revision: "a".repeat(64), offset: 0, bytes: "YWJj", eof: true };
    expect(AttachmentChunk.safeParse(chunk).success).toBe(true);
    for (const bytes of ["abc", "YWJj\n", "!!!!", "A".repeat(4 * Math.ceil(ATTACHMENT_CHUNK_BYTES / 3) + 4)]) expect(AttachmentChunk.safeParse({ ...chunk, bytes }).success).toBe(false);
  });
});
