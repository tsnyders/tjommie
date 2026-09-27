"use client";

import type { ReactNode } from "react";
import { Minus, Plus } from "lucide-react";
import { Radio } from "@base-ui/react/radio";
import { RadioGroup } from "@base-ui/react/radio-group";
import { Switch as BaseSwitch } from "@base-ui/react/switch";

// Text inputs and areas (WF3): radius 10 on the card fill, with a 1 px stroke. Add h-11 for one line. The global
// focus ring draws inside the box, over the stroke.
export const FIELD = "w-full rounded-[10px] border border-input-stroke bg-card px-4 text-[14px] leading-[21px] text-text placeholder:text-placeholder";

export function Label({ htmlFor, children }: { htmlFor: string; children: ReactNode }) {
  return (
    <label htmlFor={htmlFor} className="block text-[13px] leading-[18px] text-secondary">
      {children}
    </label>
  );
}

// 36 x 20 (WF3, WF4): on is a light track with a dark knob at the right; off is a gray track and knob.
export function Switch({ label, checked, onChange, disabled = false }: { label: string; checked: boolean; onChange: (checked: boolean) => void; disabled?: boolean }) {
  return (
    <BaseSwitch.Root
      aria-label={label}
      checked={checked}
      onCheckedChange={(next) => onChange(next)}
      disabled={disabled}
      className="relative inline-flex h-5 w-9 shrink-0 cursor-default items-center rounded-full bg-selected p-0.5 transition-colors duration-150 data-[checked]:bg-user-bubble data-[disabled]:opacity-50"
    >
      <BaseSwitch.Thumb className="size-4 rounded-full bg-secondary transition-transform duration-150 ease-[cubic-bezier(0.23,1,0.32,1)] data-[checked]:translate-x-4 data-[checked]:bg-canvas" />
    </BaseSwitch.Root>
  );
}

// A segmented control (Autonomy, Stop after, Appearance, the model picker's provider): radio buttons on a track.
export function Segmented<T extends string>({ label, value, options, onChange }: { label: string; value: T; options: readonly { value: T; label: string }[]; onChange: (value: T) => void }) {
  return (
    <RadioGroup aria-label={label} value={value} onValueChange={(next) => onChange(next as T)} className="inline-flex h-8 shrink-0 items-center gap-0.5 rounded-full bg-popover p-1">
      {options.map((option) => (
        <Radio.Root
          key={option.value}
          value={option.value}
          className="flex h-6 cursor-default items-center whitespace-nowrap rounded-full px-3 text-[13px] text-secondary hover:text-body data-[checked]:bg-selected data-[checked]:font-semibold data-[checked]:text-text"
        >
          {option.label}
        </Radio.Root>
      ))}
    </RadioGroup>
  );
}

// The 120 x 32 number stepper (Chain limit).
export function Stepper({ label, value, min, max, onChange }: { label: string; value: number; min: number; max: number; onChange: (value: number) => void }) {
  const button = "flex size-6 items-center justify-center rounded-full text-icon hover:bg-pill hover:text-text disabled:opacity-40";
  return (
    <div role="group" aria-label={label} className="flex h-8 w-[120px] shrink-0 items-center justify-between rounded-full bg-popover px-1">
      <button type="button" aria-label="Decrease" disabled={value <= min} onClick={() => onChange(value - 1)} className={button}>
        <Minus className="size-4" aria-hidden />
      </button>
      <span aria-live="polite" className="text-[14px] font-semibold text-text">
        {value}
      </span>
      <button type="button" aria-label="Increase" disabled={value >= max} onClick={() => onChange(value + 1)} className={button}>
        <Plus className="size-4" aria-hidden />
      </button>
    </div>
  );
}
