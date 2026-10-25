import { desktop } from "./desktop";
import { getState } from "./store";

// Whether this client can show the bot's browser. That is the desktop app's native bridge, never the window's width: a phone-sized desktop window
// has it and works, a web or phone client doesn't and says the browser runs on the PC. So does the desktop app while it's on another PC, whose
// bots' browsers run on that PC.
export const nativeBrowserAvailable = (): boolean => desktop()?.nativeBrowser !== undefined && getState().host === null;

export const localFileActions = () => getState().host === null ? desktop() : null;

// The room the desktop window's caption buttons take at the top right (Windows draws them over 138 x 44 px), in every width; a client without the
// desktop app has none.
export const captionClear = (): string => (desktop() ? "pr-[146px]" : "pr-4");

// The same for a phone-width header, where the clearance only applies in the desktop window (a web or phone client has none).
export const phoneCaptionClear = (): string => (desktop() ? "max-sm:pr-[146px]" : "max-sm:pr-4");
