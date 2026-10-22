// How the browser's picture and the user's pointer relate. Pure, so the viewport's math can be tested without a DOM.
// (mouseButton, wheelPixels, addressToUrl and keyRoute are moved here unchanged from browser-session.ts.)

export type Size = { width: number; height: number };
export type Box = Size & { left: number; top: number };

const MIN = { width: 240, height: 180 };
const MAX = { width: 2560, height: 1600 };

// Where a frame is drawn inside its box with object-contain: centered, scaled to fit, the rest letterbox.
export function containedRect(box: Size, frame: Size): { left: number; top: number; width: number; height: number } {
  if (box.width <= 0 || box.height <= 0 || frame.width <= 0 || frame.height <= 0) return { left: 0, top: 0, width: 0, height: 0 };
  const scale = Math.min(box.width / frame.width, box.height / frame.height);
  const width = frame.width * scale;
  const height = frame.height * scale;
  return { left: (box.width - width) / 2, top: (box.height - height) / 2, width, height };
}

// The page's own (CSS) size a frame shows: the frame says so when its picture was scaled, else the picture's size is it.
export function frameViewport(frame: Size): Size {
  const extra: Size & { viewportWidth?: unknown; viewportHeight?: unknown } = frame;
  const width = typeof extra.viewportWidth === "number" && extra.viewportWidth > 0 ? extra.viewportWidth : frame.width;
  const height = typeof extra.viewportHeight === "number" && extra.viewportHeight > 0 ? extra.viewportHeight : frame.height;
  return { width, height };
}

// A pointer's place on the page, in the page's CSS pixels, or null when it is over the letterbox, outside the drawn picture.
// The picture is drawn contained by its own (encoded) shape; the point is its fraction of the picture, scaled to the page's
// size. It stays under that size (the daemon rejects x >= width and y >= height).
export function framePoint(box: Box, picture: Size, clientX: number, clientY: number, page: Size = picture): { x: number; y: number } | null {
  const drawn = containedRect(box, picture);
  if (drawn.width <= 0 || page.width <= 0 || page.height <= 0) return null;
  const dx = clientX - box.left - drawn.left;
  const dy = clientY - box.top - drawn.top;
  if (dx < 0 || dy < 0 || dx > drawn.width || dy > drawn.height) return null;
  const at = (offset: number, size: number, of: number): number => Math.min(Math.max(Math.ceil(of) - 1, 0), Math.max(0, Math.floor((offset / size) * of)));
  return { x: at(dx, drawn.width, page.width), y: at(dy, drawn.height, page.height) };
}

// Like framePoint, but a pointer outside the drawn picture lands on its nearest edge. For releasing a button that went
// down inside the picture: the release must reach the page even if the pointer left it, or the page keeps it held.
export function framePointClamped(box: Box, picture: Size, clientX: number, clientY: number, page: Size = picture): { x: number; y: number } | null {
  const drawn = containedRect(box, picture);
  if (drawn.width <= 0) return null;
  const x = Math.min(box.left + drawn.left + drawn.width, Math.max(box.left + drawn.left, clientX));
  const y = Math.min(box.top + drawn.top + drawn.height, Math.max(box.top + drawn.top, clientY));
  return framePoint(box, picture, x, y, page);
}

// The size to ask the page for: what the viewport shows, within what the daemon accepts.
export function viewportSize(width: number, height: number): Size {
  return { width: Math.min(MAX.width, Math.max(MIN.width, Math.round(width))), height: Math.min(MAX.height, Math.max(MIN.height, Math.round(height))) };
}

// Whether a new size is worth asking for: drags and window moves produce many near-equal sizes.
export function resizeWorthIt(last: Size | null, next: Size, step = 16): boolean {
  return !last || Math.abs(last.width - next.width) >= step || Math.abs(last.height - next.height) >= step;
}

export function mouseButton(button: number): "left" | "middle" | "right" | null {
  return button === 0 ? "left" : button === 1 ? "middle" : button === 2 ? "right" : null;
}

// Wheel deltas in pixels: lines and pages become pixels, and the daemon's limit holds.
export function wheelPixels(delta: number, mode: number): number {
  const pixels = mode === 1 ? delta * 16 : mode === 2 ? delta * 400 : delta;
  return Math.max(-10_000, Math.min(10_000, Math.round(pixels)));
}

// What the address bar means: a full address as typed, a host with https (http for this computer), anything else a search.
export function addressToUrl(text: string): string {
  const value = text.trim();
  if (/^(https?|about|file):/i.test(value)) return value;
  if (/^(localhost|127\.0\.0\.1|\[::1\])(:\d+)?([/?#]|$)/i.test(value)) return `http://${value}`;
  if (!/\s/.test(value) && /^[^\s/]+\.[^\s/]{2,}([/?#:]|$)/.test(value)) return `https://${value}`;
  return `https://duckduckgo.com/?q=${encodeURIComponent(value)}`;
}

// What a key in the viewport does: "leave" is Shift+Tab, which returns focus to the panel's controls instead of the page;
// "paste" waits for the paste event, which carries the text; everything else goes to the page.
export function keyRoute(event: { key: string; shiftKey: boolean; ctrlKey: boolean; metaKey: boolean }): "leave" | "paste" | "send" {
  if (event.key === "Tab" && event.shiftKey) return "leave";
  if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "v") return "paste";
  return "send";
}

// True on Google's "couldn't sign you in" page, where the notice about automated browsers applies.
export function googleSignInRejected(url: string): boolean {
  try {
    const page = new URL(url);
    return page.hostname === "accounts.google.com" && /\/signin\/rejected/i.test(page.pathname);
  } catch {
    return false;
  }
}

type Shown = { complete: boolean; naturalWidth: number; naturalHeight: number; currentSrc: string };

// Whether the picture on screen is really this frame, decoded: a press or wheel is mapped through a frame's size, so it must
// not be taken while an older picture (or none yet) is what the user sees.
export function imageReady(image: Shown, frame: Size, src: string): boolean {
  return image.complete && image.naturalWidth === frame.width && image.naturalHeight === frame.height && image.currentSrc === src;
}

// What a picture is of: the browser session, page and layout (and stream, when the daemon names them). A new one remounts the
// image, so nothing of the old layout stays on screen or answers clicks. Ordinary frames of the same layout share a key and reuse
// the picture.
export function pictureKey(frame: { sequence: number }, state: { generation: string; activePageId: string | null }): string {
  const extra: { sequence: number; pageId?: unknown; viewportRevision?: unknown; streamId?: unknown } = frame;
  const part = (value: unknown): string => (typeof value === "string" || typeof value === "number" ? String(value) : "");
  return [state.generation, part(extra.pageId) || state.activePageId || "", part(extra.viewportRevision), part(extra.streamId)].join("|");
}
