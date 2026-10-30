import { desktop, type DesktopApi } from "./desktop";
import { remoteHosts } from "./remote-hosts";
import { getState, useApp } from "./store";

// Whether this client can show the bot's browser. That is the desktop app's native bridge, never the window's width: a phone-sized desktop window
// has it and works, a web or phone client doesn't and says the browser runs on the PC. So does the desktop app while it's on another PC, whose
// bots' browsers run on that PC.
export const nativeBrowserAvailable = (): boolean => desktop()?.nativeBrowser !== undefined && getState().host === null;

// The desktop app's bridge while the window shows this PC, for what acts on this PC: its files and folders, its folder picker and its Google
// sign-in. While it shows another PC there is none, as those are that PC's.
export const localFileActions = () => getState().host === null ? desktop() : null;

// The same two gates for a component's render, which runs again the moment the window moves to another PC or back. A click checks again.
export function useNativeBrowserAvailable(): boolean {
  useApp((state) => state.host);
  return nativeBrowserAvailable();
}

export function useLocalFileActions(): DesktopApi | null {
  useApp((state) => state.host);
  return localFileActions();
}

// The PC the window shows, named when this device knows it, for the cards that say something runs there and not here.
export const pcName = (host: string | null): string => remoteHosts().find((saved) => saved.id === host)?.name ?? "your PC";

// The room the desktop window's caption buttons take at the top right (Windows draws them over 138 x 44 px), in every width; a client without the
// desktop app has none.
export const captionClear = (): string => (desktop() ? "pr-[146px]" : "pr-4");

// The same for a phone-width header, where the clearance only applies in the desktop window (a web or phone client has none).
export const phoneCaptionClear = (): string => (desktop() ? "max-sm:pr-[146px]" : "max-sm:pr-4");
