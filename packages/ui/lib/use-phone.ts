"use client";

import { useSyncExternalStore } from "react";
import { PHONE_BELOW } from "./panel-layout";

const QUERY = `(max-width: ${PHONE_BELOW - 1}px)`;

function subscribe(listener: () => void): () => void {
  const media = window.matchMedia(QUERY);
  media.addEventListener("change", listener);
  return () => media.removeEventListener("change", listener);
}

// Whether the window is phone-sized (narrower than 640 px). Layout is CSS; this is only for what behavior changes: the home redirect, the panel's
// default, and which controls show. The server's answer is no, so a page that renders before it is measured starts as the desktop one.
export function usePhone(): boolean {
  return useSyncExternalStore(subscribe, () => window.matchMedia(QUERY).matches, () => false);
}
