import type { NativeBrowserRect } from "./desktop";

// The native page is a view over the whole window, above anything the renderer draws. So wherever the renderer puts something
// over the page's slot, the page has to be hidden: an open menu, a list, a popover, or a dialog (the desktop app also hides every
// view while a dialog's scrim is up). The browser's own page dialog is not one: it sits above the slot, not over it.
export const APP_OVERLAY = '[role="menu"], [role="listbox"], [role="dialog"], [role="alertdialog"]:not([data-page-dialog])';

export function coveredByAppOverlay(page: { visibilityState: string; querySelector(selector: string): unknown }): boolean {
  return page.visibilityState !== "visible" || page.querySelector(APP_OVERLAY) !== null;
}

const MAX = 32768; // what the desktop app accepts

// Where to mount the page for a slot's rectangle: whole CSS pixels, inside what the desktop app accepts, or null (hidden) when
// the slot has no size, is covered, or lies off the window's top-left edge.
export function slotBounds(rect: { x: number; y: number; width: number; height: number } | undefined, covered: boolean): NativeBrowserRect | null {
  if (!rect || covered || rect.width < 1 || rect.height < 1) return null;
  const at = (value: number): number => Math.min(MAX, Math.max(0, Math.round(value)));
  const x = at(rect.x);
  const y = at(rect.y);
  return { x, y, width: Math.min(MAX - x, at(rect.width)), height: Math.min(MAX - y, at(rect.height)) };
}
