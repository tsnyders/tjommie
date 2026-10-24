"use client";

import { Suspense, useEffect, useSyncExternalStore, type ReactNode } from "react";
import { usePathname, useRouter } from "next/navigation";
import { Loader } from "lucide-react";
import { startConnection } from "@/lib/connection";
import { desktop } from "@/lib/desktop";
import { isAndroidApp } from "@/lib/mobile";
import { showsTabBar } from "@/lib/phone-nav";
import { useApp } from "@/lib/store";
import { followTheme } from "@/lib/theme";
import { useKeyboardVisible } from "@/lib/use-keyboard";
import { useSheetOpen } from "@/lib/use-sheet";
import { cn } from "@/lib/utils";
import { ConnectionScreen } from "./connection-screen";
import { Rail } from "./rail";
import { TabBar } from "./tab-bar";

export function AppShell({ children }: { children: ReactNode }) {
  const router = useRouter();
  const pathname = usePathname();
  // While the phone's floating nav shows, the overview scrollers end with room to clear it (data-nav-clear in globals.css).
  const keyboard = useKeyboardVisible();
  const sheet = useSheetOpen();
  const nav = showsTabBar(pathname) && !keyboard && !sheet;
  const connection = useApp((state) => state.connection);
  // Only the Android app pads for the system bars and the cutout; the desktop window and the web are unchanged. The server's answer is no.
  const android = useSyncExternalStore(
    () => () => {},
    isAndroidApp,
    () => false,
  );
  useEffect(() => startConnection(), []);
  useEffect(() => followTheme(), []);
  useEffect(() => desktop()?.onNavigate((path) => router.push(path)), [router]);
  return (
    <div data-nav={nav || undefined} className={cn("flex h-dvh max-sm:flex-col", android && "android-app")}>
      <Suspense fallback={<div className="w-[88px] shrink-0 border-r border-rail-edge bg-rail max-sm:hidden" />}>
        <Rail />
      </Suspense>
      <main className="relative flex min-h-0 min-w-0 flex-1 flex-col">
        {connection.status === "ready" || connection.status === "reconnecting" ? children : <ConnectionScreen connection={connection} />}
        {connection.status === "reconnecting" && (
          <div role="status" className="absolute left-1/2 top-14 z-10 flex -translate-x-1/2 items-center gap-2 rounded-full bg-popover px-3 py-1.5 text-[12px] text-secondary">
            <Loader className="size-3.5 animate-spin" aria-hidden />
            Reconnecting to Tjommie
          </div>
        )}
      </main>
      <TabBar />
    </div>
  );
}
