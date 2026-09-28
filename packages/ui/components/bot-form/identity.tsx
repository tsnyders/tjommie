"use client";

import { BotColor, BotShape } from "@tjommie/protocol";
import type { BotForm } from "@/lib/bot-form";
import { cn } from "@/lib/utils";
import { BOT_COLORS, BotAvatar } from "../bot-avatar";
import { FIELD, Label } from "../controls";

// The avatar preview, the name, and the color and shape pickers (WF3, WF8). The shapes preview in the bot's color.
export function Identity({ form, onChange, nameError }: { form: BotForm; onChange: (form: BotForm) => void; nameError: string | null }) {
  return (
    <div className="flex gap-6">
      <div className="flex size-[156px] shrink-0 items-center justify-center rounded-[14px] bg-card">
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
        <div role="radiogroup" aria-label="Color" className="mt-3 flex justify-between gap-2">
          {BotColor.options.map((color) => (
            <button
              key={color}
              type="button"
              role="radio"
              aria-checked={form.color === color}
              aria-label={color}
              onClick={() => onChange({ ...form, color })}
              style={{ backgroundColor: BOT_COLORS[color] }}
              className={cn("size-7 shrink-0 rounded-full", form.color === color && "ring-2 ring-text ring-offset-[3px] ring-offset-canvas")}
            />
          ))}
        </div>
        <div role="radiogroup" aria-label="Shape" className="mt-4 flex justify-between gap-2">
          {BotShape.options.map((shape) => (
            <button
              key={shape}
              type="button"
              role="radio"
              aria-checked={form.shape === shape}
              aria-label={shape.replace("-", " ")}
              onClick={() => onChange({ ...form, shape })}
              className={cn("flex size-8 shrink-0 items-center justify-center rounded-[10px] bg-popover", form.shape === shape && "bg-selected ring-1 ring-inset ring-accent")}
            >
              <BotAvatar color={form.color} shape={shape} size={24} />
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
