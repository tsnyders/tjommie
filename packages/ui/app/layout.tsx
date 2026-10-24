import type { Metadata, Viewport } from "next";
import { AppShell } from "@/components/app-shell";
import { themeScript } from "@/lib/theme";
import "./globals.css";

export const metadata: Metadata = {
  title: "Tjommie",
};

// "cover" lets the Android app's edge-to-edge system bars hand their insets to CSS (env(safe-area-inset-*)); the shell pads for them.
export const viewport: Viewport = { width: "device-width", initialScale: 1, viewportFit: "cover" };

export default function RootLayout({ children }: LayoutProps<"/">) {
  return (
    <html lang="en" suppressHydrationWarning>
      <head>
        <script dangerouslySetInnerHTML={{ __html: themeScript }} />
      </head>
      <body>
        <AppShell>{children}</AppShell>
      </body>
    </html>
  );
}
