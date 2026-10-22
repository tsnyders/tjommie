import type { BrowserState, GoogleService, NativeBrowserBounds, NativeBrowserMount, NativeBrowserUiCommand } from "@tjommie/protocol";
// The native browser (WebContentsView). The page is drawn and takes input in the desktop app itself, not in the renderer: the
// renderer only says where it goes (mount) and what the user did with the controls. Every call is for one bot's browser, whose
// context (cookies, tabs, sign-ins) lives on in the main process while the panel is hidden. The state is the protocol's
// BrowserState; page-level commands carry its generation back, so a command for an old page is refused.
export type NativeBrowserRect = NativeBrowserBounds; // CSS pixels in the window's content
export type NativeBrowserCommand = NativeBrowserUiCommand;
export type NativeBrowserApi = {
  command(botId: string, command: NativeBrowserCommand): Promise<BrowserState>;
  // Puts the active tab's view at these bounds and shows it (visible), or hides it, keeping the context. The desktop app hides
  // every view while an app dialog's scrim is up (setDialogScrim), and the renderer hides it for a menu or list.
  mount(botId: string, mount: NativeBrowserMount): Promise<BrowserState>;
  onState(listener: (state: BrowserState) => void): () => void; // every change, for any bot
  onFocusReturn?(listener: (botId: string) => void): () => void; // the page let go of the keyboard (Shift+Tab), if the app reports it
};

export type DesktopConnection = { port: number; token: string };
export type ConnectionResult = ({ ok: true } & DesktopConnection) | { ok: false; error: string };

// The bridge the desktop app's preload script exposes (packages/desktop/src/preload.cjs).
export type DesktopApi = {
  connection(): Promise<ConnectionResult>;
  restartDaemon(): Promise<void>;
  nativeBrowser: NativeBrowserApi;
  connectGoogle(services: GoogleService[]): Promise<{ attempt: string }>;
  onConnection(listener: (connection: DesktopConnection) => void): () => void;
  onDaemonFailed(listener: (reason: string) => void): () => void;
  onNavigate(listener: (path: string) => void): () => void;
  pickFiles(): Promise<string[]>;
  pathForFile(file: File): string;
  showFile(path: string): void;
  openFolder(path: string): Promise<void>;
  pickFolder(): Promise<string | null>;
  openNotes(homeFolder: string): Promise<void>;
  setTheme(theme: "dark" | "light"): void;
  setDialogScrim(open: boolean): void; // dims the caption buttons while a dialog's scrim covers the page
  info(): Promise<{ username: string }>;
};

declare global {
  interface Window {
    tjommie?: DesktopApi;
  }
}

// The desktop app's bridge, or null when the page runs anywhere else.
export function desktop(): DesktopApi | null {
  return typeof window === "undefined" ? null : (window.tjommie ?? null);
}
