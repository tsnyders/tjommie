"use client";

import { Suspense, useEffect, type ReactNode } from "react";
import { useRouter } from "next/navigation";
import { Loader } from "lucide-react";
import { startConnection } from "@/lib/connection";
import { desktop } from "@/lib/desktop";
import { useApp } from "@/lib/store";
import { followTheme } from "@/lib/theme";
import { ConnectionScreen } from "./connection-screen";
import { Rail } from "./rail";

export function AppShell({ children }: { children: ReactNode }) {
  const router = useRouter();
  const connection = useApp((state) => state.connection);
  useEffect(() => startConnection(), []);
  useEffect(() => followTheme(), []);
  useEffect(() => desktop()?.onNavigate((path) => router.push(path)), [router]);
  return (
    <div className="flex h-screen">
      <Suspense fallback={<div className="w-[88px] shrink-0 border-r border-rail-edge bg-rail" />}>
        <Rail />
      </Suspense>
      <main className="relative flex min-w-0 flex-1 flex-col">
        {connection.status === "ready" || connection.status === "reconnecting" ? children : <ConnectionScreen connection={connection} />}
        {connection.status === "reconnecting" && (
          <div role="status" className="absolute left-1/2 top-14 z-10 flex -translate-x-1/2 items-center gap-2 rounded-full bg-popover px-3 py-1.5 text-[12px] text-secondary">
            <Loader className="size-3.5 animate-spin" aria-hidden />
            Reconnecting to Tjommie
          </div>
        )}
      </main>
    </div>
  );
}
