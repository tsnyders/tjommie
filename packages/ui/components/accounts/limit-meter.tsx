import type { AccountLimitWindow } from "@tjommie/protocol";
import { limitLabel, limitNear, limitValue } from "@/lib/accounts";
import { cn } from "@/lib/utils";

// A bar for something that fills up: a track with a fill that is the body color, and the accent once it is near full. Never red:
// a limit is a pause, not an error. A share of null is a bar with no reading.
export function Meter({ label, value, share, near, healthy = "bg-body" }: { label: string; value: string; share: number | null; near: boolean; healthy?: "bg-body" | "bg-working" }) {
  const percent = share === null ? 0 : Math.round(share * 100);
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-baseline justify-between gap-3 text-[12px] leading-[18px]">
        <span className="text-secondary">{label}</span>
        <span className="min-w-0 break-words text-right text-body">{value}</span>
      </div>
      <div role="meter" aria-label={label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={share === null ? undefined : percent} aria-valuetext={value} className="h-1.5 overflow-hidden rounded-full bg-pill">
        <div className={cn("h-full rounded-full", near ? "bg-accent" : healthy)} style={{ width: `${percent}%` }} />
      </div>
    </div>
  );
}

// One limit window of a subscription: "5-hour limit", "42% · resets 3:10 PM".
export function LimitMeter({ window, now }: { window: AccountLimitWindow; now: Date }) {
  return <Meter label={limitLabel(window)} value={limitValue(window, now)} share={window.usedPercent === null ? null : window.usedPercent / 100} near={limitNear(window)} />;
}
