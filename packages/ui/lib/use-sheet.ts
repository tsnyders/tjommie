"use client";

import { useEffect, useRef, useSyncExternalStore, type KeyboardEvent } from "react";

// Whether an Escape closes a phone's full-screen sheet: only on a phone, only if nothing took the key already, and not when something inside is
// using Escape (a typed address, the page itself: `[data-keeps-escape]`) or a page dialog is open.
export function escapeCloses(event: { key: string; defaultPrevented: boolean }, phone: boolean, keepsEscape: boolean, pageDialogOpen: boolean): boolean {
  return phone && event.key === "Escape" && !event.defaultPrevented && !keepsEscape && !pageDialogOpen;
}

// The Android app tells the page that Back was pressed by dispatching this cancelable event on window. A sheet that is open takes it (preventDefault) and
// closes; if nothing does, the app goes back through the history as it always did.
export const BACK_EVENT = "tjommie:back";
const backHandlers: (() => void)[] = [];
const dispatchBack = (event: Event): void => {
  const handler = backHandlers.at(-1);
  if (handler) handleBack(event, handler);
};
// Every phone sheet registers here while it is open, so this also tells whether one is (the floating nav hides under a sheet).
const sheetWatchers = new Set<() => void>();
const sheetsChanged = (): void => sheetWatchers.forEach((watcher) => watcher());
export function registerSheetBack(handler: () => void): () => void {
  if (!backHandlers.length) window.addEventListener(BACK_EVENT, dispatchBack);
  backHandlers.push(handler);
  sheetsChanged();
  return () => {
    const index = backHandlers.lastIndexOf(handler);
    if (index >= 0) backHandlers.splice(index, 1);
    if (!backHandlers.length) window.removeEventListener(BACK_EVENT, dispatchBack);
    sheetsChanged();
  };
}

export const sheetOpen = (): boolean => backHandlers.length > 0;
const watchSheets = (onChange: () => void): (() => void) => {
  sheetWatchers.add(onChange);
  return () => sheetWatchers.delete(onChange);
};
export function useSheetOpen(): boolean {
  return useSyncExternalStore(watchSheets, sheetOpen, () => false);
}

export function handleBack(event: Event, onClose: () => void): void {
  if (event.defaultPrevented) return; // another sheet, on top, already took it
  event.preventDefault();
  onClose();
}

// A phone's sheet (the exchange, the Team room's members): focus moves into it when it opens, to the control named by `first`, and Escape closes it.
// It is a plain element, not a dialog, so nothing hides a native browser behind it. Returning focus to what opened it is the opener's job, once the
// page behind is no longer inert (after the state change has been drawn).
export function useSheet<T extends HTMLElement>(phone: boolean, onClose: () => void, first: string): { ref: React.RefObject<T | null>; onKeyDown: (event: KeyboardEvent) => void } {
  const ref = useRef<T>(null);
  useEffect(() => {
    if (phone) ref.current?.querySelector<HTMLElement>(first)?.focus();
  }, [phone, first]);
  // Android's Back button, while this sheet is open on a phone. onClose is read through a ref, so the listener isn't remade on every render.
  const close = useRef(onClose);
  useEffect(() => {
    close.current = onClose;
  });
  useEffect(() => {
    if (!phone) return;
    return registerSheetBack(() => close.current());
  }, [phone]);
  const onKeyDown = (event: KeyboardEvent): void => {
    const keeps = (event.target as HTMLElement).closest("[data-keeps-escape]") !== null;
    if (escapeCloses(event, phone, keeps, ref.current?.querySelector('[role="alertdialog"]') != null)) onClose();
  };
  return { ref, onKeyDown };
}
