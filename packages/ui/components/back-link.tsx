"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { ChevronLeft } from "lucide-react";

// The phone's way back where there is no tab bar (below 640 px). `label` is its name for a screen reader; `children` is what shows, when the
// name alone isn't it. With `history`, it goes to the page before this one if there was one, else to `href`.
export function BackLink({ href, label, history = false, children }: { href: string; label: string; history?: boolean; children?: string }) {
  const router = useRouter();
  return (
    <Link
      href={href}
      aria-label={label}
      onClick={(event) => {
        if (history && window.history.length > 1) {
          event.preventDefault();
          router.back();
        }
      }}
      className="press flex h-11 shrink-0 items-center gap-1 rounded-[10px] pl-2 pr-3 text-[14px] text-body hover:bg-pill sm:hidden"
    >
      <ChevronLeft className="size-5 text-icon" aria-hidden />
      {children}
    </Link>
  );
}
