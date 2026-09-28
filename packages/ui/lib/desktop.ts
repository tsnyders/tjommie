export type DesktopConnection = { port: number; token: string };
export type ConnectionResult = ({ ok: true } & DesktopConnection) | { ok: false; error: string };

// The bridge the desktop app's preload script exposes (packages/desktop/src/preload.cjs).
export type DesktopApi = {
  connection(): Promise<ConnectionResult>;
  restartDaemon(): Promise<void>;
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
