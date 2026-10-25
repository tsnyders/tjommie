"use client";

import { useSyncExternalStore } from "react";

// Whether the on-screen keyboard is up, so the floating nav can step aside. In the Android app the native side reports it (the "tjommie:insets"
// event, with a window.tjommieInsets snapshot); it already resizes the page for the keyboard, so nothing here pads for it. In a phone browser the
// visual viewport shrinking well below the window stands in.
export type Insets = { imeBottom: number; keyboardVisible: boolean };
export const INSETS_EVENT = "tjommie:insets";

declare global {
  interface Window {
    tjommieInsets?: Insets;
  }
}

// A browser keyboard takes far more than this; smaller changes are the address bar moving.
const KEYBOARD_MIN = 150;

export function keyboardVisible(): boolean {
  if (typeof window === "undefined") return false;
  if (window.tjommieInsets) return window.tjommieInsets.keyboardVisible;
  const view = window.visualViewport;
  return !!view && window.innerHeight - view.height > KEYBOARD_MIN;
}

function subscribe(onChange: () => void): () => void {
  window.addEventListener(INSETS_EVENT, onChange);
  window.visualViewport?.addEventListener("resize", onChange);
  return () => {
    window.removeEventListener(INSETS_EVENT, onChange);
    window.visualViewport?.removeEventListener("resize", onChange);
  };
}

export function useKeyboardVisible(): boolean {
  return useSyncExternalStore(subscribe, keyboardVisible, () => false);
}
