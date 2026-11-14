import { createHash } from "node:crypto";
import { resolve } from "node:path";
import type { NativeBrowserBounds } from "../../protocol/src/native-browser.ts";

export function nativeBrowserProfile(root: string, botId: string): string {
  if (!botId || botId.length > 1000) throw new Error("Invalid browser agent identity.");
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
// Renderer CSS pixels become native window coordinates at the app's zoom; keep the caption band protected.
export function nativeBrowserBounds(bounds: NativeBrowserBounds, width: number, height: number, zoom = 1): NativeBrowserBounds {
  const left = Math.min(width, Math.max(0, Math.ceil(bounds.x * zoom)));
  const top = Math.min(height, Math.max(44, Math.ceil(bounds.y * zoom)));
  return { x: left, y: top, width: Math.max(0, Math.min(width, Math.floor((bounds.x + bounds.width) * zoom)) - left), height: Math.max(0, Math.min(height, Math.floor((bounds.y + bounds.height) * zoom)) - top) };
}
