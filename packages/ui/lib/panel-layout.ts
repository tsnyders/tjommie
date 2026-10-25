// The right panel's width rules: bounded, remembered, and an overlay when the content area is narrow.

export type Pane = "details" | "browser";
export const PANES: Record<Pane, { fallback: number; min: number }> = { details: { fallback: 460, min: 400 }, browser: { fallback: 560, min: 420 } };
export const CHAT_MIN = 480; // the chat always keeps this much beside the panel
export const OVERLAY_BELOW = 1000; // a narrower content area gives the panel the chat's place, over it
const OVERLAY_EDGE = 48; // the strip of chat left showing beside an overlay
const KEY_STEP = 16;
const KEY_STEP_BIG = 64;

export const PHONE_BELOW = 640; // a viewport narrower than this is a phone (Tailwind's sm); the panel is then a full-screen sheet

export const isPhone = (viewportWidth: number): boolean => viewportWidth > 0 && viewportWidth < PHONE_BELOW;

export function isOverlay(room: number): boolean {
  return room > 0 && room < OVERLAY_BELOW;
}

// The smallest and largest width the panel may have, given the room (the content area's width; 0 until measured).
export function widthBounds(pane: Pane, room: number): { min: number; max: number } {
  const { min } = PANES[pane];
  const max = room <= 0 ? 900 : isOverlay(room) ? room - OVERLAY_EDGE : room - CHAT_MIN;
  return { min, max: Math.max(min, max) };
}

export function clampWidth(width: number, bounds: { min: number; max: number }): number {
  return Math.min(bounds.max, Math.max(bounds.min, Math.round(width)));
}

// The separator sits on the panel's left edge: Left makes the panel wider, Right narrower. Null for other keys.
export function splitterWidth(key: string, shift: boolean, width: number, bounds: { min: number; max: number }): number | null {
  const step = shift ? KEY_STEP_BIG : KEY_STEP;
  if (key === "ArrowLeft") return clampWidth(width + step, bounds);
  if (key === "ArrowRight") return clampWidth(width - step, bounds);
  if (key === "Home") return bounds.min;
  if (key === "End") return bounds.max;
  return null;
}

const widthKey = (pane: Pane): string => `tjommie.panel-width.${pane}`;

export function readWidth(storage: Pick<Storage, "getItem">, pane: Pane): number {
  const stored = Number(storage.getItem(widthKey(pane)));
  return Number.isFinite(stored) && stored > 0 ? stored : PANES[pane].fallback;
}

export function saveWidth(storage: Pick<Storage, "setItem">, pane: Pane, width: number): void {
  storage.setItem(widthKey(pane), String(Math.round(width)));
}

// Which pane the right panel shows for one bot, and whether the browser fills the content area. Another bot starts at Details.
export type PanelView = { bot: string; pane: Pane; expanded: boolean };

export const panelFor = (view: PanelView, bot: string): PanelView => (view.bot === bot ? view : { bot, pane: "details", expanded: false });

// Each change starts from the latest view (pass these to a state setter), so two in a row can't undo each other. Leaving
// the browser always docks it.
export const choosePane = (view: PanelView, bot: string, pane: Pane): PanelView => ({ ...panelFor(view, bot), pane, expanded: pane === "browser" ? panelFor(view, bot).expanded : false });
export const setExpanded = (view: PanelView, bot: string, expanded: boolean): PanelView => ({ ...panelFor(view, bot), expanded });

// The chat is covered (and so inert) only by what is really showing: an overlay, or an expanded browser the bot still has.
export function coversChat(shown: boolean, overlay: boolean, view: PanelView, browser: boolean): boolean {
  return shown && (overlay || (browser && view.pane === "browser" && view.expanded));
}
