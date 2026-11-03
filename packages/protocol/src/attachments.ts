import { z } from "zod";

export const MAX_ATTACHMENT_BYTES = 200 * 1024 * 1024;
export const ATTACHMENT_CHUNK_BYTES = 256 * 1024;
export const MAX_ATTACHMENT_COMPANIONS = 64;
export function attachmentCompanionName(source: string): string | null {
  if (!source || source.length > 1024) return null;
  let name: string;
  try { name = decodeURIComponent(source).normalize("NFC"); } catch { return null; }
  while (name.startsWith("./")) name = name.slice(2);
  if (/[\\\u0000-\u001f\u007f%?#:]/.test(name) || name.split("/").some((part) => !part || part === "." || part === "..")) return null;
  return name || null;
}
export const AttachmentCompanionName = z.string().min(1).max(1024).transform((value, context) => {
  const name = attachmentCompanionName(value);
  if (name !== null) return name;
  context.addIssue({ code: "custom", message: "Use a relative companion name without traversal or a URL." }); return z.NEVER;
});
export const AttachmentCompanion = z.strictObject({ name: AttachmentCompanionName, attachmentId: z.uuid(), attachmentIndex: z.number().int().positive() });
export type AttachmentCompanion = z.infer<typeof AttachmentCompanion>;
export const AttachmentCompanions = z.array(AttachmentCompanion).max(MAX_ATTACHMENT_COMPANIONS).refine((items) => new Set(items.map((item) => item.name)).size === items.length, "Companion names must be unique.");
export const AttachmentRef = z.strictObject({ messageId: z.string().min(1).max(128), attachmentIndex: z.number().int().nonnegative(), attachmentId: z.string().min(1).max(128).optional() });
export type AttachmentRef = z.infer<typeof AttachmentRef>;
export const AttachmentInfo = z.strictObject({
  id: z.uuid(), hostId: z.string().min(1).max(128), name: z.string().min(1).max(255), size: z.number().int().min(0).max(MAX_ATTACHMENT_BYTES),
  contentType: z.string().min(1).max(127), sha256: z.string().regex(/^[a-f0-9]{64}$/),
});
export type AttachmentInfo = z.infer<typeof AttachmentInfo>;
export const AttachmentOpen = z.strictObject({ transferId: z.uuid(), attachment: AttachmentInfo, chunkBytes: z.literal(ATTACHMENT_CHUNK_BYTES), expiresAt: z.iso.datetime() });
export type AttachmentOpen = z.infer<typeof AttachmentOpen>;
export const AttachmentRead = z.strictObject({ transferId: z.uuid(), offset: z.number().int().min(0).max(MAX_ATTACHMENT_BYTES), length: z.number().int().min(1).max(ATTACHMENT_CHUNK_BYTES) });
export const AttachmentChunk = z.strictObject({
  transferId: z.uuid(), revision: AttachmentInfo.shape.sha256, offset: z.number().int().min(0).max(MAX_ATTACHMENT_BYTES),
  bytes: z.string().max(4 * Math.ceil(ATTACHMENT_CHUNK_BYTES / 3)).regex(/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/), eof: z.boolean(),
});
export type AttachmentChunk = z.infer<typeof AttachmentChunk>;
export const attachmentMethods = {
  "attachments.metadata": { params: AttachmentRef, result: AttachmentInfo },
  "attachments.open": { params: AttachmentRef, result: AttachmentOpen },
  "attachments.read": { params: AttachmentRead, result: AttachmentChunk },
  "attachments.close": { params: z.strictObject({ transferId: z.uuid() }), result: z.strictObject({}) },
} as const;

export type AttachmentNativeRead = { previewId: string; requestId: string; offset: number; length: number };
export type AttachmentNativeApi = {
  startSave(info: AttachmentInfo): Promise<{ saveId: string } | null>;
  appendSave(saveId: string, offset: number, bytes: string): Promise<void>;
  finishSave(saveId: string): Promise<{ status: "saved" | "cancelled" }>;
  cancelSave(saveId: string): Promise<void>;
  preview(info: AttachmentInfo): Promise<{ previewId: string; url: string }>;
  onRead(listener: (request: AttachmentNativeRead) => void): () => void;
  respond(previewId: string, requestId: string, bytes: string | null): Promise<void>;
  closePreview(previewId: string): Promise<void>;
  invalidate(): Promise<void>;
};
