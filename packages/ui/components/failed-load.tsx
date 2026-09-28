"use client";

import { RotateCcw } from "lucide-react";
import { Pill } from "./pill";

// A chat, the Team room or an exchange whose history didn't load. A new connection also loads it again.
export function FailedLoad({ text, onRetry }: { text: string; onRetry: () => void }) {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-3 px-8 text-center">
      <p className="text-[14px] leading-[21px] text-body">{text}</p>
      <Pill onClick={onRetry}>
        <RotateCcw className="size-3.5" aria-hidden />
        Retry
      </Pill>
    </div>
  );
}
