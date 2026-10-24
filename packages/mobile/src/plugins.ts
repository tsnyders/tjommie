import { registerPlugin } from "@capacitor/core";
import { createPlugins } from "./wrappers.ts";

// The native plugins, registered with Capacitor. Import this only where the app runs inside Capacitor (the web and desktop builds never load it).
export const { secureStore, socket, scanner } = createPlugins((name) => registerPlugin(name));

export type AppearanceInsets = { imeBottom: number; keyboardVisible: boolean };
export const appearance = registerPlugin<{
  setCanvas(options: { color: string; dark: boolean }): Promise<void>;
  getInsets(): Promise<AppearanceInsets>;
}>("TjommieAppearance");
