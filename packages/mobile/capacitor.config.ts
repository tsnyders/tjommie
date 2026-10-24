import type { CapacitorConfig } from "@capacitor/cli";

// The Android app wraps the UI's static export. The page is served from the app's own origin; nothing is loaded from a network address, and
// mixed content stays blocked. The connection to a PC is not the web view's: it is the native socket plugin's, carrying encrypted frames.
const config: CapacitorConfig = {
  appId: "dev.tjommie.app",
  appName: "Tjommie",
  webDir: "../ui/out",
  server: { androidScheme: "https" },
  android: { allowMixedContent: false, backgroundColor: "#070707" },
  plugins: { SystemBars: { insetsHandling: "css", initialViewportFitValueHint: "cover", style: "DARK" } },
};

export default config;
