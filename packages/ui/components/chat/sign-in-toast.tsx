"use client";

import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import Link from "next/link";
import { Toast } from "@base-ui/react/toast";
import { Plug, X } from "lucide-react";
import { IconButton } from "@/components/icon-button";
import { MENU_POPUP } from "@/components/menu";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";

const ShownNotices = createContext<Set<string> | null>(null);
const noticeKey = (host: string | null, subject: string): string => JSON.stringify([host, subject]);
type NoticeData = { href: string; action: "Sign in" | "Settings" };

export function SignInNotifications({ children }: { children: ReactNode }) {
  const [shown] = useState(() => new Set<string>());
  const host = useApp((state) => state.host);
  const accounts = useApp((state) => state.accounts);
  const setup = useApp((state) => state.setup);

  useEffect(() => {
    // A resolved sign-in can notify again if that account later signs out.
    for (const account of accounts ?? []) {
      if (account.setup?.signedIn) shown.delete(noticeKey(host, `account:${account.id}`));
    }
    for (const provider of setup ?? []) {
      if (provider.signedIn) shown.delete(noticeKey(host, `provider:${provider.provider}`));
    }
  }, [accounts, setup, host, shown]);

  return <ShownNotices.Provider value={shown}>
    <Toast.Provider timeout={8000} limit={1}>
      {children}
      <SignInViewport />
    </Toast.Provider>
  </ShownNotices.Provider>;
}

export function SignInToast({ subject, title, href, action = "Sign in" }: { subject: string; title: string; href: string; action?: NoticeData["action"] }) {
  const shown = useContext(ShownNotices);
  const host = useApp((state) => state.host);
  const { add, close } = Toast.useToastManager<NoticeData>();
  const key = noticeKey(host, subject);

  useEffect(() => {
    let cancelled = false;
    let id: string | undefined;
    // Wait through Strict Mode's discarded effect before marking this notice as shown.
    queueMicrotask(() => {
      if (cancelled || !shown || shown.has(key)) return;
      shown.add(key);
      id = add({ id: `sign-in:${key}`, title, data: { href, action } });
    });
    return () => {
      cancelled = true;
      if (id) close(id);
    };
  }, [add, close, shown, key, title, href, action]);

  return null;
}

function SignInViewport() {
  const { toasts } = Toast.useToastManager<NoticeData>();
  return <Toast.Portal>
    <Toast.Viewport aria-label="Account notifications" className="pointer-events-none fixed left-4 right-4 top-[calc(max(env(safe-area-inset-top,0px),var(--safe-area-inset-top,0px))+4rem)] z-30 flex flex-col gap-2 outline-none sm:left-auto sm:w-[360px]">
      {toasts.map((toast) => <Toast.Root key={toast.id} toast={toast} swipeDirection={["up", "right"]}
        data-slot="sign-in-toast"
        className={cn(MENU_POPUP, "pointer-events-auto min-w-0 p-2 data-[limited]:hidden data-[ending-style]:opacity-0")}>
        <Toast.Content className="flex items-center gap-2">
          <Plug className="ml-2 size-4 shrink-0 text-icon" aria-hidden />
          <Toast.Title className="min-w-0 flex-1 text-[13px] leading-[18px] text-text [overflow-wrap:anywhere]" />
          {toast.data && <Toast.Action render={<Link href={toast.data.href} />} nativeButton={false}
            className="press flex min-h-8 shrink-0 items-center justify-center rounded-[10px] px-3 text-[13px] text-accent hover:bg-pill max-sm:min-h-11">{toast.data.action}</Toast.Action>}
          <Toast.Close render={<IconButton label="Dismiss account notification" title="Dismiss" className="max-sm:size-11" />}>
            <X className="size-4" aria-hidden />
          </Toast.Close>
        </Toast.Content>
      </Toast.Root>)}
    </Toast.Viewport>
  </Toast.Portal>;
}
