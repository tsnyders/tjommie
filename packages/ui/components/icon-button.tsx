import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";

// A 32 px icon button. Its label is also its tooltip. A ref reaches the button.
export function IconButton({ label, active = false, className, ...props }: ComponentProps<"button"> & { label: string; active?: boolean }) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      aria-pressed={active || undefined}
      className={cn(
        "press flex size-8 shrink-0 items-center justify-center rounded-[10px] text-icon hover:bg-pill hover:text-text [&_svg]:size-4",
        active && "bg-selected text-text",
        className,
      )}
      {...props}
    />
  );
}
