import { useSyncExternalStore } from "react";
import { initialState, type AppState } from "./state";

let state = initialState;
const listeners = new Set<() => void>();

export function getState(): AppState {
  return state;
}

export function update(change: (current: AppState) => AppState): void {
  const next = change(state);
  if (next === state) return;
  state = next;
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

// Reads part of the app state. The selector must return something that's already in the state (never a new
// array or object), or React re-renders forever; derive new values with useMemo in the component.
export function useApp<T>(select: (current: AppState) => T): T {
  return useSyncExternalStore(subscribe, () => select(state), () => select(initialState));
}
