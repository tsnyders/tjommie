"use client";

import { Radio } from "@base-ui/react/radio";
import { RadioGroup } from "@base-ui/react/radio-group";
import { BotColor, BotShape } from "@tjommie/protocol";
import type { BotForm } from "@/lib/bot-form";
import { cn } from "@/lib/utils";
import { BOT_COLORS, BotAvatar } from "../bot-avatar";
import { FIELD, Label } from "../controls";

// The avatar preview, the name, and the color and shape pickers (WF3, WF8). The shapes preview in the bot's color.
// Each picker is one radio group: Tab reaches the chosen swatch, and the arrow keys move the choice.
export function Identity({ form, onChange, nameError }: { form: BotForm; onChange: (form: BotForm) => void; nameError: string | null }) {
  return (
    <div className="flex gap-6 max-sm:flex-col max-sm:gap-4">
      <div className="flex size-[156px] shrink-0 items-center justify-center rounded-[14px] bg-card max-sm:self-center">
        <BotAvatar color={form.color} shape={form.shape} size={96} />
      </div>
      <div className="min-w-0 flex-1">
        <Label htmlFor="bot-name">Name</Label>
        <input
          id="bot-name"
          value={form.name}
          maxLength={40}
          aria-invalid={nameError ? true : undefined}
          onChange={(event) => onChange({ ...form, name: event.target.value })}
          className={cn(FIELD, "mt-1.5 h-11")}
        />
        {nameError && (
          <p role="alert" className="mt-1 text-[12px] leading-[18px] text-danger">
            {nameError}
          </p>
        )}
        <RadioGroup aria-label="Color" value={form.color} onValueChange={(color) => onChange({ ...form, color })} className="mt-3 flex justify-between gap-2 max-sm:flex-wrap max-sm:justify-start max-sm:gap-3">
          {BotColor.options.map((color) => (
            <Radio.Root
              key={color}
              value={color}
              aria-label={color}
              style={{ backgroundColor: BOT_COLORS[color] }}
              className={cn("size-7 shrink-0 rounded-full", form.color === color && "ring-2 ring-text ring-offset-[3px] ring-offset-canvas")}
            />
          ))}
        </RadioGroup>
        <RadioGroup aria-label="Shape" value={form.shape} onValueChange={(shape) => onChange({ ...form, shape })} className="mt-4 flex justify-between gap-2 max-sm:flex-wrap max-sm:justify-start max-sm:gap-3">
          {BotShape.options.map((shape) => (
            <Radio.Root
              key={shape}
              value={shape}
              aria-label={shape.replace("-", " ")}
              className={cn("flex size-8 shrink-0 items-center justify-center rounded-[10px] bg-popover", form.shape === shape && "bg-selected ring-1 ring-inset ring-accent")}
            >
              <BotAvatar color={form.color} shape={shape} size={24} />
            </Radio.Root>
          ))}
        </RadioGroup>
      </div>
    </div>
  );
}
