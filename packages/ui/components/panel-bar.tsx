import Link from "next/link";
import { ChevronsRight, Settings } from "lucide-react";
import { captionClear } from "@/lib/native-capability";
import { cn } from "@/lib/utils";
import { IconButton } from "./icon-button";

// The right panel's top bar: settings and hide at the left. Windows draws its caption buttons over the right end.
export function PanelBar({ settingsHref, settingsLabel, hideLabel, onHide }: { settingsHref: string; settingsLabel: string; hideLabel: string; onHide: () => void }) {
  return (
    <div className={cn("drag flex h-11 shrink-0 items-center px-4 max-sm:h-14 max-sm:px-2", captionClear())}>
      <Link href={settingsHref} aria-label={settingsLabel} title={settingsLabel} className="press flex size-8 items-center justify-center rounded-[10px] text-icon hover:bg-pill hover:text-text max-sm:hidden">
        <Settings className="size-4" aria-hidden />
      </Link>
      <IconButton label={hideLabel} onClick={onHide} className="max-sm:size-11">
        <ChevronsRight aria-hidden />
      </IconButton>
    </div>
  );
}
