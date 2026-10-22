import { createHash } from "node:crypto";
import { resolve } from "node:path";
import type { NativeBrowserBounds } from "../../protocol/src/native-browser.ts";

export function nativeBrowserProfile(root: string, botId: string): string {
  if (!botId || botId.length > 1000) throw new Error("Invalid browser bot identity.");
  return resolve(root, createHash("sha256").update(botId).digest("hex"));
}
export function nativeBrowserUrl(value: string): string {
  const url = new URL(value);
  if (!(url.protocol === "http:" || url.protocol === "https:" || url.href === "about:blank")) throw new Error("Browser navigation only supports HTTP, HTTPS and about:blank.");
  if (url.username || url.password) throw new Error("Browser addresses cannot contain credentials.");
  return url.href;
}
// Child documents need ordinary web-created frames; none may navigate into app/OS privileges.
export function nativeBrowserFrameUrl(value: string, isMainFrame: boolean): string {
  const url = new URL(value);
  if (!isMainFrame && (url.href === "about:srcdoc" || url.protocol === "blob:" || url.protocol === "data:")) return url.href;
  return nativeBrowserUrl(value);
}
// Rectangles belong to the owning window's content coordinates. Preserve its caption/toolbar band.
export function nativeBrowserBounds(bounds: NativeBrowserBounds, width: number, height: number): NativeBrowserBounds {
  const left = Math.min(width, Math.max(0, Math.ceil(bounds.x)));
  const top = Math.min(height, Math.max(44, Math.ceil(bounds.y)));
  return { x: left, y: top, width: Math.max(0, Math.min(width, Math.floor(bounds.x + bounds.width)) - left), height: Math.max(0, Math.min(height, Math.floor(bounds.y + bounds.height)) - top) };
}
