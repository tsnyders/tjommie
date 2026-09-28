import type { ButtonHTMLAttributes } from "react";
import { cn } from "@/lib/utils";

// Pill buttons from the wireframes: light for the main action, dark for the others. BAR_PILL is the larger size of
// a page's action bar (WF3, WF8).
export const BAR_PILL = "h-9 px-4";

export function Pill({ primary = false, className, ...props }: ButtonHTMLAttributes<HTMLButtonElement> & { primary?: boolean }) {
  return (
    <button
      type="button"
      className={cn(
        "press inline-flex h-8 shrink-0 items-center gap-1.5 rounded-full px-3.5 text-[14px] font-medium whitespace-nowrap disabled:opacity-50",
        primary ? "bg-user-bubble text-user-text" : "bg-pill text-text",
        className,
      )}
      {...props}
    />
  );
}
